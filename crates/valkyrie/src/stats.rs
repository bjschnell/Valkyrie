//! `valk stats`: whether Valkyrie is making the agents better, week by week
//! (backlog #176). It reads what Valkyrie already records on this machine (the
//! sessions' event logs, the project's serving, failure, supervisor and policy
//! logs, the backlog) and, with the `analytics` setting on, which features got
//! used. Nothing is sent anywhere. Trends here are correlation, not cause.

use crate::pings::{self, Ping};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use valkyrie_context::served::{Found, FoundServe, Serve};
use valkyrie_context::steps::Record as Step;
use valkyrie_context::usage::{self, Use};
use valkyrie_context::{FailureLine, Store, outcome, project};
use valkyrie_proto::{Item, ItemStatus, Note};

const WEEK: u64 = 7 * 86_400;

/// Subcommands that are servers or plumbing rather than something you use.
const NOT_FEATURES: &[&str] = &[
    "daemon",
    // Runs once per `valk --remote` connection, on the far side: plumbing.
    "bridge",
    "hook",
    "context-hook",
    "statusline",
    "mcp",
    "help",
];

/// Everything `compute` reads, gathered from disk by `gather`.
#[derive(Default)]
pub struct Input {
    pub pings: Vec<Ping>,
    pub served: Vec<Serve>,
    pub found: Vec<FoundServe>,
    pub failures: Vec<FailureLine>,
    pub notes: Vec<Note>,
    pub steps: Vec<Step>,
    pub outcomes: Vec<outcome::Record>,
    pub items: Vec<Item>,
    /// `None` when analytics is off.
    pub uses: Option<Vec<Use>>,
    /// Every subcommand there is, for the ones never used.
    pub commands: Vec<String>,
}

/// One number per week, oldest first.
type Weekly = Vec<f64>;

pub struct Stats {
    pub now: u64,
    pub weeks: usize,
    pub project: String,
    pub rows: Vec<Section>,
    pub trends: Vec<Trend>,
    /// `(agent and model, shipped, median days to ship)` over the window.
    pub launches: Vec<(String, usize, Option<f64>)>,
    pub usage: Option<Usage>,
}

pub struct Section {
    pub title: &'static str,
    pub rows: Vec<(&'static str, Weekly)>,
}

/// A ratio over the window's later half against its earlier half.
pub struct Trend {
    pub name: &'static str,
    pub before: Option<f64>,
    pub after: Option<f64>,
    /// A lower number is the better one.
    pub lower_is_better: bool,
}

impl Trend {
    /// `better`, `worse`, `flat`, or `—` without both halves.
    pub fn verdict(&self) -> &'static str {
        let (Some(b), Some(a)) = (self.before, self.after) else {
            return "—";
        };
        // Within a tenth of each other (or both tiny) is no change.
        if (a - b).abs() <= 0.1 * b.abs().max(a.abs()).max(0.05) {
            return "flat";
        }
        if (a < b) == self.lower_is_better {
            "better"
        } else {
            "worse"
        }
    }
}

pub struct Usage {
    /// The first use recorded, in seconds.
    pub since: Option<u64>,
    /// `(surface, feature, by you, by agents)`, most used first.
    pub features: Vec<(String, String, usize, usize)>,
    /// Subcommands nobody ran in the window.
    pub unused: Vec<String>,
}

/// The week `at` (seconds) falls in, 0 the oldest, or `None` outside the window.
fn week(at: u64, now: u64, weeks: usize) -> Option<usize> {
    let back = usize::try_from(now.checked_sub(at)? / WEEK).ok()?;
    (back < weeks).then(|| weeks - 1 - back)
}

fn counts(weeks: usize, ats: impl Iterator<Item = u64>, now: u64) -> Weekly {
    let mut out = vec![0.0; weeks];
    for at in ats {
        if let Some(w) = week(at, now, weeks) {
            out[w] += 1.0;
        }
    }
    out
}

/// Distinct keys per week.
fn distinct<K: std::hash::Hash + Eq>(
    weeks: usize,
    keyed: impl Iterator<Item = (u64, K)>,
    now: u64,
) -> Weekly {
    let mut sets: Vec<HashSet<K>> = (0..weeks).map(|_| HashSet::new()).collect();
    for (at, k) in keyed {
        if let Some(w) = week(at, now, weeks) {
            sets[w].insert(k);
        }
    }
    sets.iter().map(|s| s.len() as f64).collect()
}

/// `a / b` week by week; NaN where `b` is 0, shown as `–`.
fn per(a: &Weekly, b: &Weekly) -> Weekly {
    a.iter()
        .zip(b)
        .map(|(a, b)| if *b > 0.0 { a / b } else { f64::NAN })
        .collect()
}

fn median(mut xs: Vec<f64>) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(f64::total_cmp);
    let m = xs.len() / 2;
    Some(if xs.len().is_multiple_of(2) {
        f64::midpoint(xs[m - 1], xs[m])
    } else {
        xs[m]
    })
}

/// The window's halves as `sum(num) / sum(den)` each, later half second.
fn halves(num: &Weekly, den: &Weekly) -> (Option<f64>, Option<f64>) {
    let cut = num.len() / 2;
    let ratio = |r: std::ops::Range<usize>| {
        let d: f64 = den[r.clone()].iter().sum();
        (d > 0.0).then(|| num[r].iter().sum::<f64>() / d)
    };
    (ratio(0..cut), ratio(cut..num.len()))
}

/// A stop that waited on you: a question, permission, input, screen or bell,
/// a block, or a turn end that asked something. Not a turn end that only left
/// work short of shipping: the daemon records where work was left only since
/// ADR-0013, so counting those would make every later week look worse.
fn needed_you(p: &Ping) -> bool {
    match p.kind {
        pings::KEPT_GOING | pings::SUPERVISED => false,
        "turn ended" => p.ask.is_some(),
        _ => true,
    }
}

pub fn compute(input: &Input, now: u64, weeks: usize, project: &str) -> Stats {
    let w = weeks;
    let ping_secs = |p: &Ping| p.t / 1000;

    // Sessions: how often the agents stopped on you.
    let sessions = distinct(
        w,
        input.pings.iter().map(|p| (ping_secs(p), &p.session)),
        now,
    );
    let waited = counts(
        w,
        input.pings.iter().filter(|p| needed_you(p)).map(ping_secs),
        now,
    );
    let kept = counts(
        w,
        input
            .pings
            .iter()
            .filter(|p| p.kind == pings::KEPT_GOING)
            .map(ping_secs),
        now,
    );

    // Context: what reached the agents, and whether the failures learned from
    // kept happening.
    let conversations = distinct(w, input.served.iter().map(|s| (s.at, &s.session)), now);
    let read = distinct(
        w,
        input
            .served
            .iter()
            .filter(|s| s.layer.whole())
            .map(|s| (s.at, (&s.session, s.id))),
        now,
    );
    let notes_found = counts(
        w,
        input
            .found
            .iter()
            .filter(|f| matches!(f.found, Found::Note(_)))
            .map(|f| f.at),
        now,
    );
    let past_found = counts(
        w,
        input
            .found
            .iter()
            .filter(|f| matches!(f.found, Found::Transcript { .. }))
            .map(|f| f.at),
        now,
    );
    // When each signature was first learned from: a note written from it or a
    // decision proposed from it.
    let mut learned: HashMap<u64, u64> = HashMap::new();
    let mut learn = |sig: u64, at: u64| {
        let e = learned.entry(sig).or_insert(at);
        *e = (*e).min(at);
    };
    for n in &input.notes {
        if let Some(sig) = n
            .sig
            .as_deref()
            .and_then(|s| u64::from_str_radix(s, 16).ok())
        {
            learn(sig, n.created);
        }
    }
    for l in input.failures.iter().filter(|l| l.decision.is_some()) {
        learn(l.sig, l.at);
    }
    let calls: Vec<&FailureLine> = input.failures.iter().filter(|l| l.call.is_some()).collect();
    let failed = counts(w, calls.iter().map(|l| l.at), now);
    let repeated = counts(
        w,
        calls
            .iter()
            .filter(|l| learned.get(&l.sig).is_some_and(|&at| at < l.at))
            .map(|l| l.at),
        now,
    );

    // Supervisor and policies.
    let offered = counts(w, input.steps.iter().map(|s| s.at), now);
    let accepted = counts(
        w,
        input.steps.iter().filter(|s| s.accepted()).map(|s| s.at),
        now,
    );
    let by_policy = counts(
        w,
        input
            .steps
            .iter()
            .filter(|s| s.policy.is_some())
            .map(|s| s.at),
        now,
    );
    let handled = counts(
        w,
        input
            .outcomes
            .iter()
            .filter(|o| !o.overridden)
            .map(|o| o.at),
        now,
    );
    let overridden = counts(
        w,
        input.outcomes.iter().filter(|o| o.overridden).map(|o| o.at),
        now,
    );

    // Backlog.
    let by_you = |i: &&Item| matches!(i.by.as_str(), "human" | "you" | "me");
    let filed_you = counts(w, input.items.iter().filter(by_you).map(|i| i.created), now);
    let filed_agents = counts(
        w,
        input.items.iter().filter(|i| !by_you(i)).map(|i| i.created),
        now,
    );
    let shipped: Vec<&Item> = input
        .items
        .iter()
        .filter(|i| i.status == ItemStatus::Shipped)
        .collect();
    let shipped_n = counts(w, shipped.iter().map(|i| i.updated), now);
    let mut ship_days: Vec<Vec<f64>> = vec![Vec::new(); w];
    let mut launches: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for i in &shipped {
        let Some(wk) = week(i.updated, now, w) else {
            continue;
        };
        let days = i.updated.saturating_sub(i.created) as f64 / 86_400.0;
        ship_days[wk].push(days);
        let who = match &i.launch {
            Some(l) if l.model.is_empty() => format!("{} (default model)", l.agent),
            Some(l) => format!("{} {}", l.agent, l.model),
            None => "not launched from the board".to_owned(),
        };
        launches.entry(who).or_default().push(days);
    }
    let ship_median: Weekly = ship_days
        .into_iter()
        .map(|d| median(d).unwrap_or(f64::NAN))
        .collect();
    let mut launches: Vec<(String, usize, Option<f64>)> = launches
        .into_iter()
        .map(|(who, days)| (who, days.len(), median(days)))
        .collect();
    launches.sort_by_key(|(_, n, _)| std::cmp::Reverse(*n));

    let steps_taken: f64 = offered.iter().sum();
    let blocks: Vec<f64> = handled
        .iter()
        .zip(&overridden)
        .map(|(a, b)| a + b)
        .collect();
    let trends = vec![
        trend(
            "stops that needed you, per session",
            &waited,
            &sessions,
            true,
        ),
        trend(
            "failed tool calls, per conversation",
            &failed,
            &conversations,
            true,
        ),
        trend(
            "failures already noted or decided, per conversation",
            &repeated,
            &conversations,
            true,
        ),
        trend(
            "policy blocks you overrode, share",
            &overridden,
            &blocks,
            true,
        ),
        trend(
            "supervisor steps taken as offered, share",
            &accepted,
            &offered,
            false,
        ),
    ];
    let trends = trends
        .into_iter()
        // A supervisor that offered nothing has no share to compare.
        .filter(|t| !(t.name.starts_with("supervisor") && steps_taken == 0.0))
        .collect();

    let rows = vec![
        Section {
            title: "Sessions",
            rows: vec![
                ("agent sessions", sessions.clone()),
                ("stops that needed you", waited.clone()),
                ("  per session", per(&waited, &sessions)),
                ("stops a policy kept going", kept),
            ],
        },
        Section {
            title: "Context",
            rows: vec![
                ("conversations that got context", conversations.clone()),
                (
                    "decisions read whole, per conv.",
                    per(&read, &conversations),
                ),
                ("notes found by a search", notes_found),
                ("past-session lines found", past_found),
                ("failed tool calls", failed),
                ("  already noted or decided", repeated),
            ],
        },
        Section {
            title: "Supervisor",
            rows: vec![
                ("steps offered", offered),
                ("  taken as offered", accepted),
                ("  taken by a policy", by_policy),
                ("policy blocks handled", handled),
                ("policy blocks you overrode", overridden),
            ],
        },
        Section {
            title: "Backlog",
            rows: vec![
                ("items you filed", filed_you),
                ("items agents filed", filed_agents),
                ("shipped", shipped_n),
                ("median days to ship", ship_median),
            ],
        },
    ];

    let usage = input.uses.as_ref().map(|uses| {
        let start = now.saturating_sub(w as u64 * WEEK);
        let window: Vec<&Use> = uses.iter().filter(|u| u.at > start).collect();
        let mut by: BTreeMap<(String, String), (usize, usize)> = BTreeMap::new();
        for u in &window {
            let e = by
                .entry((u.surface.clone(), u.feature.clone()))
                .or_default();
            if u.by_agent() {
                e.1 += 1;
            } else {
                e.0 += 1;
            }
        }
        let mut features: Vec<_> = by
            .into_iter()
            .map(|((s, f), (you, agents))| (s, f, you, agents))
            .collect();
        features.sort_by_key(|(s, f, you, agents)| {
            (std::cmp::Reverse(you + agents), s.clone(), f.clone())
        });
        let used: HashSet<&str> = window
            .iter()
            .filter(|u| u.surface == "cli")
            .map(|u| u.feature.split(' ').next().unwrap_or(""))
            .collect();
        Usage {
            since: uses.iter().map(|u| u.at).min(),
            features,
            unused: input
                .commands
                .iter()
                .filter(|c| !used.contains(c.as_str()))
                .cloned()
                .collect(),
        }
    });

    Stats {
        now,
        weeks: w,
        project: project.to_owned(),
        rows,
        trends,
        launches,
        usage,
    }
}

fn trend(name: &'static str, num: &Weekly, den: &Weekly, lower_is_better: bool) -> Trend {
    let (before, after) = halves(num, den);
    Trend {
        name,
        before,
        after,
        lower_is_better,
    }
}

/// Reads everything `compute` needs for `root`'s project.
pub fn gather(root: &Path, now: u64, weeks: usize) -> Result<Input> {
    let since = now.saturating_sub(weeks as u64 * WEEK);
    let state = valkyrie_proto::state_dir();
    let store = Store::new(crate::context::store_base());
    let pings = pings::collect(&state.join("sessions"), since * 1000, Some(root))?;
    let items = read_items(&state.join("backlog"))
        .into_iter()
        .filter(|i| i.project.as_deref().is_some_and(|p| same(p, root)))
        .collect();
    let uses = usage::enabled().then(|| {
        usage::read(&usage::default_file(), now)
            .into_iter()
            // TUI and web uses have no project: they count everywhere.
            .filter(|u| u.project.as_deref().is_none_or(|p| same(p, root)))
            .collect()
    });
    Ok(Input {
        pings,
        served: store.served(root, now),
        found: store.found_served(root, now),
        failures: store.failure_log(root, now),
        notes: store.notes(root),
        steps: store.steps(root, now),
        outcomes: store.outcomes(root, now),
        items,
        uses,
        commands: commands(),
    })
}

fn same(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    a == b || canon(a) == canon(b)
}

/// Every backlog item; a file that doesn't parse is skipped.
fn read_items(dir: &Path) -> Vec<Item> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| serde_json::from_slice(&std::fs::read(p).ok()?).ok())
        .collect()
}

/// The top-level subcommands someone would run, by name.
fn commands() -> Vec<String> {
    use clap::CommandFactory;
    crate::Cli::command()
        .get_subcommands()
        .filter(|c| !c.is_hide_set() && !NOT_FEATURES.contains(&c.get_name()))
        .map(|c| c.get_name().to_owned())
        .collect()
}

/// The subcommand path `args` (without the program) runs, by its names, like
/// `todo add`: what a use is recorded as. Options and values are never kept.
pub fn feature(cmd: &clap::Command, args: &[String]) -> Vec<String> {
    let mut cmd = cmd;
    let mut path = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if let Some(opt) = arg.strip_prefix('-') {
            // An option that takes a value in the next argument: skip that too.
            let name = opt.trim_start_matches('-');
            let takes = !arg.contains('=')
                && cmd.get_arguments().any(|a| {
                    a.get_action().takes_values()
                        && (a.get_long() == Some(name)
                            || (name.len() == 1 && a.get_short() == name.chars().next()))
                });
            if takes {
                args.next();
            }
            continue;
        }
        match cmd.find_subcommand(arg) {
            Some(sub) => {
                path.push(sub.get_name().to_owned());
                cmd = sub;
            }
            None => break,
        }
    }
    path
}

/// Records this run of `valk`, if analytics is on and it ran something you'd
/// call a feature.
pub fn record_cli() {
    if !usage::enabled() {
        return;
    }
    use clap::CommandFactory;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = feature(&crate::Cli::command(), &args);
    let Some(top) = path.first() else { return };
    // Bare `valk web` is the bridge, a server like the daemon; `web pair` isn't.
    if NOT_FEATURES.contains(&top.as_str()) || path == ["web"] {
        return;
    }
    record("cli", &path.join(" "));
}

/// Records a use of `feature` on `surface` by whoever runs this process.
pub fn record(surface: &str, feature: &str) {
    let by = valkyrie_daemon::agent_above(std::process::id()).unwrap_or(usage::YOU);
    let project = std::env::current_dir().ok().map(|d| project::root(&d));
    usage::record(
        &usage::default_file(),
        &Use {
            at: now_secs(),
            surface: surface.to_owned(),
            feature: feature.to_owned(),
            by: by.to_owned(),
            project,
        },
    );
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn date(at: u64) -> String {
    jiff::Timestamp::from_second(at as i64)
        .map(|t| {
            t.to_zoned_datetime(jiff::tz::TimeZone::system())
                .strftime("%Y-%m-%d")
                .to_string()
        })
        .unwrap_or_default()
}

fn cell(x: f64) -> String {
    if x.is_nan() {
        "–".to_owned()
    } else if x.fract() == 0.0 {
        format!("{x:.0}")
    } else {
        format!("{x:.1}")
    }
}

fn ratio(x: Option<f64>) -> String {
    x.map_or_else(|| "–".to_owned(), |x| format!("{x:.2}"))
}

pub fn report(s: &Stats) -> String {
    let mut out = String::new();
    let start = s.now.saturating_sub(s.weeks as u64 * WEEK);
    let _ = writeln!(
        out,
        "valk stats: {}, the last {} weeks (from {})",
        s.project,
        s.weeks,
        date(start)
    );
    let _ = writeln!(
        out,
        "Correlation, not cause: these move with everything else that changed too.\n"
    );

    let half = s.weeks / 2;
    let _ = writeln!(
        out,
        "Trend: the last {} weeks against the {} before",
        s.weeks - half,
        half
    );
    let width = s.trends.iter().map(|t| t.name.len()).max().unwrap_or(0);
    for t in &s.trends {
        let _ = writeln!(
            out,
            "  {:width$}  {:>5} → {:<5}  {}",
            t.name,
            ratio(t.before),
            ratio(t.after),
            t.verdict()
        );
    }

    let label = s
        .rows
        .iter()
        .flat_map(|sec| sec.rows.iter().map(|(n, _)| n.len()))
        .max()
        .unwrap_or(0)
        + 2;
    let _ = write!(out, "\n{:label$}", "week of");
    for i in 0..s.weeks {
        let at = start + i as u64 * WEEK;
        let d = date(at);
        let _ = write!(out, "  {:>5}", d.get(5..).unwrap_or(&d));
    }
    out.push('\n');
    for sec in &s.rows {
        let _ = writeln!(out, "{}", sec.title);
        for (name, weekly) in &sec.rows {
            let _ = write!(out, "  {:w$}", name, w = label - 2);
            for x in weekly {
                let _ = write!(out, "  {:>5}", cell(*x));
            }
            out.push('\n');
        }
    }

    if !s.launches.is_empty() {
        let _ = writeln!(out, "\nShipped, by the agent and model last launched");
        for (who, n, days) in &s.launches {
            let days = days.map_or_else(String::new, |d| format!(", median {d:.1} days"));
            let _ = writeln!(out, "  {who}: {n}{days}");
        }
    }

    match &s.usage {
        None => {
            let _ = writeln!(
                out,
                "\nFeatures: not counted. Turn Analytics on in the settings panel (`,`), \
                 or `analytics = on` in settings.toml; it stays on this machine."
            );
        }
        Some(u) => {
            let since = u.since.map_or_else(
                || "nothing yet".to_owned(),
                |at| format!("since {}", date(at)),
            );
            let _ = writeln!(out, "\nFeatures used ({since}): you / agents");
            for (surface, feature, you, agents) in u.features.iter().take(25) {
                let _ = writeln!(out, "  {surface:4} {feature:28} {you:>5} / {agents}");
            }
            if u.features.len() > 25 {
                let _ = writeln!(out, "  … and {} more (--json)", u.features.len() - 25);
            }
            if u.since.is_some() && !u.unused.is_empty() {
                let _ = writeln!(out, "Commands nobody ran: {}", u.unused.join(", "));
            }
        }
    }
    out
}

fn num(x: f64) -> Value {
    if x.is_nan() { Value::Null } else { json!(x) }
}

pub fn json(s: &Stats) -> Value {
    let start = s.now.saturating_sub(s.weeks as u64 * WEEK);
    let sections: serde_json::Map<String, Value> = s
        .rows
        .iter()
        .map(|sec| {
            let rows: serde_json::Map<String, Value> = sec
                .rows
                .iter()
                .map(|(n, w)| {
                    (
                        n.trim().to_owned(),
                        w.iter().copied().map(num).collect::<Value>(),
                    )
                })
                .collect();
            (sec.title.to_lowercase(), Value::Object(rows))
        })
        .collect();
    json!({
        "project": s.project,
        "weeks": (0..s.weeks).map(|i| start + i as u64 * WEEK).collect::<Vec<_>>(),
        "trends": s.trends.iter().map(|t| json!({
            "name": t.name,
            "before": t.before,
            "after": t.after,
            "lower_is_better": t.lower_is_better,
            "verdict": t.verdict(),
        })).collect::<Vec<_>>(),
        "sections": sections,
        "shipped_by_launch": s.launches.iter().map(|(who, n, d)| json!({
            "launch": who, "shipped": n, "median_days": d,
        })).collect::<Vec<_>>(),
        "usage": s.usage.as_ref().map(|u| json!({
            "since": u.since,
            "features": u.features.iter().map(|(s, f, you, agents)| json!({
                "surface": s, "feature": f, "you": you, "agents": agents,
            })).collect::<Vec<_>>(),
            "unused": u.unused,
        })),
    })
}

/// The project `repo` (or the current directory) belongs to.
pub fn root(repo: Option<&Path>) -> Result<PathBuf> {
    let dir = match repo {
        Some(r) => r.to_path_buf(),
        None => std::env::current_dir()?,
    };
    Ok(project::root(&dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_context::served::Layer;

    const NOW: u64 = 100 * WEEK;

    fn ping(kind: &'static str, session: &str, at: u64, ask: bool) -> Ping {
        Ping {
            kind,
            session: session.into(),
            t: at * 1000,
            seen: false,
            summary: String::new(),
            ask: ask.then(|| "want me to push?".into()),
            source: None,
            work: Some("uncommitted".into()),
            cwd: None,
        }
    }

    fn row<'a>(s: &'a Stats, name: &str) -> &'a Weekly {
        s.rows
            .iter()
            .flat_map(|sec| &sec.rows)
            .find(|(n, _)| n.trim() == name)
            .map(|(_, w)| w)
            .unwrap()
    }

    fn trend<'a>(s: &'a Stats, start: &str) -> &'a Trend {
        s.trends.iter().find(|t| t.name.starts_with(start)).unwrap()
    }

    #[test]
    fn weeks_count_back_from_now_oldest_first() {
        assert_eq!(week(NOW, NOW, 4), Some(3));
        assert_eq!(week(NOW - WEEK + 1, NOW, 4), Some(3));
        assert_eq!(week(NOW - WEEK, NOW, 4), Some(2));
        assert_eq!(week(NOW - 4 * WEEK, NOW, 4), None);
        assert_eq!(
            week(NOW + 5, NOW, 4),
            None,
            "the future isn't in the window"
        );
    }

    #[test]
    fn stops_that_needed_you_leave_out_work_states_and_policies() {
        let old = NOW - 3 * WEEK;
        let input = Input {
            pings: vec![
                ping("question", "a", old, false),
                ping("permission", "a", old, false),
                ping("turn ended", "a", old, true),
                // Only left work short: not counted, see `needed_you`.
                ping("turn ended", "b", NOW - 10, false),
                ping("question", "b", NOW - 10, false),
                ping(pings::KEPT_GOING, "b", NOW - 10, false),
            ],
            ..Input::default()
        };
        let s = compute(&input, NOW, 4, "/r");
        assert_eq!(row(&s, "agent sessions"), &vec![1.0, 0.0, 0.0, 1.0]);
        assert_eq!(row(&s, "stops that needed you"), &vec![3.0, 0.0, 0.0, 1.0]);
        assert_eq!(
            row(&s, "stops a policy kept going"),
            &vec![0.0, 0.0, 0.0, 1.0]
        );
        assert!(row(&s, "per session")[1].is_nan());
        let t = trend(&s, "stops that needed you");
        assert_eq!((t.before, t.after), (Some(3.0), Some(1.0)));
        assert_eq!(t.verdict(), "better");
    }

    #[test]
    fn a_failure_counts_as_repeated_only_after_it_was_learned_from() {
        let sig = 0xabc;
        let fail = |at: u64, call: &str| FailureLine {
            at,
            sig,
            session: Some("s".into()),
            call: Some(call.into()),
            decision: None,
        };
        let note: Note = serde_json::from_value(json!({
            "id": 1, "project": "/r", "text": "use --socket", "status": "live",
            "created": NOW - 2 * WEEK, "by": "valkyrie", "session": "s",
            "sig": format!("{sig:016x}"),
        }))
        .unwrap();
        let input = Input {
            failures: vec![
                fail(NOW - 3 * WEEK, "c1"),
                fail(NOW - WEEK - 5, "c2"),
                fail(NOW - 5, "c3"),
                // Another signature, never learned from.
                FailureLine {
                    sig: 0xdef,
                    ..fail(NOW - 5, "c4")
                },
            ],
            notes: vec![note],
            served: vec![Serve {
                session: "s".into(),
                id: 3,
                layer: Layer::Block,
                at: NOW - 5,
            }],
            ..Input::default()
        };
        let s = compute(&input, NOW, 4, "/r");
        assert_eq!(row(&s, "failed tool calls"), &vec![1.0, 0.0, 1.0, 2.0]);
        assert_eq!(
            row(&s, "already noted or decided"),
            &vec![0.0, 0.0, 1.0, 1.0]
        );
        assert_eq!(
            row(&s, "conversations that got context"),
            &vec![0.0, 0.0, 0.0, 1.0]
        );
    }

    #[test]
    fn supervisor_policies_and_the_backlog_add_up() {
        let step = |at: u64, taken: &str, policy: Option<u32>| {
            valkyrie_context::steps::Record::from_json(&json!({
                "at": at, "session": "x", "trigger": "conflicting",
                "offered": "merge-base", "taken": taken, "pr": "", "policy": policy,
            }))
            .unwrap()
        };
        let outcome = |at: u64, how: &str| {
            outcome::Record::from_json(&json!({
                "at": at, "session": "x", "policy": 5, "scope": "project",
                "project": "/r", "outcome": how, "how": "stop",
            }))
            .unwrap()
        };
        let item = |id: u32, by: &str, status: &str, created: u64, updated: u64, launch: Value| {
            serde_json::from_value::<Item>(json!({
                "id": id, "title": "t", "kind": "bug", "status": status, "priority": "p2",
                "by": by, "boot": 0, "created": created, "updated": updated, "launch": launch,
            }))
            .unwrap()
        };
        let input = Input {
            steps: vec![
                step(NOW - 5, "merge-base", Some(5)),
                step(NOW - 6, "leave", None),
            ],
            outcomes: vec![outcome(NOW - 5, "handled"), outcome(NOW - 6, "overridden")],
            items: vec![
                item(
                    1,
                    "human",
                    "shipped",
                    NOW - WEEK - 86_400,
                    NOW - 5,
                    json!({"agent": "claude", "model": "opus"}),
                ),
                item(
                    2,
                    "claude",
                    "shipped",
                    NOW - 3 * 86_400,
                    NOW - 86_400,
                    json!({"agent": "claude", "model": "opus"}),
                ),
                item(3, "claude", "open", NOW - 5, NOW - 5, Value::Null),
            ],
            ..Input::default()
        };
        let s = compute(&input, NOW, 2, "/r");
        assert_eq!(row(&s, "steps offered"), &vec![0.0, 2.0]);
        assert_eq!(row(&s, "taken as offered"), &vec![0.0, 1.0]);
        assert_eq!(row(&s, "taken by a policy"), &vec![0.0, 1.0]);
        assert_eq!(row(&s, "policy blocks you overrode"), &vec![0.0, 1.0]);
        assert_eq!(row(&s, "items you filed"), &vec![1.0, 0.0]);
        assert_eq!(row(&s, "items agents filed"), &vec![0.0, 2.0]);
        assert_eq!(row(&s, "shipped"), &vec![0.0, 2.0]);
        // 8 days and 2 days.
        assert!((row(&s, "median days to ship")[1] - 5.0).abs() < 0.01);
        let (who, n, days) = &s.launches[0];
        assert_eq!((who.as_str(), *n), ("claude opus", 2));
        assert!(days.is_some_and(|d| (d - 5.0).abs() < 0.01));
        let text = report(&s);
        assert!(text.contains("Correlation, not cause"), "{text}");
        assert!(text.contains("Features: not counted"), "{text}");
        assert_eq!(
            json(&s)["sections"]["backlog"]["shipped"],
            json!([0.0, 2.0])
        );
    }

    #[test]
    fn features_count_you_and_agents_and_name_what_nobody_ran() {
        let u = |at: u64, surface: &str, feature: &str, by: &str| Use {
            at,
            surface: surface.into(),
            feature: feature.into(),
            by: by.into(),
            project: None,
        };
        let input = Input {
            uses: Some(vec![
                u(NOW - 9, "cli", "todo add", "claude"),
                u(NOW - 8, "cli", "todo add", "claude"),
                u(NOW - 7, "cli", "todo list", usage::YOU),
                u(NOW - 6, "web", "launch_item", usage::YOU),
                // Before the window.
                u(NOW - 9 * WEEK, "cli", "pings", usage::YOU),
            ]),
            commands: vec!["todo".into(), "pings".into(), "stats".into()],
            ..Input::default()
        };
        let s = compute(&input, NOW, 4, "/r");
        let usage = s.usage.as_ref().unwrap();
        assert_eq!(
            usage.features[0],
            ("cli".to_owned(), "todo add".to_owned(), 0, 2)
        );
        assert_eq!(usage.unused, ["pings", "stats"]);
        assert_eq!(usage.since, Some(NOW - 9 * WEEK));
        assert!(report(&s).contains("Commands nobody ran: pings, stats"));
    }

    #[test]
    fn a_use_is_named_by_its_subcommands_never_its_values() {
        use clap::CommandFactory;
        let cmd = crate::Cli::command();
        let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            feature(&cmd, &args(&["todo", "add", "-k", "bug", "secret title"])),
            ["todo", "add"]
        );
        assert_eq!(
            feature(
                &cmd,
                &args(&["--socket", "/tmp/todo", "pings", "--days", "3"])
            ),
            ["pings"]
        );
        assert_eq!(feature(&cmd, &args(&["stats", "--json"])), ["stats"]);
        assert!(feature(&cmd, &args(&[])).is_empty());
        assert!(feature(&cmd, &args(&["not-a-command"])).is_empty());
    }
}

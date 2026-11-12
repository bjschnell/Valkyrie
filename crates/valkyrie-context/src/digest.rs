//! The away digest (backlog #105): what every agent did while you were gone, from
//! what the daemon already records. Each session's event log folds into a `Run`
//! (where it worked, its branch and pull request, what it was asked, how it
//! ended), each run gets one `Outcome`, and `render` writes the lot as markdown to
//! paste. `Presence` remembers when you were last gone, so the digest knows where
//! "while you were gone" starts. Reading only: nothing here acts on a session.

use crate::project;
use anyhow::{Context, Result, bail};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned, civil};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use valkyrie_proto::{Digest, DigestEntry, DigestGroup, Item, ItemStatus, Outcome};

/// No typing anywhere for this long counts as being gone.
pub const AWAY_GAP_MS: u64 = 30 * 60 * 1000;
/// With no record of you leaving, the digest looks this far back.
pub const FALLBACK_MS: u64 = 24 * 60 * 60 * 1000;
/// Idle this long with work short of shipping, a live session counts as
/// abandoned; sooner, it's stuck (it still waits on you).
pub const ABANDONED_MS: u64 = 2 * 60 * 60 * 1000;
/// Presence is written at most this often while you type.
const SAVE_EVERY_MS: u64 = 60 * 1000;
/// Longest `what` line.
const MAX_WHAT: usize = 90;

/// When you last typed into any session, and when you were last gone.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    /// Your last keystroke (ms since the epoch; 0: none recorded).
    pub last_ms: u64,
    /// The keystroke before the last gap of `AWAY_GAP_MS` or more: when you left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_ms: Option<u64>,
    #[serde(skip)]
    saved_ms: u64,
}

impl Presence {
    /// You typed at `now`. Returns whether to write it down: on coming back, and
    /// otherwise at most once a minute.
    pub fn typed(&mut self, now: u64) -> bool {
        let back = self.last_ms > 0 && now.saturating_sub(self.last_ms) >= AWAY_GAP_MS;
        if back {
            self.left_ms = Some(self.last_ms);
        }
        self.last_ms = self.last_ms.max(now);
        let due = back || now.saturating_sub(self.saved_ms) >= SAVE_EVERY_MS;
        if due {
            self.saved_ms = now;
        }
        due
    }

    /// Where "while you were gone" starts: your last keystroke while you're still
    /// gone, else when you last left. `None` with nothing recorded.
    pub fn since(&self, now: u64) -> Option<u64> {
        if self.last_ms == 0 {
            return None;
        }
        if now.saturating_sub(self.last_ms) >= AWAY_GAP_MS {
            return Some(self.last_ms);
        }
        self.left_ms
    }
}

/// One session, as its event log tells it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Run {
    /// The log's name, `<created ns>-<id>`.
    pub key: String,
    pub started_ms: u64,
    /// The agent seen in it (`claude`, `codex`); `None`: a shell, left out.
    pub agent: Option<String>,
    pub name: Option<String>,
    pub topic: Option<String>,
    /// Where its agent last worked, from its hooks.
    pub cwd: Option<PathBuf>,
    pub branch: Option<String>,
    pub pr: Option<String>,
    /// The work state its last turn end left (`clean`, `uncommitted`, `unpushed`,
    /// `no-pr`, `pr-red`).
    pub work: Option<String>,
    /// The last agent state and what it said with it.
    pub state: Option<String>,
    pub status_summary: Option<String>,
    pub seen: bool,
    /// The agent's own last line at the end of a turn.
    pub finished: Option<String>,
    /// The supervisor's last flag on it.
    pub flag: Option<String>,
    /// Your first prompt in the window, and how many.
    pub prompt: Option<String>,
    pub prompts: u32,
    /// Pull requests this log was told merged: any session in the repository is.
    pub merged: Vec<String>,
    pub exited_ms: Option<u64>,
    /// First and last activity in the window; `None`: nothing happened in it.
    pub first_ms: Option<u64>,
    pub last_ms: u64,
}

impl Run {
    fn active(&mut self, t: u64, since: u64, until: u64) {
        if t >= since && t <= until {
            self.first_ms = Some(self.first_ms.map_or(t, |f| f.min(t)));
            self.last_ms = self.last_ms.max(t);
        }
    }
}

/// Folds the event log at `path` up to `until`. Every log is read whole, so a
/// branch or pull request recorded before the window still names the work.
pub fn read_run(path: &Path, since: u64, until: u64) -> Result<Run> {
    let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let key = path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".events.jsonl"))
        .unwrap_or_default()
        .to_owned();
    let mut run = Run {
        key,
        ..Run::default()
    };
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        // Most of a log is output and screen scans: skip them unparsed.
        if line.starts_with(r#"{"k":"out""#) || line.starts_with(r#"{"k":"scan""#) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        fold(&mut run, &v, since, until);
    }
    Ok(run)
}

fn text(v: &Value) -> Option<String> {
    v.as_str()
        .map(crate::backlog::one_line)
        .filter(|s| !s.is_empty())
}

fn agent_name(v: &Value) -> Option<String> {
    v.as_str()
        .filter(|a| !a.is_empty() && *a != "generic" && *a != "unknown")
        .map(str::to_owned)
}

fn fold(run: &mut Run, v: &Value, since: u64, until: u64) {
    let t = v["t"].as_u64().unwrap_or(0);
    if t > until {
        return;
    }
    match v["k"].as_str().unwrap_or_default() {
        "start" => {
            run.started_ms = t;
            run.agent = agent_name(&v["agent"]).or(run.agent.take());
        }
        "foreground" => run.agent = agent_name(&v["agent"]).or(run.agent.take()),
        "rename" => run.name = text(&v["name"]),
        "topic" => {
            run.topic = text(&v["topic"]);
            run.active(t, since, until);
        }
        "hook" => {
            if v["ignored"].as_bool() == Some(true) {
                return;
            }
            if let Some(agent) = agent_name(&v["agent"]) {
                run.agent = Some(agent);
            }
            let p = &v["payload"];
            if let Some(cwd) = p["cwd"].as_str().filter(|c| !c.is_empty()) {
                run.cwd = Some(PathBuf::from(cwd));
            }
            if p["hook_event_name"] == "UserPromptSubmit" && t >= since {
                run.prompts += 1;
                if run.prompt.is_none() {
                    run.prompt = text(&p["prompt"]);
                }
            }
            run.active(t, since, until);
        }
        "state" => {
            let s = &v["status"];
            if let Some(agent) = agent_name(&s["agent"]) {
                run.agent = Some(agent);
            }
            run.state = s["state"].as_str().map(str::to_owned);
            run.status_summary = text(&s["summary"]);
            run.seen = s["seen"].as_bool().unwrap_or(true);
            if run.state.as_deref() == Some("review_ready")
                && let Some(summary) = &run.status_summary
            {
                run.finished = Some(summary.clone());
            }
            run.active(t, since, until);
        }
        "seen" => run.seen = true,
        "work" => {
            run.work = v["state"].as_str().map(str::to_owned);
            if let Some(branch) = text(&v["branch"]) {
                run.branch = Some(branch);
            }
            if let Some(pr) = text(&v["pr"]) {
                run.pr = Some(pr);
            }
            run.active(t, since, until);
        }
        "flag" => {
            run.flag = text(&v["summary"]);
            run.active(t, since, until);
        }
        "merged" => {
            if let Some(pr) = text(&v["pr"]) {
                run.merged.push(pr);
            }
        }
        "exit" => {
            run.exited_ms = Some(t);
            run.active(t, since, until);
        }
        _ => {}
    }
}

/// The backlog item a run worked on: the one with its pull request, else its
/// branch, the latest changed first.
pub fn item_for<'a>(run: &Run, items: &'a [Item]) -> Option<&'a Item> {
    let by = |f: &dyn Fn(&Item) -> bool| {
        items
            .iter()
            .filter(|i| f(i))
            .max_by_key(|i| (i.updated, i.id))
    };
    run.pr
        .as_ref()
        .and_then(|pr| by(&|i| i.pr.as_ref() == Some(pr)))
        .or_else(|| {
            run.branch
                .as_ref()
                .and_then(|b| by(&|i| i.branch.as_ref() == Some(b)))
        })
}

/// `#123` for a pull request URL, else the URL.
pub fn pr_label(url: &str) -> String {
    match url.rsplit_once("/pull/") {
        Some((_, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => format!("PR #{n}"),
        _ => format!("PR {url}"),
    }
}

/// `2h 10m`, `35m`, `3d 4h`.
pub fn span(ms: u64) -> String {
    let m = ms / 60_000;
    match m {
        0 => "under a minute".into(),
        1..60 => format!("{m}m"),
        60..1440 => match m % 60 {
            0 => format!("{}h", m / 60),
            r => format!("{}h {r}m", m / 60),
        },
        _ => match (m / 60) % 24 {
            0 => format!("{}d", m / 1440),
            h => format!("{}d {h}h", m / 1440),
        },
    }
}

/// Where a run's work ended up, and why, one line.
pub fn classify(
    run: &Run,
    item: Option<&Item>,
    merged: &HashSet<String>,
    live: bool,
    now: u64,
) -> (Outcome, String) {
    let pr = run.pr.as_ref().or(item.and_then(|i| i.pr.as_ref()));
    if let Some(pr) = pr.filter(|pr| merged.contains(*pr)) {
        return (Outcome::Shipped, format!("{} merged", pr_label(pr)));
    }
    if let Some(item) = item.filter(|i| i.status == ItemStatus::Shipped) {
        return (Outcome::Shipped, format!("#{} shipped", item.id));
    }
    let ended = !live || run.exited_ms.is_some() || run.state.as_deref() == Some("exited");
    let said = |fallback: &str| {
        run.status_summary
            .clone()
            .unwrap_or_else(|| fallback.to_owned())
    };
    if !ended {
        match run.state.as_deref() {
            Some("working") => return (Outcome::Working, "working".into()),
            Some("needs_input") => {
                return (Outcome::Stuck, format!("needs you: {}", said("waiting on you")));
            }
            Some("blocked") => {
                let why = run.flag.clone().unwrap_or_else(|| said("blocked"));
                return (Outcome::Stuck, why);
            }
            Some("stale") => return (Outcome::Stuck, "no output for a long while".into()),
            Some("interrupted") => return (Outcome::Stuck, "interrupted mid-turn".into()),
            _ => {}
        }
    }
    if run.work.as_deref() == Some("pr-red") {
        let on = pr.map(|p| format!(" on {}", pr_label(p))).unwrap_or_default();
        return (Outcome::Stuck, format!("CI red{on}"));
    }
    if let Some(pr) = pr {
        if item.is_some_and(|i| i.pr_closed) {
            return (
                Outcome::Abandoned,
                format!("{} closed without merging", pr_label(pr)),
            );
        }
        return (Outcome::Waiting, format!("{} open", pr_label(pr)));
    }
    let short = match run.work.as_deref() {
        Some("uncommitted") => Some("uncommitted changes"),
        Some("unpushed") => Some("commits not pushed"),
        Some("no-pr") => Some("pushed, no PR"),
        _ => None,
    };
    if let Some(short) = short {
        let idle = now.saturating_sub(run.last_ms);
        if ended {
            return (Outcome::Abandoned, format!("ended with {short}"));
        }
        if idle >= ABANDONED_MS {
            return (Outcome::Abandoned, format!("idle {} with {short}", span(idle)));
        }
        return (Outcome::Stuck, format!("stopped with {short}"));
    }
    if ended {
        if matches!(run.state.as_deref(), Some("needs_input" | "blocked")) {
            return (Outcome::Abandoned, "ended while it waited on you".into());
        }
        return (Outcome::Done, "ended".into());
    }
    if run.seen {
        (Outcome::Done, "finished".into())
    } else {
        (Outcome::Done, "finished, not looked at yet".into())
    }
}

/// The repository a run worked in, by folder name. A worktree that's gone can't
/// be asked, but Valkyrie names them `<repo>-<branch>` beside the repository, so
/// it's matched against `roots` by that name.
pub fn repo_name(cwd: Option<&Path>, roots: &[PathBuf]) -> String {
    let Some(cwd) = cwd else {
        return "unknown".into();
    };
    let name = |p: &Path| {
        p.file_name()
            .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
    };
    if cwd.exists() {
        return name(&project::root(cwd));
    }
    if let Some(root) = roots.iter().find(|r| cwd.starts_with(r)) {
        return name(root);
    }
    roots
        .iter()
        .filter(|root| {
            let (Some(parent), Some(n)) = (root.parent(), root.file_name()) else {
                return false;
            };
            let prefix = format!("{}-", n.to_string_lossy());
            cwd.strip_prefix(parent)
                .ok()
                .and_then(|rest| rest.components().next())
                .is_some_and(|c| c.as_os_str().to_string_lossy().starts_with(&prefix))
        })
        .max_by_key(|r| r.as_os_str().len())
        .map_or_else(|| name(cwd), |r| name(r))
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max - 1).collect();
    format!("{}…", cut.trim_end())
}

/// What a run was about, one line.
pub fn what(run: &Run, item: Option<&Item>, agent: &str) -> String {
    if let Some(item) = item {
        return clip(&format!("#{} {}", item.id, item.title), MAX_WHAT);
    }
    let dir = run
        .cwd
        .as_deref()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned());
    let named = run
        .name
        .as_ref()
        .filter(|n| n.as_str() != agent && Some(n.as_str()) != dir.as_deref());
    run.topic
        .as_ref()
        .or(named)
        .or(run.prompt.as_ref())
        .map(|w| clip(w, MAX_WHAT))
        .unwrap_or_else(|| format!("{agent} in {}", dir.as_deref().unwrap_or("?")))
}

/// What the daemon knows beside the logs.
pub struct Known<'a> {
    pub items: &'a [Item],
    /// Live sessions by log key: their ids, to jump to them.
    pub live: &'a BTreeMap<String, u32>,
    /// Repository roots known to exist, to place gone worktrees.
    pub roots: &'a [PathBuf],
}

/// The entries for the runs active in the window, most urgent first. Shells are
/// left out.
pub fn entries(runs: &[Run], known: &Known, now: u64) -> Vec<DigestEntry> {
    let mut merged: HashSet<String> = runs.iter().flat_map(|r| r.merged.iter().cloned()).collect();
    merged.extend(
        known
            .items
            .iter()
            .filter(|i| i.status == ItemStatus::Shipped)
            .filter_map(|i| i.pr.clone()),
    );
    let mut out: Vec<DigestEntry> = runs
        .iter()
        .filter_map(|run| {
            let first_ms = run.first_ms?;
            let agent = run.agent.clone()?;
            let item = item_for(run, known.items);
            let session = known.live.get(&run.key).copied();
            let (outcome, detail) = classify(run, item, &merged, session.is_some(), now);
            let cwd = run.cwd.clone();
            let repo = match (&cwd, item.and_then(|i| i.project.as_deref())) {
                (Some(c), _) if c.exists() => repo_name(Some(c), known.roots),
                (_, Some(project)) => repo_name(Some(project), known.roots),
                (c, None) => repo_name(c.as_deref(), known.roots),
            };
            Some(DigestEntry {
                key: run.key.clone(),
                session,
                repo,
                what: what(run, item, &agent),
                agent,
                cwd,
                item: item.map(|i| i.id),
                branch: run.branch.clone(),
                pr: run.pr.clone().or_else(|| item.and_then(|i| i.pr.clone())),
                outcome,
                detail,
                summary: run.finished.clone(),
                prompts: run.prompts,
                first_ms,
                last_ms: run.last_ms,
                ended_ms: run.exited_ms,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.outcome
            .cmp(&b.outcome)
            .then(b.last_ms.cmp(&a.last_ms))
            .then(a.key.cmp(&b.key))
    });
    out
}

fn zoned(ms: u64, tz: &TimeZone) -> Option<Zoned> {
    Timestamp::from_millisecond(i64::try_from(ms).ok()?)
        .ok()
        .map(|t| t.to_zoned(tz.clone()))
}

fn day_label(ms: u64, tz: &TimeZone) -> String {
    zoned(ms, tz).map_or_else(|| "?".into(), |z| z.strftime("%a %-d %b").to_string())
}

fn clock(ms: u64, tz: &TimeZone) -> String {
    zoned(ms, tz).map_or_else(|| "?".into(), |z| z.strftime("%H:%M").to_string())
}

fn when(ms: u64, tz: &TimeZone) -> String {
    zoned(ms, tz).map_or_else(|| "?".into(), |z| z.strftime("%a %-d %b %H:%M").to_string())
}

/// The digest as markdown to paste: a heading per repository or day, a line per
/// session, its agent's own last line under it.
pub fn render(digest: &Digest, tz: &TimeZone) -> String {
    let mut out = String::from("# Away digest\n\n");
    let window = span(digest.until_ms.saturating_sub(digest.since_ms));
    let from = when(digest.since_ms, tz);
    if digest.away {
        out.push_str(&format!("Since you left, {from} ({window} ago).\n\n"));
    } else {
        out.push_str(&format!("Since {from} ({window} ago).\n\n"));
    }
    if digest.entries.is_empty() {
        out.push_str("No agent did anything in that time.\n");
        return out;
    }
    let counts: Vec<String> = Outcome::ALL
        .iter()
        .filter_map(|o| {
            let n = digest.entries.iter().filter(|e| e.outcome == *o).count();
            (n > 0).then(|| format!("{n} {}", o.label()))
        })
        .collect();
    let n = digest.entries.len();
    out.push_str(&format!(
        "{n} agent session{}: {}.\n",
        if n == 1 { "" } else { "s" },
        counts.join(", ")
    ));
    let mut groups: Vec<(String, Vec<&DigestEntry>)> = Vec::new();
    let mut ordered: Vec<&DigestEntry> = digest.entries.iter().collect();
    if digest.by == DigestGroup::Day {
        // Days newest first; within a day, the entries' own order.
        ordered.sort_by_key(|e| std::cmp::Reverse(day_key(e.last_ms, tz)));
    }
    for e in ordered {
        let key = match digest.by {
            DigestGroup::Repo => e.repo.clone(),
            DigestGroup::Day => day_label(e.last_ms, tz),
        };
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, list)) => list.push(e),
            None => groups.push((key, vec![e])),
        }
    }
    for (key, list) in groups {
        out.push_str(&format!("\n## {key}\n\n"));
        for e in list {
            let at = if digest.by == DigestGroup::Repo {
                format!("{}, {}", e.agent, day_clock(e, tz))
            } else {
                format!("{} in {}, {}", e.agent, e.repo, clock_span(e, tz))
            };
            out.push_str(&format!(
                "- **{}**: {}: {} ({at})\n",
                e.outcome.label(),
                e.what,
                e.detail
            ));
            if let Some(summary) = &e.summary {
                out.push_str(&format!("  > {summary}\n"));
            }
        }
    }
    out
}

fn day_key(ms: u64, tz: &TimeZone) -> Option<civil::Date> {
    zoned(ms, tz).map(|z| z.date())
}

/// `10:00–11:00` within a day, or both ends in full across midnight.
fn clock_span(e: &DigestEntry, tz: &TimeZone) -> String {
    if day_key(e.first_ms, tz) != day_key(e.last_ms, tz) {
        return format!("{} – {}", when(e.first_ms, tz), when(e.last_ms, tz));
    }
    let (a, b) = (clock(e.first_ms, tz), clock(e.last_ms, tz));
    if a == b { a } else { format!("{a}–{b}") }
}

fn day_clock(e: &DigestEntry, tz: &TimeZone) -> String {
    if day_key(e.first_ms, tz) != day_key(e.last_ms, tz) {
        return clock_span(e, tz);
    }
    format!("{} {}", day_label(e.last_ms, tz), clock_span(e, tz))
}

/// When a `--since` starts: `90m`, `12h`, `3d`, `1w`; `today`, `yesterday`; a
/// date (`2026-11-10`, local midnight) or a local time (`2026-11-10 09:00`).
pub fn parse_since(text: &str, now: u64, tz: &TimeZone) -> Result<u64> {
    let text = text.trim();
    let unit = |c: char| match c {
        'm' => Some(60_000u64),
        'h' => Some(3_600_000),
        'd' => Some(86_400_000),
        'w' => Some(7 * 86_400_000),
        _ => None,
    };
    if let Some(last) = text.chars().last()
        && let Some(ms) = unit(last)
        && let Ok(n) = text[..text.len() - 1].parse::<u64>()
    {
        return Ok(now.saturating_sub(n.saturating_mul(ms)));
    }
    let today = zoned(now, tz).context("the clock")?.date();
    let start = match text {
        "today" => today.to_zoned(tz.clone())?,
        "yesterday" => today.yesterday()?.to_zoned(tz.clone())?,
        // A date parses from a date and time too, dropping the time: try the
        // time first.
        _ => match text.replace(' ', "T").parse::<civil::DateTime>() {
            Ok(dt) if text.contains(':') => dt.to_zoned(tz.clone())?,
            _ => match text.parse::<civil::Date>() {
                Ok(d) => d.to_zoned(tz.clone())?,
                Err(_) => bail!(
                    "--since {text:?}: give 90m, 12h, 3d, today, yesterday, 2026-11-10 or \"2026-11-10 09:00\""
                ),
            },
        },
    };
    let ms = start.timestamp().as_millisecond();
    Ok(u64::try_from(ms).unwrap_or(0).min(now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeMap;

    const H: u64 = 3_600_000;
    /// 2026-11-10 12:00 UTC.
    const NOON: u64 = 1_794_312_000_000;

    fn write_log(dir: &Path, stem: &str, lines: &[Value]) -> PathBuf {
        let path = dir.join(format!("{stem}.events.jsonl"));
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("valkyrie-digest-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hook(t: u64, cwd: &str, event: &str, prompt: Option<&str>) -> Value {
        let mut payload = json!({"cwd": cwd, "hook_event_name": event});
        if let Some(p) = prompt {
            payload["prompt"] = p.into();
        }
        json!({"k": "hook", "t": t, "off": 0, "agent": "claude", "ignored": false, "events": [], "payload": payload})
    }

    fn state(t: u64, state: &str, summary: Option<&str>, seen: bool) -> Value {
        json!({"k": "state", "t": t, "off": 0, "status": {"agent": "claude", "state": state,
            "summary": summary, "seen": seen, "seq": 1, "since_ms": t, "ask": null, "hooked": true}})
    }

    #[test]
    fn a_log_folds_into_its_run_and_only_the_window_counts_as_activity() {
        let dir = tempdir("fold");
        let path = write_log(
            &dir,
            "1-7",
            &[
                json!({"k": "start", "t": NOON - 5 * H, "off": 0, "agent": "generic", "command": ["fish"]}),
                json!({"k": "foreground", "t": NOON - 5 * H, "off": 0, "agent": "claude"}),
                hook(NOON - 5 * H, "/r/app", "UserPromptSubmit", Some("old ask")),
                json!({"k": "work", "t": NOON - 4 * H, "off": 0, "state": "clean", "branch": "feat/x", "pr": "https://github.com/o/r/pull/12"}),
                json!({"k": "out", "t": NOON - H, "off": 0}),
                hook(NOON - H, "/r/app", "UserPromptSubmit", Some("fix   the\nflaky test")),
                hook(NOON - H + 1, "/r/app", "UserPromptSubmit", Some("and push")),
                state(NOON - H + 2, "review_ready", Some("Pushed the fix."), false),
                json!({"k": "topic", "t": NOON - H + 3, "off": 0, "topic": "Flaky test"}),
                // After the window: ignored.
                state(NOON + H, "working", None, true),
            ],
        );
        let run = read_run(&path, NOON - 2 * H, NOON).unwrap();
        assert_eq!(run.key, "1-7");
        assert_eq!(run.agent.as_deref(), Some("claude"));
        assert_eq!(run.cwd.as_deref(), Some(Path::new("/r/app")));
        assert_eq!(run.branch.as_deref(), Some("feat/x"));
        assert_eq!(run.pr.as_deref(), Some("https://github.com/o/r/pull/12"));
        assert_eq!(run.prompt.as_deref(), Some("fix the flaky test"));
        assert_eq!(run.prompts, 2);
        assert_eq!(run.state.as_deref(), Some("review_ready"));
        assert_eq!(run.finished.as_deref(), Some("Pushed the fix."));
        assert!(!run.seen);
        assert_eq!(run.topic.as_deref(), Some("Flaky test"));
        assert_eq!(run.first_ms, Some(NOON - H));
        assert_eq!(run.last_ms, NOON - H + 3);

        let quiet = read_run(&path, NOON - 30 * 60_000, NOON).unwrap();
        assert_eq!(quiet.first_ms, None, "nothing happened in the last half hour");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn run(state: &str, work: Option<&str>, pr: Option<&str>) -> Run {
        Run {
            key: "k".into(),
            agent: Some("claude".into()),
            state: Some(state.into()),
            work: work.map(Into::into),
            pr: pr.map(Into::into),
            seen: true,
            first_ms: Some(NOON - H),
            last_ms: NOON - H,
            ..Run::default()
        }
    }

    fn item(id: u32, status: ItemStatus, pr: Option<&str>) -> Item {
        let mut i: Item = serde_json::from_value(json!({
            "id": id, "title": format!("item {id}"), "created": 0, "updated": 0,
        }))
        .unwrap();
        (i.status, i.pr) = (status, pr.map(Into::into));
        i
    }

    #[test]
    fn outcomes_follow_where_the_work_ended_up() {
        let pr = "https://github.com/o/r/pull/12";
        let merged: HashSet<String> = [pr.to_owned()].into();
        let none = HashSet::new();
        let c = |r: &Run, i: Option<&Item>, m: &HashSet<String>, live: bool| {
            classify(r, i, m, live, NOON)
        };
        assert_eq!(
            c(&run("idle", Some("clean"), Some(pr)), None, &merged, true),
            (Outcome::Shipped, "PR #12 merged".into())
        );
        let shipped = item(5, ItemStatus::Shipped, None);
        assert_eq!(
            c(&run("exited", None, None), Some(&shipped), &none, false).0,
            Outcome::Shipped
        );
        assert_eq!(c(&run("working", None, None), None, &none, true).0, Outcome::Working);
        let mut asking = run("needs_input", None, None);
        asking.status_summary = Some("Permission: Bash rm -rf target".into());
        assert_eq!(
            c(&asking, None, &none, true),
            (Outcome::Stuck, "needs you: Permission: Bash rm -rf target".into())
        );
        let mut flagged = run("blocked", Some("pr-red"), Some(pr));
        flagged.flag = Some("CI red on PR #12: lint".into());
        assert_eq!(
            c(&flagged, None, &none, true),
            (Outcome::Stuck, "CI red on PR #12: lint".into())
        );
        assert_eq!(
            c(&run("exited", Some("pr-red"), Some(pr)), None, &none, false),
            (Outcome::Stuck, "CI red on PR #12".into())
        );
        assert_eq!(
            c(&run("idle", Some("clean"), Some(pr)), None, &none, true),
            (Outcome::Waiting, "PR #12 open".into())
        );
        let mut closed = item(6, ItemStatus::Open, Some(pr));
        closed.pr_closed = true;
        assert_eq!(
            c(&run("idle", Some("clean"), None), Some(&closed), &none, true).0,
            Outcome::Abandoned
        );
        assert_eq!(
            c(&run("exited", Some("uncommitted"), None), None, &none, false),
            (Outcome::Abandoned, "ended with uncommitted changes".into())
        );
        let mut idle = run("review_ready", Some("unpushed"), None);
        idle.last_ms = NOON - 3 * H;
        assert_eq!(
            c(&idle, None, &none, true),
            (Outcome::Abandoned, "idle 3h with commits not pushed".into())
        );
        idle.last_ms = NOON - H;
        assert_eq!(
            c(&idle, None, &none, true),
            (Outcome::Stuck, "stopped with commits not pushed".into())
        );
        // A log with no exit whose session isn't live died with its daemon.
        assert_eq!(
            c(&run("needs_input", None, None), None, &none, false),
            (Outcome::Abandoned, "ended while it waited on you".into())
        );
        let mut unread = run("review_ready", Some("clean"), None);
        unread.seen = false;
        assert_eq!(
            c(&unread, None, &none, true),
            (Outcome::Done, "finished, not looked at yet".into())
        );
    }

    #[test]
    fn entries_leave_out_shells_and_quiet_runs_and_put_the_urgent_first() {
        let pr = "https://github.com/o/r/pull/12";
        let mut shipped = run("idle", Some("clean"), Some(pr));
        shipped.key = "a".into();
        shipped.cwd = Some("/gone/repos/app-feat-x/src".into());
        shipped.merged = vec![pr.into()];
        let mut stuck = run("needs_input", None, None);
        stuck.key = "b".into();
        stuck.prompt = Some("why is the build slow".into());
        let mut shell = run("idle", None, None);
        shell.key = "c".into();
        shell.agent = None;
        let mut quiet = run("idle", None, None);
        quiet.key = "d".into();
        quiet.first_ms = None;
        let mut on_it = item(9, ItemStatus::Open, Some(pr));
        on_it.branch = Some("feat/x".into());
        let items = [on_it];
        let live: BTreeMap<String, u32> = [("b".to_owned(), 4)].into();
        let roots = [PathBuf::from("/gone/repos/app")];
        let known = Known {
            items: &items,
            live: &live,
            roots: &roots,
        };
        let got = entries(&[shipped, stuck, shell, quiet], &known, NOON);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].outcome, Outcome::Stuck);
        assert_eq!(got[0].session, Some(4));
        assert_eq!(got[0].what, "why is the build slow");
        assert_eq!(got[1].outcome, Outcome::Shipped);
        assert_eq!(got[1].item, Some(9));
        assert_eq!(got[1].what, "#9 item 9");
        assert_eq!(got[1].repo, "app", "a gone worktree is placed by its name");
    }

    #[test]
    fn the_markdown_groups_by_repo_or_day() {
        let entry = |outcome, repo: &str, last_ms, summary: Option<&str>| DigestEntry {
            key: "k".into(),
            session: None,
            repo: repo.into(),
            cwd: None,
            agent: "claude".into(),
            what: "#9 Away digest".into(),
            item: Some(9),
            branch: None,
            pr: None,
            outcome,
            detail: "PR #12 open".into(),
            summary: summary.map(Into::into),
            prompts: 1,
            first_ms: last_ms - H,
            last_ms,
            ended_ms: None,
        };
        let mut digest = Digest {
            since_ms: NOON - 26 * H,
            until_ms: NOON,
            away: true,
            by: DigestGroup::Repo,
            entries: vec![
                entry(Outcome::Waiting, "valkyrie", NOON - H, Some("Opened PR #12.")),
                entry(Outcome::Waiting, "dotfiles", NOON - 20 * H, None),
            ],
            markdown: String::new(),
        };
        let md = render(&digest, &TimeZone::UTC);
        assert_eq!(
            md,
            "# Away digest\n\nSince you left, Mon 9 Nov 10:00 (1d 2h ago).\n\n\
             2 agent sessions: 2 waiting on PR/CI.\n\
             \n## valkyrie\n\n\
             - **waiting on PR/CI**: #9 Away digest: PR #12 open (claude, Tue 10 Nov 10:00–11:00)\n  > Opened PR #12.\n\
             \n## dotfiles\n\n\
             - **waiting on PR/CI**: #9 Away digest: PR #12 open (claude, Mon 9 Nov 15:00–16:00)\n"
        );
        digest.by = DigestGroup::Day;
        let md = render(&digest, &TimeZone::UTC);
        assert!(md.contains("\n## Tue 10 Nov\n\n- **waiting on PR/CI**: #9 Away digest: PR #12 open (claude in valkyrie, 10:00–11:00)\n"), "{md}");
        assert!(md.find("## Tue 10 Nov") < md.find("## Mon 9 Nov"));
        // Across midnight, both ends in full.
        digest.by = DigestGroup::Repo;
        digest.entries[0].first_ms = NOON - 13 * H;
        let md = render(&digest, &TimeZone::UTC);
        assert!(md.contains("(claude, Mon 9 Nov 23:00 – Tue 10 Nov 11:00)"), "{md}");
        digest.entries.clear();
        assert!(render(&digest, &TimeZone::UTC).ends_with("No agent did anything in that time.\n"));
    }

    #[test]
    fn presence_knows_when_you_left() {
        let mut p = Presence::default();
        assert_eq!(p.since(NOON), None);
        assert!(p.typed(NOON - 10 * H), "the first keystroke is written down");
        assert!(!p.typed(NOON - 10 * H + 1000), "not every keystroke");
        // Gone for 9 hours, back at NOON - H.
        assert!(p.typed(NOON - H), "coming back is written down");
        assert_eq!(p.left_ms, Some(NOON - 10 * H + 1000));
        assert_eq!(p.since(NOON - H + 60_000), Some(NOON - 10 * H + 1000));
        // Gone again, checking from the phone without typing.
        assert_eq!(p.since(NOON), Some(NOON - H));
        let saved: Presence = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(saved.since(NOON), p.since(NOON));
    }

    #[test]
    fn since_takes_spans_days_and_times() {
        let tz = TimeZone::UTC;
        assert_eq!(parse_since("90m", NOON, &tz).unwrap(), NOON - 90 * 60_000);
        assert_eq!(parse_since("12h", NOON, &tz).unwrap(), NOON - 12 * H);
        assert_eq!(parse_since("today", NOON, &tz).unwrap(), NOON - 12 * H);
        assert_eq!(parse_since("yesterday", NOON, &tz).unwrap(), NOON - 36 * H);
        assert_eq!(parse_since("2026-11-09", NOON, &tz).unwrap(), NOON - 36 * H);
        assert_eq!(parse_since("2026-11-10 09:30", NOON, &tz).unwrap(), NOON - 150 * 60_000);
        assert_eq!(parse_since("2027-01-01", NOON, &tz).unwrap(), NOON, "never past now");
        assert!(parse_since("last tuesday", NOON, &tz).is_err());
    }

    #[test]
    fn spans_and_pr_labels_read_short() {
        assert_eq!(span(30_000), "under a minute");
        assert_eq!(span(35 * 60_000), "35m");
        assert_eq!(span(2 * H), "2h");
        assert_eq!(span(2 * H + 10 * 60_000), "2h 10m");
        assert_eq!(span(76 * H), "3d 4h");
        assert_eq!(pr_label("https://github.com/o/r/pull/170"), "PR #170");
        assert_eq!(pr_label("https://x/merge_requests/3"), "PR https://x/merge_requests/3");
    }
}

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
use valkyrie_proto::{Digest, DigestEntry, DigestGroup, Item, ItemStatus, Outcome, SearchField};

/// No typing anywhere for this long counts as being gone.
pub const AWAY_GAP_MS: u64 = 30 * 60 * 1000;
/// With no record of you leaving, the digest looks this far back.
pub const FALLBACK_MS: u64 = 24 * 60 * 60 * 1000;
/// Idle this long with work short of shipping, a live session counts as
/// abandoned; sooner, it's stuck (it still waits on you).
pub const ABANDONED_MS: u64 = 2 * 60 * 60 * 1000;
/// The furthest back a digest reaches.
pub const LONGEST_MS: u64 = 31 * 24 * 60 * 60 * 1000;
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
    /// The branch and pull request its last turn end was on: a later branch with
    /// no PR yet clears the PR.
    pub branch: Option<String>,
    pub pr: Option<String>,
    /// Every pull request it was on, oldest first, for one it finished with.
    pub prs: Vec<String>,
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
    /// The command it started with.
    pub command: Vec<String>,
    /// Where it started; older logs don't say, and its agent's first hook does.
    pub home: Option<PathBuf>,
    /// The repository it started in (its main checkout), and the linked worktree,
    /// as the daemon wrote them down; older logs don't say.
    pub repo: Option<PathBuf>,
    pub worktree: Option<PathBuf>,
    /// The agent's own conversation, the latest it was on (backlog #150).
    pub conversation: Option<String>,
    /// The daemon wrote down `conversation` itself: a hook's guess doesn't replace it.
    pub conversation_logged: bool,
    /// First and last activity in the window; `None`: nothing happened in it.
    pub first_ms: Option<u64>,
    pub last_ms: u64,
    /// What was said and written in the window, for search (`read_said`); the
    /// digest leaves it empty.
    pub said: Vec<Said>,
}

/// One line of a session's history: a prompt, a closing reply, a file written.
#[derive(Debug, Clone, PartialEq)]
pub struct Said {
    pub t: u64,
    pub field: SearchField,
    pub text: String,
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
    read(path, since, until, false)
}

/// `read_run`, also keeping what was said and written in the window: the
/// search over session history (backlog #106) reads the logs the digest does.
pub fn read_said(path: &Path, since: u64, until: u64) -> Result<Run> {
    let mut run = read(path, since, until, true)?;
    // A file written many times is one line, at its last write.
    let mut seen = HashSet::new();
    run.said.reverse();
    run.said
        .retain(|s| s.field != SearchField::File || seen.insert(s.text.clone()));
    run.said.reverse();
    Ok(run)
}

fn read(path: &Path, since: u64, until: u64, said: bool) -> Result<Run> {
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
        if said {
            keep_said(&mut run, &v, since, until);
        }
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
            run.command = v["command"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            run.agent = agent_name(&v["agent"]).or(run.agent.take());
            if let Some(cwd) = v["cwd"].as_str().filter(|c| !c.is_empty()) {
                run.home = Some(PathBuf::from(cwd));
            }
            if let Some(name) = text(&v["name"]) {
                run.name = Some(name);
            }
            let path = |k: &str| v[k].as_str().filter(|p| !p.is_empty()).map(PathBuf::from);
            run.repo = path("repo");
            run.worktree = path("worktree");
        }
        "conversation" => {
            run.conversation = text(&v["id"]);
            run.conversation_logged = true;
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
                run.home.get_or_insert_with(|| PathBuf::from(cwd));
            }
            // A session start names the conversation (`/clear` starts another);
            // an agent that started before the daemon names it on its next hook.
            // A log where the daemon wrote it down itself (#150) keeps that.
            if !run.conversation_logged
                && let Some(id) = p["session_id"].as_str().filter(|c| !c.is_empty())
                && (p["hook_event_name"] == "SessionStart" || run.conversation.is_none())
            {
                run.conversation = Some(id.to_owned());
            }
            if p["hook_event_name"] == "UserPromptSubmit" {
                // A new turn: the last one's closing line isn't about it.
                run.finished = None;
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
            // Each turn end says where the work is now, branch and PR both.
            run.work = v["state"].as_str().map(str::to_owned);
            run.branch = text(&v["branch"]);
            run.pr = text(&v["pr"]);
            if let Some(pr) = &run.pr
                && !run.prs.contains(pr)
            {
                run.prs.push(pr.clone());
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

/// Longest line kept for search: a pasted log in a prompt is mostly noise.
const MAX_SAID: usize = 4000;

/// A prompt, a closing reply or a file written, as a line of history.
fn keep_said(run: &mut Run, v: &Value, since: u64, until: u64) {
    let t = v["t"].as_u64().unwrap_or(0);
    if v["k"] != "hook" || v["ignored"].as_bool() == Some(true) || t < since || t > until {
        return;
    }
    let p = &v["payload"];
    let (field, text) = match p["hook_event_name"].as_str().unwrap_or_default() {
        "UserPromptSubmit" => (SearchField::You, &p["prompt"]),
        "Stop" => (SearchField::Agent, &p["last_assistant_message"]),
        // Reading a file isn't working on it.
        "PostToolUse" if p["tool_name"] != "Read" => {
            let input = &p["tool_input"];
            let path = if input["file_path"].is_string() {
                &input["file_path"]
            } else {
                &input["notebook_path"]
            };
            (SearchField::File, path)
        }
        _ => return,
    };
    let Some(text) = text.as_str().map(crate::file::one_line) else {
        return;
    };
    if text.is_empty() {
        return;
    }
    let text = if text.chars().count() > MAX_SAID {
        text.chars().take(MAX_SAID).collect()
    } else {
        text
    };
    run.said.push(Said { t, field, text });
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
        Some((_, n)) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
            format!("PR #{n}")
        }
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
    // Back on a branch with nothing to ship after an earlier PR merged.
    let short = matches!(
        run.work.as_deref(),
        Some("uncommitted" | "unpushed" | "no-pr" | "pr-red")
    );
    if pr.is_none()
        && !short
        && let Some(done) = run.prs.iter().rev().find(|p| merged.contains(*p))
    {
        return (Outcome::Shipped, format!("{} merged", pr_label(done)));
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
                return (
                    Outcome::Stuck,
                    format!("needs you: {}", said("waiting on you")),
                );
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
        let on = pr
            .map(|p| format!(" on {}", pr_label(p)))
            .unwrap_or_default();
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
            return (
                Outcome::Abandoned,
                format!("idle {} with {short}", span(idle)),
            );
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
        p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
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

/// The repository a run worked in: its directory while that exists, else its
/// backlog item's.
pub fn repo_of(run: &Run, item: Option<&Item>, roots: &[PathBuf]) -> String {
    match (&run.cwd, item.and_then(|i| i.project.as_deref())) {
        (Some(c), _) if c.exists() => repo_name(Some(c), roots),
        (_, Some(project)) => repo_name(Some(project), roots),
        (c, None) => repo_name(c.as_deref(), roots),
    }
}

pub(crate) fn clip(text: &str, max: usize) -> String {
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
            let repo = repo_of(run, item, known.roots);
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

/// A day and a time, `Thu 12 Nov 14:03`.
pub fn when(ms: u64, tz: &TimeZone) -> String {
    zoned(ms, tz).map_or_else(|| "?".into(), |z| z.strftime("%a %-d %b %H:%M").to_string())
}

/// The digest as markdown to paste: a heading per repository or day, a line per
/// session, its agent's own last line under it.
pub fn render(digest: &Digest, tz: &TimeZone) -> String {
    let mut out = format!("# Away digest\n\n{}\n\n", about(digest, tz));
    if digest.entries.is_empty() {
        out.push_str("No agent did anything in that time.\n");
        return out;
    }
    out.push_str(&format!("{}.\n", tally(&digest.entries)));
    for (key, list) in grouped(digest, tz) {
        out.push_str(&format!("\n## {key}\n\n"));
        for e in list {
            out.push_str(&format!(
                "- **{}**: {}: {} ({})\n",
                e.outcome.label(),
                e.what,
                e.detail,
                at(e, digest.by, tz)
            ));
            if let Some(summary) = &e.summary {
                out.push_str(&format!("  > {summary}\n"));
            }
        }
    }
    out
}

/// Where the window starts: "Since you left, Tue 10 Nov 18:00 (14h ago)."
pub fn about(digest: &Digest, tz: &TimeZone) -> String {
    let window = span(digest.until_ms.saturating_sub(digest.since_ms));
    let from = when(digest.since_ms, tz);
    if digest.away {
        format!("Since you left, {from} ({window} ago).")
    } else {
        format!("Since {from} ({window} ago).")
    }
}

/// How many sessions ended up where: "5 agent sessions: 1 stuck, 4 shipped".
pub fn tally(entries: &[DigestEntry]) -> String {
    let counts: Vec<String> = Outcome::ALL
        .iter()
        .filter_map(|o| {
            let n = entries.iter().filter(|e| e.outcome == *o).count();
            (n > 0).then(|| format!("{n} {}", o.label()))
        })
        .collect();
    let n = entries.len();
    format!(
        "{n} agent session{}: {}",
        if n == 1 { "" } else { "s" },
        counts.join(", ")
    )
}

/// The entries under their headings, in the order they're shown: by repository
/// in the digest's own order, or by day, newest first.
pub fn grouped<'a>(digest: &'a Digest, tz: &TimeZone) -> Vec<(String, Vec<&'a DigestEntry>)> {
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
    groups
}

/// Who and when, for a line under `by`'s heading: the agent and its day and
/// times by repository; the agent, its repository and times by day.
pub fn at(e: &DigestEntry, by: DigestGroup, tz: &TimeZone) -> String {
    match by {
        DigestGroup::Repo => format!("{}, {}", e.agent, day_clock(e, tz)),
        DigestGroup::Day => format!("{} in {}, {}", e.agent, e.repo, clock_span(e, tz)),
    }
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
        let dir =
            std::env::temp_dir().join(format!("valkyrie-digest-{name}-{}", std::process::id()));
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
                hook(
                    NOON - H,
                    "/r/app",
                    "UserPromptSubmit",
                    Some("fix   the\nflaky test"),
                ),
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
        assert_eq!(run.command, vec!["fish".to_owned()]);
        // An old log names no start directory: its agent's first hook does.
        assert_eq!(run.home.as_deref(), Some(Path::new("/r/app")));

        let quiet = read_run(&path, NOON - 30 * 60_000, NOON).unwrap();
        assert_eq!(
            quiet.first_ms, None,
            "nothing happened in the last half hour"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_run_knows_where_and_on_what_conversation_to_resume() {
        let dir = tempdir("resume");
        let start_hook = |t: u64, id: &str| {
            json!({"k": "hook", "t": t, "off": 0, "agent": "claude", "ignored": false,
                "payload": {"cwd": "/r/app/sub", "hook_event_name": "SessionStart", "session_id": id}})
        };
        // An older log: the session start's hook names the conversation.
        let old = write_log(
            &dir,
            "1-1",
            &[
                json!({"k": "start", "t": NOON - H, "off": 0, "agent": "claude", "command": ["claude", "--model", "opus"]}),
                start_hook(NOON - H + 1, "first"),
                start_hook(NOON - H + 2, "after-clear"),
            ],
        );
        let run = read_run(&old, 0, NOON).unwrap();
        assert_eq!(run.conversation.as_deref(), Some("after-clear"));
        assert_eq!(run.home.as_deref(), Some(Path::new("/r/app/sub")));
        // A newer one: the daemon's own word wins over any hook's.
        let new = write_log(
            &dir,
            "1-2",
            &[
                json!({"k": "start", "t": NOON - H, "off": 0, "agent": "claude", "command": ["claude"],
                    "cwd": "/r/app-x", "name": "fixer", "repo": "/r/app", "worktree": "/r/app-x"}),
                json!({"k": "conversation", "t": NOON - H + 1, "off": 0, "id": "kept"}),
                start_hook(NOON - H + 2, "a codex exec's"),
            ],
        );
        let run = read_run(&new, 0, NOON).unwrap();
        assert_eq!(run.conversation.as_deref(), Some("kept"));
        assert_eq!(run.home.as_deref(), Some(Path::new("/r/app-x")));
        assert_eq!(run.name.as_deref(), Some("fixer"));
        assert_eq!(run.repo.as_deref(), Some(Path::new("/r/app")));
        assert_eq!(run.worktree.as_deref(), Some(Path::new("/r/app-x")));
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
    fn read_said_keeps_prompts_closing_replies_and_files_written_once_each() {
        let dir = tempdir("said");
        let tool = |t, name: &str, input: Value| {
            json!({"k": "hook", "t": t, "off": 0, "agent": "claude", "ignored": false,
                "payload": {"cwd": "/r/app", "hook_event_name": "PostToolUse",
                    "tool_name": name, "tool_input": input}})
        };
        let mut start = hook(NOON - 3 * H, "/r/app", "SessionStart", None);
        start["payload"]["session_id"] = "conv-1".into();
        let mut stop = hook(NOON - H, "/r/app", "Stop", None);
        stop["payload"]["last_assistant_message"] = "Zoom works.\n\nPR is up.".into();
        let mut ignored = hook(NOON - H, "/r/app", "UserPromptSubmit", Some("not ours"));
        ignored["ignored"] = true.into();
        let path = write_log(
            &dir,
            "1-9",
            &[
                json!({"k": "start", "t": NOON - 3 * H, "off": 0, "agent": "claude", "command": ["claude", "--model", "opus"]}),
                start,
                hook(
                    NOON - 3 * H,
                    "/r/app",
                    "UserPromptSubmit",
                    Some("before the window"),
                ),
                hook(
                    NOON - 2 * H,
                    "/r/app",
                    "UserPromptSubmit",
                    Some("add  a\nzoom key"),
                ),
                tool(
                    NOON - 2 * H + 1,
                    "Edit",
                    json!({"file_path": "/r/app/src/zoom.rs"}),
                ),
                tool(
                    NOON - 2 * H + 2,
                    "Read",
                    json!({"file_path": "/r/app/README.md"}),
                ),
                tool(
                    NOON - 2 * H + 3,
                    "Edit",
                    json!({"file_path": "/r/app/src/zoom.rs"}),
                ),
                tool(
                    NOON - 2 * H + 4,
                    "NotebookEdit",
                    json!({"notebook_path": "/r/app/n.ipynb"}),
                ),
                ignored,
                stop,
            ],
        );
        let run = read_said(&path, NOON - 2 * H - 5, NOON).unwrap();
        let said: Vec<(SearchField, &str, u64)> = run
            .said
            .iter()
            .map(|s| (s.field, s.text.as_str(), s.t))
            .collect();
        assert_eq!(
            said,
            vec![
                (SearchField::You, "add a zoom key", NOON - 2 * H),
                (SearchField::File, "/r/app/src/zoom.rs", NOON - 2 * H + 3),
                (SearchField::File, "/r/app/n.ipynb", NOON - 2 * H + 4),
                (SearchField::Agent, "Zoom works. PR is up.", NOON - H),
            ]
        );
        assert_eq!(run.conversation.as_deref(), Some("conv-1"));
        assert_eq!(run.command, ["claude", "--model", "opus"]);
        assert!(
            read_run(&path, NOON - 2 * H - 5, NOON)
                .unwrap()
                .said
                .is_empty(),
            "the digest keeps none"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_later_branch_without_a_pr_drops_the_earlier_pr() {
        let dir = tempdir("branch");
        let pr = "https://github.com/o/r/pull/177";
        let work = |t, branch: &str, pr: Option<&str>, state: &str| json!({"k": "work", "t": t, "off": 0, "state": state, "branch": branch, "pr": pr});
        let path = write_log(
            &dir,
            "1-8",
            &[
                json!({"k": "start", "t": NOON - 3 * H, "off": 0, "agent": "claude", "command": ["claude"]}),
                work(NOON - 2 * H, "feat/a", Some(pr), "clean"),
                state(NOON - 2 * H, "review_ready", Some("Opened PR #177."), true),
                hook(NOON - H, "/r/app", "UserPromptSubmit", Some("next one")),
                work(NOON - H + 5, "feat/b", None, "uncommitted"),
                json!({"k": "exit", "t": NOON - H + 9, "off": 0, "code": 0}),
            ],
        );
        let run = read_run(&path, NOON - 3 * H, NOON).unwrap();
        assert_eq!(run.pr, None);
        assert_eq!(run.branch.as_deref(), Some("feat/b"));
        assert_eq!(run.finished, None, "the new turn said nothing yet");
        let merged: HashSet<String> = [pr.to_owned()].into();
        assert_eq!(
            classify(&run, None, &merged, false, NOON),
            (Outcome::Abandoned, "ended with uncommitted changes".into())
        );
        // Back on the default branch with nothing left, the merged PR counts.
        let mut back = run.clone();
        back.work = Some("clean".into());
        assert_eq!(
            classify(&back, None, &merged, false, NOON),
            (Outcome::Shipped, "PR #177 merged".into())
        );
        std::fs::remove_dir_all(&dir).unwrap();
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
        assert_eq!(
            c(&run("working", None, None), None, &none, true).0,
            Outcome::Working
        );
        let mut asking = run("needs_input", None, None);
        asking.status_summary = Some("Permission: Bash rm -rf target".into());
        assert_eq!(
            c(&asking, None, &none, true),
            (
                Outcome::Stuck,
                "needs you: Permission: Bash rm -rf target".into()
            )
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
            c(
                &run("idle", Some("clean"), None),
                Some(&closed),
                &none,
                true
            )
            .0,
            Outcome::Abandoned
        );
        assert_eq!(
            c(
                &run("exited", Some("uncommitted"), None),
                None,
                &none,
                false
            ),
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
                entry(
                    Outcome::Waiting,
                    "valkyrie",
                    NOON - H,
                    Some("Opened PR #12."),
                ),
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
        assert!(
            md.contains("(claude, Mon 9 Nov 23:00 – Tue 10 Nov 11:00)"),
            "{md}"
        );
        digest.entries.clear();
        assert!(render(&digest, &TimeZone::UTC).ends_with("No agent did anything in that time.\n"));
    }

    #[test]
    fn presence_knows_when_you_left() {
        let mut p = Presence::default();
        assert_eq!(p.since(NOON), None);
        assert!(
            p.typed(NOON - 10 * H),
            "the first keystroke is written down"
        );
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
        assert_eq!(
            parse_since("2026-11-10 09:30", NOON, &tz).unwrap(),
            NOON - 150 * 60_000
        );
        assert_eq!(
            parse_since("2027-01-01", NOON, &tz).unwrap(),
            NOON,
            "never past now"
        );
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
        assert_eq!(
            pr_label("https://x/merge_requests/3"),
            "PR https://x/merge_requests/3"
        );
    }
}

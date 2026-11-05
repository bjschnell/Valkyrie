//! `valk pings`: where agents stopped and waited on you, read offline from the
//! sessions' event logs (DESIGN §7). Before a supervisor answers for you, this
//! shows which of those stops it could have answered.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use valkyrie_context::{project, skill};

/// Every kind, in the order the report lists them.
const KINDS: &[&str] = &[
    "question",
    "permission",
    "input",
    "screen",
    "bell",
    "blocked",
    "turn ended",
];

/// Words in a reply's last line that hand the next step back to you.
const OFFERS: &[&str] = &[
    "want me to",
    "should i ",
    "shall i ",
    "let me know",
    "tell me",
    "say the word",
    "if you want",
    "if you'd like",
    "whenever you say",
    "your call",
];

/// Topics, first match wins: `(name, words)`. A word matches at the start of a
/// word (`commit` in "committed"); a trailing space makes it whole (`pr `).
const TOPICS: &[(&str, &[&str])] = &[
    (
        "wait till you're done, clean up",
        &[
            "when you re done",
            "when you are done",
            "clean up",
            "cleanup",
            "tear down",
            "killed ",
        ],
    ),
    (
        "commit, push, PR or merge",
        &[
            "commit",
            "push",
            "pr ",
            "pull request",
            "merge",
            "ship",
            "release",
        ],
    ),
    ("pick an option", &["or ", "which", "option", "prefer"]),
    (
        "approve a plan",
        &[
            "plan",
            "approach",
            "direction",
            "look good",
            "sound good",
            "sound right",
        ],
    ),
    (
        "go on",
        &[
            "proceed",
            "continue",
            "go ahead",
            "keep going",
            "next",
            "start",
        ],
    ),
];

/// Examples shown per topic.
const EXAMPLES: usize = 3;

/// One time an agent stopped and waited on you.
pub struct Ping {
    pub kind: &'static str,
    /// The event log's stem, `<ns>-<id>`.
    pub session: String,
    pub t: u64,
    /// You were watching the session when it happened.
    pub seen: bool,
    pub summary: String,
    /// A turn end whose last line asks you something: that line.
    pub ask: Option<String>,
    /// Where a turn end's reply was read from: `hook`, `transcript`, `summary`, or
    /// `none` when nothing recorded one.
    pub source: Option<&'static str>,
    /// Where a turn end left its work (ADR-0013 §1): `clean`, `uncommitted`,
    /// `unpushed`, `no-pr` or `pr-red`; `None` before the daemon recorded it.
    pub work: Option<String>,
    cwd: Option<PathBuf>,
}

/// A Claude stop a `ship` policy kept going (ADR-0013 §5): the hook answered it,
/// so it never reached you. Not one of [`KINDS`].
pub const KEPT_GOING: &str = "kept going";

/// A step the supervisor took on a flagged session (ADR-0019 §6): `source` is
/// `policy` when it took it without asking, `you` when you tapped it. Not one of
/// [`KINDS`].
pub const SUPERVISED: &str = "supervisor step";

/// Work states that leave the next step to you, whatever the reply says.
const SHORT: &[&str] = &["uncommitted", "unpushed", "no-pr", "pr-red"];

impl Ping {
    /// A turn end that left work short of shipping.
    pub fn short(&self) -> bool {
        self.work.as_deref().is_some_and(|w| SHORT.contains(&w))
    }
}

/// The summary the tracker gives a turn still waiting on background work.
const BACKGROUND: &str = "background work still running";

/// The pings since `since_ms` in every event log in `dir`, oldest first; with
/// `repo`, only those whose agent ran in that project (any of its worktrees).
pub fn collect(dir: &Path, since_ms: u64, repo: Option<&Path>) -> Result<Vec<Ping>> {
    let mut pings = Vec::new();
    let mut transcripts = Transcripts::new();
    let entries = match std::fs::read_dir(dir) {
        // Nothing recorded yet.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(pings),
        entries => entries.with_context(|| format!("read {}", dir.display()))?,
    };
    for entry in entries {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if let Some(stem) = name.strip_suffix(".events.jsonl") {
            read_log(&path, stem, since_ms, &mut transcripts, &mut pings)?;
        }
    }
    if let Some(repo) = repo {
        let canonical =
            std::fs::canonicalize(repo).with_context(|| format!("--repo {}", repo.display()))?;
        let root = project::root(&canonical);
        let mut inside = HashMap::new();
        pings.retain(|p| {
            let Some(cwd) = &p.cwd else { return false };
            *inside
                .entry(cwd.clone())
                .or_insert_with(|| in_repo(cwd, repo, &root))
        });
    }
    pings.sort_by_key(|p| p.t);
    Ok(pings)
}

/// Whether `cwd` is in the project at `root`. A directory that's gone can't be
/// asked for its repository; a worktree Valkyrie made is named `<repo>-<branch>`
/// beside the root (DESIGN §4.3), so a gone one is matched by that name.
fn in_repo(cwd: &Path, repo: &Path, root: &Path) -> bool {
    if cwd.starts_with(repo) || cwd.starts_with(root) {
        return true;
    }
    if cwd.exists() {
        return project::root(cwd) == root;
    }
    // A gone cwd can't be canonicalized, so it may be spelled like either path
    // (macOS's /var is /private/var, and Windows canonical paths start \\?\).
    [root, repo].into_iter().any(|base| {
        let (Some(parent), Some(name)) = (base.parent(), base.file_name()) else {
            return false;
        };
        let prefix = format!("{}-", name.to_string_lossy());
        cwd.strip_prefix(parent)
            .ok()
            .and_then(|rest| rest.components().next())
            .is_some_and(|dir| dir.as_os_str().to_string_lossy().starts_with(&prefix))
    })
}

/// The last Stop hook seen before a state line.
struct Stop {
    message: Option<String>,
    transcript: Option<PathBuf>,
}

fn read_log(
    path: &Path,
    stem: &str,
    since_ms: u64,
    transcripts: &mut Transcripts,
    out: &mut Vec<Ping>,
) -> Result<()> {
    let bytes = match std::fs::read(path) {
        // Pruned while we looked.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        bytes => bytes.with_context(|| format!("read {}", path.display()))?,
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut seqs = HashSet::new();
    let mut cwd: Option<PathBuf> = None;
    let mut stop: Option<Stop> = None;
    // The last turn end's summary, while only background work followed it: the
    // tracker repeats that turn end once the work is done, under a new seq.
    let mut ended: Option<String> = None;
    let mut background = false;
    // The last ping this log added, when it's a turn end: the daemon writes where
    // that turn left its work a moment later, once git has been read.
    let mut last_end: Option<usize> = None;
    // A live session's last line may be half written.
    for line in text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        let t = line["t"].as_u64().unwrap_or(0);
        match line["k"].as_str().unwrap_or("") {
            "hook" => {
                let p = &line["payload"];
                if let Some(dir) = p["cwd"].as_str() {
                    cwd = Some(dir.into());
                }
                if p["hook_event_name"] == "Stop" && !p["valk_continued"].is_null() {
                    // A stop a policy answered for you (ADR-0013): not a turn end,
                    // but what the policy saved you.
                    if line["t"].as_u64().unwrap_or(0) >= since_ms {
                        last_end = None;
                        out.push(Ping {
                            kind: KEPT_GOING,
                            session: stem.to_owned(),
                            t: line["t"].as_u64().unwrap_or(0),
                            seen: false,
                            summary: String::new(),
                            ask: None,
                            source: None,
                            work: p["valk_continued"]["state"].as_str().map(str::to_owned),
                            cwd: cwd.clone(),
                        });
                    }
                } else if p["hook_event_name"] == "Stop" {
                    stop = Some(Stop {
                        message: p["last_assistant_message"].as_str().map(str::to_owned),
                        transcript: p["transcript_path"].as_str().map(PathBuf::from),
                    });
                }
            }
            // `leave` does nothing, so it's no step taken.
            "supervisor" if t >= since_ms && line["action"] != "leave" => {
                out.push(Ping {
                    kind: SUPERVISED,
                    session: stem.to_owned(),
                    t,
                    seen: true,
                    summary: line["action"].as_str().unwrap_or("").to_owned(),
                    ask: None,
                    source: Some(if line["auto"] == true {
                        "policy"
                    } else {
                        "you"
                    }),
                    work: None,
                    cwd: cwd.clone(),
                });
            }
            "state" => {
                let s = &line["status"];
                // The same transition is written again when only `seen` changes.
                if !s["seq"].as_u64().is_some_and(|seq| seqs.insert(seq)) {
                    continue;
                }
                let stop = stop.take();
                let state = s["state"].as_str();
                let summary = s["summary"].as_str().unwrap_or("").to_owned();
                if state == Some("working") && summary == BACKGROUND {
                    background = ended.is_some();
                    continue;
                }
                let repeat = state == Some("review_ready")
                    && background
                    && stop.is_none()
                    && ended.as_ref() == Some(&summary);
                ended = (state == Some("review_ready")).then(|| summary.clone());
                background = false;
                if repeat || t < since_ms || s["agent"].as_str().is_none_or(|a| a == "generic") {
                    continue;
                }
                let kind = match (state, s["ask"].as_str()) {
                    (Some("needs_input"), ask) => KINDS
                        .iter()
                        .find(|k| Some(**k) == ask)
                        .copied()
                        .unwrap_or("input"),
                    (Some("blocked"), _) => "blocked",
                    (Some("review_ready"), _) => "turn ended",
                    _ => continue,
                };
                let (mut ask, mut source) = (None, None);
                if kind == "turn ended" {
                    let reply = stop
                        .as_ref()
                        .and_then(|s| Some((s.message.clone()?, "hook")))
                        .or_else(|| {
                            let path = stop.as_ref()?.transcript.as_ref()?;
                            Some((transcripts.last_reply(path, t)?, "transcript"))
                        })
                        .or_else(|| (!summary.is_empty()).then(|| (summary.clone(), "summary")));
                    // A hookless turn end, seen only on the screen, has no reply at all.
                    let (reply, from) = reply.unwrap_or_default();
                    ask = asking(&reply);
                    source = Some(if from.is_empty() { "none" } else { from });
                }
                last_end = (kind == "turn ended").then_some(out.len());
                out.push(Ping {
                    kind,
                    session: stem.to_owned(),
                    t,
                    seen: s["seen"].as_bool().unwrap_or(false),
                    summary,
                    ask,
                    source,
                    work: None,
                    cwd: cwd.clone(),
                });
            }
            "work" => {
                if let Some(i) = last_end.take() {
                    out[i].work = line["state"].as_str().map(str::to_owned);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The agent's text replies in each Claude transcript, with their times, read
/// once per run.
struct Transcripts(HashMap<PathBuf, Vec<(Option<u64>, String)>>);

impl Transcripts {
    fn new() -> Self {
        Self(HashMap::new())
    }

    /// The last text reply up to `t_ms`.
    fn last_reply(&mut self, path: &Path, t_ms: u64) -> Option<String> {
        let replies = self
            .0
            .entry(path.to_path_buf())
            .or_insert_with(|| read_replies(path));
        let upto = replies.partition_point(|(at, _)| at.is_none_or(|at| at <= t_ms / 1000 + 1));
        replies[..upto].last().map(|(_, text)| text.clone())
    }
}

fn read_replies(path: &Path) -> Vec<(Option<u64>, String)> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let mut replies = Vec::new();
    for event in String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        // A subagent's replies are to its parent, not to you.
        if event["type"] != "assistant" || event["isSidechain"] == true {
            continue;
        }
        let at = event["timestamp"].as_str().and_then(skill::epoch);
        let blocks = event["message"]["content"].as_array();
        for b in blocks.into_iter().flatten().filter(|b| b["type"] == "text") {
            if let Some(text) = b["text"].as_str() {
                replies.push((at, text.to_owned()));
            }
        }
    }
    replies
}

/// The reply's last line, if it asks you something or offers you the next step.
fn asking(reply: &str) -> Option<String> {
    let line = reply
        .lines()
        .map(|l| l.trim_matches(|c: char| c.is_whitespace() || "*_#->".contains(c)))
        .rfind(|l| !l.is_empty() && !l.starts_with("```"))?;
    let lower = line.to_lowercase();
    (line.contains('?') || OFFERS.iter().any(|o| lower.contains(o))).then(|| line.replace("**", ""))
}

fn topic(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let padded = format!(" {} ", words.join(" "));
    TOPICS
        .iter()
        .find(|(_, words)| words.iter().any(|w| padded.contains(&format!(" {w}"))))
        .map_or("other", |(name, _)| name)
}

/// One line of at most 100 characters.
fn clip(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(100) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

/// `topic -> examples, newest first`, biggest topic first.
fn by_topic<'a>(texts: impl Iterator<Item = &'a str>) -> Vec<(&'static str, Vec<&'a str>)> {
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for text in texts {
        groups.entry(topic(text)).or_default().push(text);
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    for (_, texts) in &mut groups {
        texts.reverse();
    }
    groups.sort_by_key(|(_, texts)| std::cmp::Reverse(texts.len()));
    groups
}

/// Days the pings cover: the window, or less when the records start later.
fn span_days(pings: &[Ping], since_ms: u64, now_ms: u64) -> f64 {
    let from = pings.first().map_or(since_ms, |p| p.t.max(since_ms));
    (now_ms.saturating_sub(from) as f64 / 86_400_000.0).max(1.0)
}

pub fn report(pings: &[Ping], days: u64, since_ms: u64, now_ms: u64) -> Result<String> {
    let mut s = String::new();
    let sessions: HashSet<&str> = pings.iter().map(|p| p.session.as_str()).collect();
    let span = span_days(pings, since_ms, now_ms);
    writeln!(
        s,
        "Agent stops in the last {days} days ({span:.1} days of records), in {} sessions:",
        sessions.len()
    )?;
    writeln!(
        s,
        "{:<12} {:>6} {:>9} {:>8} {:>6}",
        "", "count", "sessions", "per day", "away"
    )?;
    for kind in KINDS {
        let of: Vec<&Ping> = pings.iter().filter(|p| p.kind == *kind).collect();
        let in_sessions: HashSet<&str> = of.iter().map(|p| p.session.as_str()).collect();
        let away = of.iter().filter(|p| !p.seen).count();
        writeln!(
            s,
            "{kind:<12} {:>6} {:>9} {:>8.1} {away:>6}",
            of.len(),
            in_sessions.len(),
            of.len() as f64 / span
        )?;
    }
    writeln!(
        s,
        "(away: you weren't watching the session when it stopped)"
    )?;

    let ends: Vec<&Ping> = pings.iter().filter(|p| p.kind == "turn ended").collect();
    let asks: Vec<&str> = ends.iter().filter_map(|p| p.ask.as_deref()).collect();
    let mut sources: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &ends {
        *sources.entry(p.source.unwrap_or("none")).or_default() += 1;
    }
    // Left short of shipping without asking: a statement ("I haven't pushed")
    // hands you the next step as surely as a question.
    let short = ends.iter().filter(|p| p.ask.is_none() && p.short()).count();
    let unknown = ends
        .iter()
        .filter(|p| p.ask.is_none() && !p.short() && p.source == Some("none"))
        .count();
    let sources: Vec<String> = sources.iter().map(|(k, n)| format!("{k} {n}")).collect();
    writeln!(s, "\nTurn ends: {}", ends.len())?;
    writeln!(s, "  ended asking you something  {}", asks.len())?;
    writeln!(s, "  left work short of shipping {short}")?;
    writeln!(
        s,
        "  just finished               {}",
        ends.len() - asks.len() - short - unknown
    )?;
    writeln!(s, "  no reply recorded           {unknown}")?;
    writeln!(s, "  (last reply read from: {})", sources.join(", "))?;
    let kept = pings.iter().filter(|p| p.kind == KEPT_GOING).count();
    if kept > 0 {
        writeln!(
            s,
            "  kept going by a policy      {kept} (not counted above)"
        )?;
    }
    let steps: Vec<&Ping> = pings.iter().filter(|p| p.kind == SUPERVISED).collect();
    if !steps.is_empty() {
        let auto = steps.iter().filter(|p| p.source == Some("policy")).count();
        writeln!(
            s,
            "  steps the supervisor took   {} ({auto} on its own by a policy, {} you tapped)",
            steps.len(),
            steps.len() - auto
        )?;
    }
    let recorded: Vec<&&Ping> = ends.iter().filter(|p| p.work.is_some()).collect();
    if !recorded.is_empty() {
        let mut by: BTreeMap<&str, usize> = BTreeMap::new();
        for p in &recorded {
            *by.entry(p.work.as_deref().unwrap_or("")).or_default() += 1;
        }
        let states: Vec<String> = ["uncommitted", "unpushed", "no-pr", "pr-red", "clean"]
            .iter()
            .map(|w| format!("{w} {}", by.get(w).copied().unwrap_or(0)))
            .collect();
        writeln!(
            s,
            "  (work at {} turn ends: {})",
            recorded.len(),
            states.join(", ")
        )?;
    }

    let questions = pings.iter().filter(|p| p.kind == "question");
    let questions: Vec<&str> = questions.map(|p| p.summary.as_str()).collect();
    writeln!(s, "\nTopics below are rough word matching.")?;
    for (title, texts) in [("Questions", questions), ("Asked in a reply", asks)] {
        writeln!(s, "\n{title}: {}", texts.len())?;
        for (topic, examples) in by_topic(texts.into_iter()) {
            writeln!(s, "  {topic}  {}", examples.len())?;
            for e in examples.iter().take(EXAMPLES) {
                writeln!(s, "    {}", clip(e))?;
            }
        }
    }
    Ok(s)
}

pub fn json(pings: &[Ping], days: u64, since_ms: u64, now_ms: u64) -> Value {
    let groups = |texts: Vec<&str>| -> Vec<Value> {
        by_topic(texts.into_iter())
            .into_iter()
            .map(|(topic, texts)| json!({"topic": topic, "count": texts.len(), "examples": texts}))
            .collect()
    };
    let questions = pings.iter().filter(|p| p.kind == "question");
    let asks = pings.iter().filter_map(|p| p.ask.as_deref());
    json!({
        "days": days,
        "span_days": span_days(pings, since_ms, now_ms),
        "questions": groups(questions.map(|p| p.summary.as_str()).collect()),
        "asks": groups(asks.collect()),
        "pings": pings.iter().map(|p| json!({
            "kind": p.kind,
            "session": p.session,
            "t": p.t,
            "seen": p.seen,
            "summary": p.summary,
            "ask": p.ask,
            "source": p.source,
            "work": p.work,
            "cwd": p.cwd,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A string goes in as it is, to write a broken line.
    fn write_log(dir: &Path, stem: &str, lines: &[Value]) {
        let log: String = lines
            .iter()
            .map(|l| match l {
                Value::String(raw) => format!("{raw}\n"),
                l => format!("{l}\n"),
            })
            .collect();
        std::fs::write(dir.join(format!("{stem}.events.jsonl")), log).unwrap();
    }

    fn state(t: u64, seq: u64, state: &str, ask: Option<&str>, summary: &str, seen: bool) -> Value {
        json!({"k": "state", "t": t, "status": {"agent": "claude", "state": state, "ask": ask,
               "summary": summary, "seq": seq, "seen": seen}})
    }

    fn stop(t: u64, cwd: &Path, message: Option<&str>, transcript: Option<&Path>) -> Value {
        let mut payload = json!({"hook_event_name": "Stop", "cwd": cwd});
        if let Some(m) = message {
            payload["last_assistant_message"] = json!(m);
        }
        if let Some(p) = transcript {
            payload["transcript_path"] = json!(p);
        }
        json!({"k": "hook", "t": t, "agent": "claude", "payload": payload})
    }

    #[test]
    fn counts_stops_and_reads_what_each_turn_ended_with() {
        let dir = std::env::temp_dir().join(format!("valkyrie-pings-{}", std::process::id()));
        let repo = dir.join("repo");
        let other = dir.join("other");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(&other).unwrap();
        // A transcript for the Stop hook that carries no message.
        let transcript = dir.join("t.jsonl");
        std::fs::write(
            &transcript,
            [
                json!({"type": "assistant", "timestamp": "1970-01-01T00:16:40Z",
                       "message": {"content": [{"type": "text", "text": "Done.\n\nShould I push it?"}]}}),
                json!({"type": "assistant", "timestamp": "1970-01-01T00:16:40Z", "isSidechain": true,
                       "message": {"content": [{"type": "text", "text": "A subagent's reply."}]}}),
                json!({"type": "assistant", "timestamp": "1970-01-01T05:00:00Z",
                       "message": {"content": [{"type": "text", "text": "A later turn."}]}}),
            ]
            .map(|l| format!("{l}\n"))
            .concat(),
        )
        .unwrap();
        write_log(
            &dir,
            "1-1",
            &[
                json!({"k": "start", "t": 100, "agent": "claude"}),
                json!({"k": "hook", "t": 900, "agent": "claude",
                       "payload": {"hook_event_name": "PreToolUse", "cwd": repo}}),
                state(
                    900,
                    1,
                    "needs_input",
                    Some("question"),
                    "Which loop first?",
                    false,
                ),
                // `seen` flipping writes the same transition again.
                state(
                    950,
                    1,
                    "needs_input",
                    Some("question"),
                    "Which loop first?",
                    true,
                ),
                stop(
                    1000,
                    &repo,
                    Some("Built it.\n\n**Want me** to commit and open the PR?"),
                    None,
                ),
                state(1000, 2, "review_ready", None, "Built it.", true),
                stop(1_000_000, &repo, None, Some(&transcript)),
                state(1_000_000, 3, "review_ready", None, "Done.", false),
                // Background work, then the tracker repeats the same turn end.
                state(1_000_010, 4, "working", None, BACKGROUND, false),
                state(1_000_020, 5, "review_ready", None, "Done.", false),
                json!("{not json"),
                state(1_000_100, 6, "working", None, "", true),
                state(1_000_200, 7, "review_ready", None, "All green.", false),
                state(1_000_210, 8, "working", None, "", true),
                // Seen only on the screen: no hook, no summary.
                state(1_000_220, 9, "review_ready", None, "", false),
                state(1_000_300, 10, "blocked", None, "exited with code 1", false),
                json!({"k": "state", "t": 1_000_400, "status": {"agent": "claude"}}),
            ],
        );
        write_log(
            &dir,
            "1-2",
            &[
                json!({"k": "hook", "t": 1, "agent": "claude",
                       "payload": {"hook_event_name": "PreToolUse", "cwd": other}}),
                state(
                    2,
                    1,
                    "needs_input",
                    Some("permission"),
                    "Bash: rm -rf x",
                    true,
                ),
                // Shells don't count, nor anything before the window.
                json!({"k": "state", "t": 2000, "status": {"agent": "generic", "state": "blocked",
                       "seq": 2}}),
            ],
        );

        let pings = collect(&dir, 0, None).unwrap();
        let kinds: Vec<&str> = pings.iter().map(|p| p.kind).collect();
        assert_eq!(
            kinds,
            [
                "permission",
                "question",
                "turn ended",
                "turn ended",
                "turn ended",
                "turn ended",
                "blocked"
            ]
        );
        assert!(!pings[1].seen, "seen at the transition, not later");
        let ends: Vec<(Option<&str>, Option<&str>)> = pings
            .iter()
            .filter(|p| p.kind == "turn ended")
            .map(|p| (p.ask.as_deref(), p.source))
            .collect();
        assert_eq!(
            ends,
            [
                (Some("Want me to commit and open the PR?"), Some("hook")),
                (Some("Should I push it?"), Some("transcript")),
                (None, Some("summary")),
                (None, Some("none")),
            ]
        );
        assert_eq!(collect(&dir, 1000, None).unwrap().len(), 5);
        let in_repo = collect(&dir, 0, Some(&repo)).unwrap();
        assert!(in_repo.iter().all(|p| p.session == "1-1"));
        assert_eq!(in_repo.len(), 6);

        let text = report(&pings, 30, 0, 86_400_000).unwrap();
        assert!(text.contains("in 2 sessions"), "{text}");
        assert!(text.contains("ended asking you something  2"), "{text}");
        assert!(text.contains("just finished               1"), "{text}");
        assert!(text.contains("no reply recorded           1"), "{text}");
        assert!(
            text.contains("hook 1, none 1, summary 1, transcript 1"),
            "{text}"
        );
        assert!(text.contains("commit, push, PR or merge  2"), "{text}");
        assert!(
            text.contains("pick an option  1\n    Which loop first?"),
            "{text}"
        );
        let v = json(&pings, 30, 0, 86_400_000);
        assert_eq!(v["pings"].as_array().unwrap().len(), 7);
        assert_eq!(v["asks"][0]["count"], 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn matches_deleted_worktrees_by_name_and_survives_missing_files() {
        let dir = std::env::temp_dir().join(format!("valkyrie-pings-gone-{}", std::process::id()));
        let repo = dir.join("valk");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let ask = |cwd: &Path| {
            [
                json!({"k": "hook", "t": 1, "agent": "claude",
                       "payload": {"hook_event_name": "PreToolUse", "cwd": cwd}}),
                state(1, 1, "needs_input", Some("question"), "Which?", false),
            ]
        };
        // None of these directories exist any more.
        write_log(&dir, "1-1", &ask(&dir.join("valk-fix-1-race/crates")));
        write_log(&dir, "1-2", &ask(&dir.join("valkyrie")));
        write_log(&dir, "1-3", &ask(&dir.join("other-valk")));
        // A Stop hook naming a transcript that's gone falls back to the summary.
        write_log(
            &dir,
            "1-4",
            &[
                stop(5, &repo, None, Some(&dir.join("gone.jsonl"))),
                state(5, 1, "review_ready", None, "Shall I merge it?", false),
            ],
        );
        let pings = collect(&dir, 0, Some(&repo)).unwrap();
        let sessions: Vec<&str> = pings.iter().map(|p| p.session.as_str()).collect();
        assert_eq!(sessions, ["1-1", "1-4"]);
        assert_eq!(pings[1].source, Some("summary"));
        assert_eq!(pings[1].ask.as_deref(), Some("Shall I merge it?"));

        assert!(collect(&dir, 0, Some(&dir.join("nowhere"))).is_err());
        let none = collect(&dir.join("sessions"), 0, None).unwrap();
        let text = report(&none, 30, 0, 86_400_000).unwrap();
        assert!(text.contains("in 0 sessions"), "{text}");
        assert!(text.contains("Questions: 0"), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_supervisors_steps_are_counted_apart_asked_or_not() {
        let dir = std::env::temp_dir().join(format!("valkyrie-pings-sup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let step = |t: u64, action: &str, auto: bool| {
            json!({"k": "supervisor", "t": t, "trigger": "conflicting",
                   "action": action, "offered": action, "auto": auto})
        };
        write_log(
            &dir,
            "3-1",
            &[
                step(1000, "merge-base", false),
                step(2000, "leave", false),
                step(3000, "merge-base", true),
                step(4000, "rerun", true),
            ],
        );
        let pings = collect(&dir, 0, None).unwrap();
        let steps: Vec<_> = pings.iter().filter(|p| p.kind == SUPERVISED).collect();
        assert_eq!(steps.len(), 3);
        let text = report(&pings, 30, 0, 86_400_000).unwrap();
        assert!(
            text.contains("steps the supervisor took   3 (2 on its own by a policy, 1 you tapped)"),
            "{text}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_turn_that_leaves_work_unshipped_is_counted_whatever_it_says() {
        let dir = std::env::temp_dir().join(format!("valkyrie-pings-work-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let work = |t: u64, state: &str| json!({"k": "work", "t": t, "state": state});
        write_log(
            &dir,
            "2-1",
            &[
                // A statement, not a question: only the work line shows it stalled.
                stop(
                    1000,
                    &dir,
                    Some("Committed as 6120299. I haven't pushed."),
                    None,
                ),
                state(1000, 1, "review_ready", None, "Committed.", false),
                work(1500, "unpushed"),
                // Asked, and short too: counted once, as asking.
                stop(2000, &dir, Some("Want me to open the PR?"), None),
                state(2000, 2, "review_ready", None, "Pushed.", false),
                work(2600, "no-pr"),
                stop(3000, &dir, Some("PR is up and green."), None),
                state(3000, 3, "review_ready", None, "PR up.", false),
                work(3100, "clean"),
                // A work line only ever belongs to the turn end right before it.
                state(
                    4000,
                    4,
                    "needs_input",
                    Some("question"),
                    "Which one?",
                    false,
                ),
                work(4100, "uncommitted"),
                // A stop the ship policy answered: never a turn end, counted apart.
                json!({"k": "hook", "t": 5000, "agent": "claude", "payload": {
                    "hook_event_name": "Stop", "cwd": dir,
                    "valk_continued": {"policy": 9, "state": "uncommitted", "until": "pr"}}}),
            ],
        );
        let pings = collect(&dir, 0, None).unwrap();
        let kept: Vec<_> = pings.iter().filter(|p| p.kind == KEPT_GOING).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].work.as_deref(), Some("uncommitted"));
        let pings: Vec<Ping> = pings.into_iter().filter(|p| p.kind != KEPT_GOING).collect();
        assert!(
            report(&collect(&dir, 0, None).unwrap(), 1, 0, 86_400_000)
                .unwrap()
                .contains("kept going by a policy      1")
        );
        let ends: Vec<_> = pings.iter().filter(|p| p.kind == "turn ended").collect();
        let works: Vec<_> = ends.iter().map(|p| p.work.as_deref()).collect();
        assert_eq!(works, [Some("unpushed"), Some("no-pr"), Some("clean")]);
        assert!(
            pings
                .iter()
                .all(|p| p.kind == "turn ended" || p.work.is_none())
        );
        let text = report(&pings, 1, 0, 86_400_000).unwrap();
        assert!(text.contains("ended asking you something  1"), "{text}");
        assert!(text.contains("left work short of shipping 1"), "{text}");
        assert!(text.contains("just finished               1"), "{text}");
        assert!(
            text.contains(
                "work at 3 turn ends: uncommitted 0, unpushed 1, no-pr 1, pr-red 0, clean 1"
            ),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_last_line_asks_when_it_questions_or_offers() {
        assert_eq!(
            asking("Done.\nSay the word and I'll push."),
            Some("Say the word and I'll push.".into())
        );
        assert_eq!(
            asking("Fixed the race? Yes.\n\nNothing is committed yet."),
            None
        );
        assert_eq!(asking("Which one?\n```\ncode\n```"), None);
        assert_eq!(topic("Where should skills live?"), "other");
        assert_eq!(
            topic("Want me to fast-forward main and push?"),
            "commit, push, PR or merge"
        );
        assert_eq!(topic("Does this approach look good?"), "approve a plan");
        assert_eq!(topic("Should I proceed?"), "go on");
        assert_eq!(
            topic("Tell me when you're done and I'll stop the servers. Nothing is pushed yet."),
            "wait till you're done, clean up"
        );
    }
}

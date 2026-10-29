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
    /// Where a turn end's reply was read from: `hook`, `transcript` or `summary`.
    pub source: Option<&'static str>,
    cwd: Option<PathBuf>,
}

/// The pings since `since_ms` in every event log in `dir`, oldest first; with
/// `repo`, only those whose agent ran in that project (any of its worktrees).
pub fn collect(dir: &Path, since_ms: u64, repo: Option<&Path>) -> Result<Vec<Ping>> {
    let mut pings = Vec::new();
    let entries = std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if let Some(stem) = name.strip_suffix(".events.jsonl") {
            read_log(&path, stem, since_ms, &mut pings)?;
        }
    }
    if let Some(repo) = repo {
        let repo = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
        let root = project::root(&repo);
        let mut inside = HashMap::new();
        pings.retain(|p| {
            let Some(cwd) = &p.cwd else { return false };
            *inside
                .entry(cwd.clone())
                .or_insert_with(|| cwd.starts_with(&repo) || project::root(cwd) == root)
        });
    }
    pings.sort_by_key(|p| p.t);
    Ok(pings)
}

/// The last Stop hook seen before a state line.
struct Stop {
    message: Option<String>,
    transcript: Option<PathBuf>,
}

fn read_log(path: &Path, stem: &str, since_ms: u64, out: &mut Vec<Ping>) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut seqs = HashSet::new();
    let mut cwd: Option<PathBuf> = None;
    let mut stop: Option<Stop> = None;
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
                if p["hook_event_name"] == "Stop" {
                    stop = Some(Stop {
                        message: p["last_assistant_message"].as_str().map(str::to_owned),
                        transcript: p["transcript_path"].as_str().map(PathBuf::from),
                    });
                }
            }
            "state" => {
                let s = &line["status"];
                // The same transition is written again when only `seen` changes.
                if !s["seq"].as_u64().is_some_and(|seq| seqs.insert(seq)) {
                    continue;
                }
                let stop = stop.take();
                if t < since_ms || s["agent"].as_str().is_none_or(|a| a == "generic") {
                    continue;
                }
                let kind = match (s["state"].as_str(), s["ask"].as_str()) {
                    (Some("needs_input"), ask) => KINDS
                        .iter()
                        .find(|k| Some(**k) == ask)
                        .copied()
                        .unwrap_or("input"),
                    (Some("blocked"), _) => "blocked",
                    (Some("review_ready"), _) => "turn ended",
                    _ => continue,
                };
                let summary = s["summary"].as_str().unwrap_or("").to_owned();
                let (mut ask, mut source) = (None, None);
                if kind == "turn ended" {
                    let (reply, from) = stop
                        .as_ref()
                        .and_then(|s| Some((s.message.clone()?, "hook")))
                        .or_else(|| {
                            let reply = last_reply(stop.as_ref()?.transcript.as_ref()?, t)?;
                            Some((reply, "transcript"))
                        })
                        .unwrap_or_else(|| (summary.clone(), "summary"));
                    ask = asking(&reply);
                    source = Some(from);
                }
                out.push(Ping {
                    kind,
                    session: stem.to_owned(),
                    t,
                    seen: s["seen"].as_bool().unwrap_or(false),
                    summary,
                    ask,
                    source,
                    cwd: cwd.clone(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

/// The agent's last text reply in a Claude transcript up to `t_ms`.
fn last_reply(transcript: &Path, t_ms: u64) -> Option<String> {
    let text = std::fs::read_to_string(transcript).ok()?;
    let mut last = None;
    for event in text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        if event["type"] != "assistant" {
            continue;
        }
        let at = event["timestamp"].as_str().and_then(skill::epoch);
        if at.is_some_and(|at| at > t_ms / 1000 + 1) {
            break;
        }
        let blocks = event["message"]["content"].as_array();
        for b in blocks.into_iter().flatten().filter(|b| b["type"] == "text") {
            last = b["text"].as_str().map(str::to_owned);
        }
    }
    last
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
        *sources.entry(p.source.unwrap_or("summary")).or_default() += 1;
    }
    let sources: Vec<String> = sources.iter().map(|(k, n)| format!("{k} {n}")).collect();
    writeln!(s, "\nTurn ends: {}", ends.len())?;
    writeln!(s, "  ended asking you something  {}", asks.len())?;
    writeln!(
        s,
        "  just finished               {}",
        ends.len() - asks.len()
    )?;
    writeln!(s, "  (last reply read from: {})", sources.join(", "))?;

    let questions = pings.iter().filter(|p| p.kind == "question");
    let questions: Vec<&str> = questions.map(|p| p.summary.as_str()).collect();
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
            "cwd": p.cwd,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_log(dir: &Path, stem: &str, lines: &[Value]) {
        let log: String = lines.iter().map(|l| format!("{l}\n")).collect();
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
        std::fs::create_dir_all(&other).unwrap();
        // A transcript for the Stop hook that carries no message.
        let transcript = dir.join("t.jsonl");
        std::fs::write(
            &transcript,
            [
                json!({"type": "assistant", "timestamp": "1970-01-01T00:16:40Z",
                       "message": {"content": [{"type": "text", "text": "Done.\n\nShould I push it?"}]}}),
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
                state(1_000_100, 4, "working", None, "", true),
                state(1_000_200, 5, "review_ready", None, "All green.", false),
                state(1_000_300, 6, "blocked", None, "exited with code 1", false),
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
            ]
        );
        assert_eq!(collect(&dir, 1000, None).unwrap().len(), 4);
        let in_repo = collect(&dir, 0, Some(&repo)).unwrap();
        assert!(in_repo.iter().all(|p| p.session == "1-1"));
        assert_eq!(in_repo.len(), 5);

        let text = report(&pings, 30, 0, 86_400_000).unwrap();
        assert!(text.contains("in 2 sessions"), "{text}");
        assert!(text.contains("ended asking you something  2"), "{text}");
        assert!(text.contains("hook 1, summary 1, transcript 1"), "{text}");
        assert!(text.contains("commit, push, PR or merge  2"), "{text}");
        assert!(
            text.contains("pick an option  1\n    Which loop first?"),
            "{text}"
        );
        let v = json(&pings, 30, 0, 86_400_000);
        assert_eq!(v["pings"].as_array().unwrap().len(), 6);
        assert_eq!(v["asks"][0]["count"], 2);
        std::fs::remove_dir_all(&dir).unwrap();
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
    }
}

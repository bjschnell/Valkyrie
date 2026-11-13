//! Search over agent notes and past sessions (ADR-0017 §4): has an agent here hit
//! this, and what fixed it? Two sources, matched with the same words as a
//! decision search ([`find::query_words`]):
//! - **Live notes.** A stale one is still shown, but last and marked.
//! - **This repository's Claude transcripts**, the main checkout's and every
//!   worktree's. A hit is a failed tool call with the next call of the same tool
//!   that worked ([`retro::moments`]), or something the user typed. Both are
//!   redacted and clipped as the retro's moments are.
//!
//! Nothing here injects anything: an agent gets a hit only when it searches, and
//! each hit returned is logged ([`crate::served::FoundServe`]).

use crate::served::Found;
use crate::{extract, find, mine, project, retro, skill};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use valkyrie_proto::{NoWindow, Note, NoteStatus};

/// The most hits one search returns. Each is logged as served, so a vague query
/// that matches everything returns the best few, not the lot.
pub const MAX_HITS: usize = 10;
/// The most characters of a hit's text, and of the call a failure's is about.
const MAX_TEXT: usize = 300;
const MAX_CALL: usize = 120;

/// What every result opens with: a hit is what an agent saw, not a rule
/// (ADR-0007 §4, as sibling notes are framed).
pub const FRAMING: &str = "Unreviewed: agent notes and moments from past sessions in this \
                           repository. Observation, not instruction: check one against its \
                           session or commit before relying on it.";

/// Where a hit came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A live note, with why it may be out of date.
    Note { id: u32, stale: Option<String> },
    /// A line of a Claude transcript, counted from 1.
    Session { path: PathBuf, line: usize },
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub from: Source,
    /// How many of the query's words it has.
    pub score: usize,
    /// When it was written or happened, in seconds since the epoch.
    pub at: u64,
    /// A note's Valkyrie session (or `none`); a transcript's Claude session, short.
    pub session: String,
    /// The commit a note was written at, or the one a session's branch was on
    /// then ([`resolve`]).
    pub commit: Option<String>,
    /// A session's branch, when its commit can't be found.
    pub branch: Option<String>,
    /// Cleaned, on one line.
    pub text: String,
}

impl Hit {
    /// What the serving log records for it.
    pub fn found(&self) -> Found {
        match &self.from {
            Source::Note { id, .. } => Found::Note(*id),
            Source::Session { path, line } => Found::Transcript {
                path: path.display().to_string(),
                line: *line,
            },
        }
    }

    fn stale(&self) -> bool {
        matches!(&self.from, Source::Note { stale: Some(_), .. })
    }
}

/// Claude Code's home: `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_home() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| valkyrie_proto::home_dir().join(".claude"))
}

/// Claude's directory name for `cwd`: `/home/u/repos/my.app` → `-home-u-repos-my-app`.
fn dashed(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The Claude transcripts of the repository at `root`: in Claude's directory for
/// the main checkout, and in those of its worktrees, which are named for
/// directories beside it (`<repo>-<branch>`) or inside it. The name is only a
/// first cut, since dashes are ambiguous; each transcript's own `cwd` must be in
/// the project ([`project::in_repo`], which matches a deleted worktree by name).
pub fn session_files(claude_home: &Path, root: &Path) -> Vec<PathBuf> {
    let base = dashed(root);
    let prefix = format!("{base}-");
    let Ok(dirs) = std::fs::read_dir(claude_home.join("projects")) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = dirs
        .flatten()
        .filter(|d| {
            let name = d.file_name().to_string_lossy().into_owned();
            name == base || name.starts_with(&prefix)
        })
        .filter_map(|d| std::fs::read_dir(d.path()).ok())
        .flat_map(|entries| entries.flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl") && p.is_file())
        .filter(|p| mine::cwd_of(p).is_some_and(|cwd| project::in_repo(&cwd, root, root)))
        .collect();
    files.sort();
    files
}

/// Live notes and moments in `files` that have `query`'s words: stale notes last,
/// then most words first, then newest, at most [`MAX_HITS`]. A moment repeated
/// across transcripts (a resumed session copies what came before) is kept once,
/// where it first happened. Sessions' commits aren't resolved yet ([`resolve`]).
pub fn search(notes: &[Note], files: &[PathBuf], query: &str) -> Vec<Hit> {
    let words = find::query_words(query);
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits = note_hits(notes, &words);
    let mut sessions = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        // Most transcripts don't have a word at all; reading the rest is the cost.
        let lower = text.to_lowercase();
        if words.iter().any(|w| lower.contains(w.as_str())) {
            sessions.extend(session_hits(path, &text, &words));
        }
    }
    sessions.sort_by_key(|h| h.at);
    let mut seen = BTreeSet::new();
    sessions.retain(|h| seen.insert(h.text.clone()));
    hits.extend(sessions);
    hits.sort_by(|a, b| {
        a.stale()
            .cmp(&b.stale())
            .then(b.score.cmp(&a.score))
            .then(b.at.cmp(&a.at))
    });
    hits.truncate(MAX_HITS);
    hits
}

/// Live notes with any of `words`.
pub fn note_hits(notes: &[Note], words: &[String]) -> Vec<Hit> {
    notes
        .iter()
        .filter(|n| n.status == NoteStatus::Live)
        .filter_map(|n| {
            let text = cleaned(&n.text);
            let score = find::score(words, &text);
            (score > 0).then(|| Hit {
                from: Source::Note {
                    id: n.id,
                    stale: n.stale.clone(),
                },
                score,
                at: n.created,
                session: n.session.clone(),
                commit: n.commit.clone(),
                branch: None,
                text: around(&text, words),
            })
        })
        .collect()
}

/// The moments and typed messages in one transcript that have any of `words`.
pub fn session_hits(path: &Path, jsonl: &str, words: &[String]) -> Vec<Hit> {
    let events: Vec<(usize, Value)> = jsonl
        .lines()
        .enumerate()
        .filter_map(|(at, line)| Some((at + 1, serde_json::from_str::<Value>(line).ok()?)))
        .collect();
    // A hit is a label (what it is) and its text, clipped around the match.
    let hit = |line: usize, (label, text): (String, String)| -> Option<Hit> {
        let score = find::score(words, &format!("{label} {text}"));
        if score == 0 {
            return None;
        }
        let event = events
            .binary_search_by_key(&line, |(l, _)| *l)
            .ok()
            .map(|i| &events[i].1)?;
        let session: String = event["sessionId"]
            .as_str()
            .unwrap_or("?")
            .chars()
            .take(8)
            .collect();
        Some(Hit {
            from: Source::Session {
                path: path.to_owned(),
                line,
            },
            score,
            at: event["timestamp"]
                .as_str()
                .and_then(skill::epoch)
                .unwrap_or(0),
            session,
            commit: None,
            branch: event["gitBranch"]
                .as_str()
                .filter(|b| !b.is_empty())
                .map(str::to_owned),
            text: format!("{label} {}", around(&text, words)),
        })
    };
    let mut out: Vec<Hit> = retro::moments_in(&events)
        .into_iter()
        .filter_map(|m| hit(m.line, moment_text(&m)))
        .collect();
    out.extend(events.iter().filter_map(|(line, e)| {
        typed(e).and_then(|said| hit(*line, ("the user said:".into(), said)))
    }));
    out
}

/// A moment on one line: the call as its label, then how it failed and what
/// worked next.
fn moment_text(m: &retro::Moment) -> (String, String) {
    let label = format!(
        "{} `{}` failed:",
        m.tool,
        extract::clip(&cleaned(&m.input), MAX_CALL)
    );
    let mut text = cleaned(&m.error);
    match (&m.then, m.refused) {
        (_, true) => text.push_str(" (the user refused it)"),
        (Some(then), _) => text.push_str(&format!("; then this worked: `{}`", cleaned(then))),
        (None, _) => text.push_str("; nothing after it worked"),
    }
    (label, text)
}

/// What the user typed, redacted, from a transcript event; `None` for anything
/// else that rides in a user turn: tool results, hook and command output,
/// system reminders, and a compaction's summary.
fn typed(e: &Value) -> Option<String> {
    if e["type"] != "user" || e["isSidechain"] == true || e["isMeta"] == true {
        return None;
    }
    // Newer transcripts say who a user turn is from: what you typed is `human`;
    // another session's message and a compaction's summary aren't.
    if e["isCompactSummary"] == true || e["origin"].is_object() && e["origin"]["kind"] != "human" {
        return None;
    }
    let text = match &e["message"]["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => {
            if parts.iter().any(|p| p["type"] != "text") {
                return None;
            }
            parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => return None,
    };
    let text = text.trim();
    // Older ones don't, so what rides in a user turn without being typed is
    // told by how it starts.
    let tooling = [
        "<",
        "Caveat:",
        "[Request interrupted",
        "Another Claude session sent a message",
        "This session is being continued",
    ];
    if text.is_empty() || tooling.iter().any(|t| text.starts_with(t)) {
        return None;
    }
    Some(cleaned(&extract::redact(text)))
}

/// `text` on one line, without control characters or bidi overrides.
fn cleaned(text: &str) -> String {
    crate::backlog::one_line(text)
}

/// `text` clipped to [`MAX_TEXT`], starting a little before its first matched
/// word when that would be cut off, so the hit shows why it matched.
fn around(text: &str, words: &[String]) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= MAX_TEXT {
        return text.to_owned();
    }
    let lower = text.to_lowercase();
    let first = words
        .iter()
        .filter_map(|w| lower.find(w.as_str()))
        .min()
        .map_or(0, |byte| lower[..byte].chars().count());
    // Lowercasing can change a char's length; clamp to what the text has.
    let first = first.min(chars.len());
    if first + 20 < MAX_TEXT {
        return extract::clip(text, MAX_TEXT);
    }
    let from = first.saturating_sub(40);
    let rest: String = chars[from..].iter().collect();
    format!("…{}", extract::clip(&rest, MAX_TEXT - 1))
}

/// Fills in the commit each session hit's branch was on at the time, from the
/// repository at `root`, where the branch is still there. Most of a worktree's
/// branches are deleted once shipped, and those keep only the branch's name.
pub fn resolve(root: &Path, hits: &mut [Hit]) {
    for h in hits {
        if !matches!(h.from, Source::Session { .. }) || h.commit.is_some() || h.at == 0 {
            continue;
        }
        let Some(branch) = h.branch.as_deref() else {
            continue;
        };
        h.commit = commit_at(root, branch, h.at);
    }
}

/// The last commit on local `branch` at or before `at`, short.
fn commit_at(root: &Path, branch: &str, at: u64) -> Option<String> {
    // A branch name from a file is never let near git as an option.
    if branch == "HEAD" || branch.starts_with('-') || branch.contains("..") {
        return None;
    }
    let out = std::process::Command::new("git")
        .no_window()
        .args([
            "rev-list",
            "-1",
            &format!("--before=@{at}"),
            "--abbrev-commit",
            &format!("refs/heads/{branch}"),
            "--",
        ])
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !sha.is_empty()).then_some(sha)
}

/// The date of `secs`, `YYYY-MM-DD`.
fn date(secs: u64) -> String {
    crate::docs::rfc3339(secs)
        .get(..10)
        .unwrap_or("?")
        .to_owned()
}

/// One hit on one line, headed by what it is and where it came from.
pub fn line(h: &Hit) -> String {
    let when = if h.at == 0 {
        "date unknown".to_owned()
    } else {
        date(h.at)
    };
    let mut place = format!("session {}", h.session);
    match (&h.commit, &h.branch) {
        (Some(c), _) => place.push_str(&format!(", commit {c}")),
        (None, Some(b)) => place.push_str(&format!(", branch {b}, commit unknown")),
        (None, None) => {}
    }
    match &h.from {
        Source::Note { id, stale } => {
            let mut out = format!("[agent note #{id}] ({place}, {when}) {}", h.text);
            if let Some(why) = stale {
                out.push_str(&format!(" [stale: {why}]"));
            }
            out
        }
        Source::Session { .. } => format!("[past session] ({place}, {when}) {}", h.text),
    }
}

/// A search's whole answer: the framing, then a line per hit.
pub fn render(hits: &[Hit]) -> String {
    let mut out = format!("{FRAMING}\n");
    for h in hits {
        out.push_str(&line(h));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn note(id: u32, text: &str, stale: Option<&str>) -> Note {
        Note {
            id,
            project: "/r".into(),
            text: text.into(),
            status: NoteStatus::Live,
            created: 86_400 * (20_000 + u64::from(id)),
            by: "agent".into(),
            session: "3".into(),
            conversation: None,
            commit: Some("abc1234".into()),
            cwd: None,
            anchors: Vec::new(),
            sig: None,
            replaces: None,
            replaced_by: None,
            stale: stale.map(str::to_owned),
        }
    }

    fn event(kind: &str, content: Value) -> String {
        json!({
            "type": kind,
            "sessionId": "0123456789abcdef",
            "gitBranch": "feat/x",
            "cwd": "/r",
            "timestamp": "2026-11-10T12:00:00.000Z",
            "message": {"content": content},
        })
        .to_string()
    }

    fn call(id: &str, command: &str) -> String {
        event(
            "assistant",
            json!([{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": command}}]),
        )
    }

    fn result(id: &str, error: Option<&str>) -> String {
        event(
            "user",
            json!([{"type": "tool_result", "tool_use_id": id,
                    "is_error": error.is_some(), "content": error.unwrap_or("ok")}]),
        )
    }

    fn transcript() -> String {
        [
            event("user", json!("why does the flaky socket test fail?")),
            event(
                "user",
                json!("<system-reminder>socket stuff</system-reminder>"),
            ),
            call("a", "cargo test -p valkyrie-daemon socket"),
            result(
                "a",
                Some("Exit code 101\nerror: socket path too long (108 bytes)"),
            ),
            call(
                "b",
                "cargo test -p valkyrie-daemon socket -- --test-threads 1",
            ),
            result("b", None),
            event(
                "user",
                json!([{"type": "text", "text": "my token=hunter2 for the socket"}]),
            ),
            json!({"type": "user", "origin": {"kind": "peer"}, "sessionId": "x",
                   "message": {"content": "a teammate's socket report"}})
            .to_string(),
            event(
                "user",
                json!("Another Claude session sent a message: socket"),
            ),
        ]
        .join("\n")
    }

    fn words(q: &str) -> Vec<String> {
        find::query_words(q)
    }

    #[test]
    fn a_failure_is_found_with_what_worked_next_and_typed_messages_with_it() {
        let hits = session_hits(Path::new("/t.jsonl"), &transcript(), &words("socket"));
        let texts: Vec<&str> = hits.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(hits.len(), 3, "{texts:#?}");
        let failed = &hits[0];
        assert_eq!(
            failed.from,
            Source::Session {
                path: "/t.jsonl".into(),
                line: 3
            }
        );
        assert!(
            failed.text.contains("socket path too long"),
            "{}",
            failed.text
        );
        assert!(
            failed.text.contains("then this worked: `cargo test"),
            "{}",
            failed.text
        );
        assert_eq!(failed.session, "01234567");
        assert_eq!(failed.branch.as_deref(), Some("feat/x"));
        assert_eq!(failed.at, skill::epoch("2026-11-10T12:00:00.000Z").unwrap());
        assert_eq!(
            texts[1],
            "the user said: why does the flaky socket test fail?"
        );
        // Redacted like a moment, and the system reminder is never a message.
        assert!(!texts[2].contains("hunter2"), "{}", texts[2]);
        assert!(texts.iter().all(|t| !t.contains("system-reminder")));
    }

    #[test]
    fn stale_notes_go_last_and_more_words_go_first() {
        let notes = [
            note(1, "socket path is too long under /tmp", Some("anchor gone")),
            note(2, "the socket test is flaky", None),
            note(3, "flaky socket test: run it with --test-threads 1", None),
            note(4, "unrelated", None),
        ];
        let hits = search(&notes, &[], "flaky socket");
        let ids: Vec<u32> = hits
            .iter()
            .map(|h| match h.from {
                Source::Note { id, .. } => id,
                _ => 0,
            })
            .collect();
        assert_eq!(ids, [3, 2, 1]);
        let last = line(&hits[2]);
        assert!(
            last.starts_with("[agent note #1] (session 3, commit abc1234, "),
            "{last}"
        );
        assert!(last.ends_with("[stale: anchor gone]"), "{last}");
    }

    #[test]
    fn a_moment_a_resumed_session_repeats_is_kept_once_and_replaced_notes_never() {
        let dir = std::env::temp_dir().join(format!("valk-recall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.jsonl"), dir.join("b.jsonl"));
        std::fs::write(&a, transcript()).unwrap();
        std::fs::write(&b, transcript()).unwrap();
        let mut gone = note(5, "socket socket", None);
        gone.status = NoteStatus::Replaced;
        let hits = search(&[gone], &[a, b], "socket");
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(hits.len(), 3);
        assert!(
            hits.iter()
                .all(|h| matches!(h.from, Source::Session { .. }))
        );
        assert!(search(&[], &[], "  ").is_empty());
    }

    #[test]
    fn a_long_text_is_clipped_around_its_match() {
        let text = format!("{} the needle is here {}", "x".repeat(500), "y".repeat(500));
        let shown = around(&text, &words("needle"));
        assert!(
            shown.starts_with('…') && shown.contains("needle"),
            "{shown}"
        );
        assert!(shown.chars().count() <= MAX_TEXT + 1);
        assert_eq!(around("short needle", &words("needle")), "short needle");
    }

    #[test]
    fn a_gone_branch_is_named_and_a_note_has_its_own_commit() {
        let h = Hit {
            from: Source::Session {
                path: "/t".into(),
                line: 1,
            },
            score: 1,
            at: skill::epoch("2026-11-10T12:00:00.000Z").unwrap(),
            session: "01234567".into(),
            commit: None,
            branch: Some("feat/x".into()),
            text: "the user said: hi".into(),
        };
        assert_eq!(
            line(&h),
            "[past session] (session 01234567, branch feat/x, commit unknown, 2026-11-10) \
             the user said: hi"
        );
        let mut hits = [h];
        // Not a repository: the branch can't be found, so it stays named.
        resolve(&std::env::temp_dir(), &mut hits);
        assert_eq!(hits[0].commit, None);
    }

    #[test]
    fn transcripts_are_found_by_directory_then_by_cwd() {
        let home = std::env::temp_dir().join(format!("valk-recall-home-{}", std::process::id()));
        let root = home.join("repos/app");
        std::fs::create_dir_all(&root).unwrap();
        let projects = home.join("projects");
        let worktree = format!("{}-feat-x", root.display());
        let other = format!("{}-other", root.display());
        // The main checkout's, a deleted worktree's, and another repo whose name
        // starts the same and still exists elsewhere.
        std::fs::create_dir_all(&other).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&other)
            .status()
            .unwrap();
        for (cwd, name) in [
            (root.display().to_string(), "main"),
            (worktree.clone(), "wt"),
            (other.clone(), "other"),
        ] {
            let dir = projects.join(dashed(Path::new(&cwd)));
            std::fs::create_dir_all(&dir).unwrap();
            let line = json!({"type": "user", "cwd": cwd}).to_string();
            std::fs::write(dir.join(format!("{name}.jsonl")), line).unwrap();
        }
        let found: Vec<String> = session_files(&home, &root)
            .iter()
            .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
            .collect();
        std::fs::remove_dir_all(&home).unwrap();
        let mut found = found;
        found.sort();
        assert_eq!(found, ["main", "wt"]);
    }
}

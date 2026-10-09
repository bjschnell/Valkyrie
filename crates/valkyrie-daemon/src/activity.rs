//! What each agent is doing, for the agents beside it (DESIGN §6.5): the files it
//! edited lately and what you last asked it, read from its hooks. An agent asking
//! (its context hook, on each prompt) is told about the other agents in the same
//! repository, so two of them don't edit the same file at once.

use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Edits kept per session.
const MAX_EDITS: usize = 32;
/// How long an edit or a prompt counts as "right now".
pub const RECENT_MS: u64 = 20 * 60 * 1000;
/// Files named per sibling.
const MAX_FILES: usize = 5;

#[derive(Debug, Default, Clone)]
pub struct Activity {
    /// Files edited, newest last, with when (ms).
    edits: VecDeque<(PathBuf, u64)>,
    /// What you last asked, and when.
    prompt: Option<(String, u64)>,
    /// What this session's agent was last told, so it isn't told again.
    told: Option<u64>,
}

impl Activity {
    /// Takes what a hook payload says: edited files (after the tool ran) and your
    /// prompt. A session start forgets what the agent was told: it may have lost it.
    pub fn note(&mut self, payload: &Value, now: u64) {
        match payload["hook_event_name"].as_str() {
            Some("PostToolUse") => {
                let cwd = payload["cwd"].as_str().map(Path::new);
                for path in edited(&payload["tool_input"]) {
                    let path = match cwd {
                        Some(cwd) if path.is_relative() => cwd.join(path),
                        _ => path,
                    };
                    self.edits.retain(|(p, _)| *p != path);
                    self.edits.push_back((path, now));
                    if self.edits.len() > MAX_EDITS {
                        self.edits.pop_front();
                    }
                }
            }
            Some("UserPromptSubmit") => {
                if let Some(prompt) = payload["prompt"].as_str().and_then(first_line) {
                    self.prompt = Some((prompt, now));
                }
            }
            Some("SessionStart") => self.told = None,
            _ => {}
        }
    }

    /// Files edited since `since`, newest first.
    pub fn edited_since(&self, since: u64) -> Vec<&Path> {
        self.edits
            .iter()
            .rev()
            .filter(|(_, at)| *at >= since)
            .map(|(p, _)| p.as_path())
            .collect()
    }

    fn last_edit(&self) -> Option<u64> {
        self.edits.back().map(|(_, at)| *at)
    }

    /// Whether `text` is news to this agent; remembers it either way.
    pub fn tell(&mut self, text: &str) -> bool {
        let mark = text.bytes().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
        });
        self.told.replace(mark) != Some(mark)
    }
}

/// The paths a tool call edits: Claude's `file_path`/`notebook_path`, a `path`,
/// or the files a patch names (`*** Update File: …`, Codex's `apply_patch`).
fn edited(input: &Value) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = ["file_path", "notebook_path", "path"]
        .iter()
        .filter_map(|k| input[k].as_str())
        .map(PathBuf::from)
        .collect();
    for text in ["command", "patch", "input"]
        .iter()
        .filter_map(|k| input[k].as_str())
    {
        for line in text.lines() {
            for head in ["*** Update File: ", "*** Add File: ", "*** Delete File: "] {
                if let Some(path) = line.strip_prefix(head) {
                    out.push(PathBuf::from(path.trim()));
                }
            }
        }
    }
    out
}

fn first_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    // Tags and relays aren't what you asked.
    if line.starts_with('<') {
        return None;
    }
    Some(if line.chars().count() > 100 {
        line.chars().take(100).collect::<String>() + "…"
    } else {
        line.to_owned()
    })
}

/// Another session, as the asking agent hears about it.
pub struct Sibling<'a> {
    pub name: &'a str,
    pub agent: &'a str,
    /// Its state's label (`working`, `needs input`).
    pub state: &'a str,
    /// Its checkout, when it isn't the asker's (another worktree).
    pub checkout: Option<&'a Path>,
    pub activity: &'a Activity,
}

/// What to tell an agent about the agents beside it, or `None` when there's
/// nothing recent to say. `mine` is the asker's own activity, for overlaps;
/// paths read relative to `root`.
pub fn tell(root: &Path, mine: &Activity, siblings: &[Sibling], now: u64) -> Option<String> {
    let since = now.saturating_sub(RECENT_MS);
    // Below the repo, or below the sibling's own checkout (another worktree).
    let rel = |p: &Path, checkout: Option<&Path>| {
        p.strip_prefix(root)
            .ok()
            .or_else(|| p.strip_prefix(checkout?).ok())
            .map(|r| r.display().to_string())
            .unwrap_or_else(|| p.display().to_string())
    };
    let my_files = mine.edited_since(since);
    let mut lines = Vec::new();
    let mut overlaps = Vec::new();
    for s in siblings {
        let a = s.activity;
        let busy = s.state == "working"
            || a.last_edit().is_some_and(|t| t >= since)
            || a.prompt.as_ref().is_some_and(|(_, t)| *t >= since);
        if !busy {
            continue;
        }
        let mut line = format!("- {} ({}, {}", s.name, s.agent, s.state);
        if let Some(checkout) = s.checkout {
            line.push_str(&format!(", in {}", checkout.display()));
        }
        line.push(')');
        if let Some((prompt, at)) = &a.prompt
            && *at >= since
        {
            line.push_str(&format!(": asked “{prompt}”"));
        }
        let files = a.edited_since(since);
        if !files.is_empty() {
            let shown: Vec<String> = files
                .iter()
                .take(MAX_FILES)
                .map(|p| rel(p, s.checkout))
                .collect();
            line.push_str(&format!("; editing {}", shown.join(", ")));
            if files.len() > MAX_FILES {
                line.push_str(&format!(" and {} more", files.len() - MAX_FILES));
            }
        }
        for f in &files {
            // The same file, or the same path in another worktree of the repo.
            let same = my_files
                .iter()
                .any(|m| m == f || same_in_worktrees(m, f, root, s.checkout));
            if same {
                overlaps.push(format!("{} ({})", rel(f, s.checkout), s.name));
            }
        }
        lines.push(line);
    }
    if lines.is_empty() {
        return None;
    }
    let mut out =
        String::from("Other agents are working in this repository right now (from Valkyrie):\n");
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    if !overlaps.is_empty() {
        out.push_str(&format!(
            "You have both edited: {}. Before changing these again, check what they did, \
             and tell the user if your changes conflict.\n",
            overlaps.join(", ")
        ));
    } else {
        out.push_str("Avoid editing the files they're editing; tell the user if you need to.\n");
    }
    Some(out)
}

/// `a` and `b` are the same file in two checkouts of the repo: equal paths below
/// their checkouts. Only checked when the sibling is in another checkout.
fn same_in_worktrees(a: &Path, b: &Path, root: &Path, theirs: Option<&Path>) -> bool {
    let Some(theirs) = theirs else {
        return false;
    };
    match (a.strip_prefix(root), b.strip_prefix(theirs)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MIN: u64 = 60_000;

    fn edit(a: &mut Activity, path: &str, at: u64) {
        a.note(
            &json!({"hook_event_name":"PostToolUse","cwd":"/r","tool_name":"Edit",
                    "tool_input":{"file_path": path}}),
            at,
        );
    }

    #[test]
    fn notes_edits_from_either_agent_and_your_prompts() {
        let mut a = Activity::default();
        edit(&mut a, "/r/src/a.rs", 1);
        edit(&mut a, "src/b.rs", 2);
        a.note(
            &json!({"hook_event_name":"PostToolUse","cwd":"/r","tool_name":"apply_patch",
                    "tool_input":{"command":"*** Begin Patch\n*** Update File: src/c.rs\n@@\n*** Add File: d.rs\n"}}),
            3,
        );
        edit(&mut a, "/r/src/a.rs", 4);
        let files: Vec<_> = a
            .edited_since(0)
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        assert_eq!(
            files,
            ["/r/src/a.rs", "/r/d.rs", "/r/src/c.rs", "/r/src/b.rs"]
        );
        a.note(
            &json!({"hook_event_name":"UserPromptSubmit","prompt":"\n  fix the login bug\nmore"}),
            5,
        );
        assert_eq!(a.prompt.as_ref().unwrap().0, "fix the login bug");
        a.note(
            &json!({"hook_event_name":"UserPromptSubmit","prompt":"<task-notification>"}),
            6,
        );
        assert_eq!(a.prompt.as_ref().unwrap().0, "fix the login bug");
    }

    #[test]
    fn tells_about_busy_siblings_and_shared_files_once() {
        let now = 100 * MIN;
        let mut mine = Activity::default();
        edit(&mut mine, "/r/src/auth.rs", now - MIN);
        let mut busy = Activity::default();
        edit(&mut busy, "/r/src/auth.rs", now - 2 * MIN);
        edit(&mut busy, "/r/src/db.rs", now - 2 * MIN);
        busy.note(
            &json!({"hook_event_name":"UserPromptSubmit","prompt":"refactor auth"}),
            now - 3 * MIN,
        );
        let mut old = Activity::default();
        edit(&mut old, "/r/x.rs", now - 60 * MIN);
        let siblings = [
            Sibling {
                name: "api",
                agent: "codex",
                state: "working",
                checkout: None,
                activity: &busy,
            },
            Sibling {
                name: "stale",
                agent: "claude",
                state: "idle",
                checkout: None,
                activity: &old,
            },
        ];
        let text = tell(Path::new("/r"), &mine, &siblings, now).unwrap();
        assert!(
            text.contains(
                "- api (codex, working): asked “refactor auth”; editing src/db.rs, src/auth.rs"
            ),
            "{text}"
        );
        assert!(!text.contains("stale"), "{text}");
        assert!(
            text.contains("You have both edited: src/auth.rs (api)"),
            "{text}"
        );
        assert!(mine.tell(&text));
        assert!(!mine.tell(&text));
        mine.note(&json!({"hook_event_name":"SessionStart"}), now);
        assert!(mine.tell(&text));

        let idle = [Sibling {
            name: "stale",
            agent: "claude",
            state: "idle",
            checkout: None,
            activity: &old,
        }];
        assert!(tell(Path::new("/r"), &mine, &idle, now).is_none());
    }

    #[test]
    fn the_same_file_in_another_worktree_overlaps() {
        let now = 100 * MIN;
        let mut mine = Activity::default();
        edit(&mut mine, "/r/src/auth.rs", now);
        let mut theirs = Activity::default();
        edit(&mut theirs, "/r-feature/src/auth.rs", now);
        let wt = Path::new("/r-feature");
        let siblings = [Sibling {
            name: "feat",
            agent: "claude",
            state: "working",
            checkout: Some(wt),
            activity: &theirs,
        }];
        let text = tell(Path::new("/r"), &mine, &siblings, now).unwrap();
        assert!(text.contains("(claude, working, in /r-feature)"), "{text}");
        assert!(
            text.contains("You have both edited: src/auth.rs (feat)"),
            "{text}"
        );
    }
}

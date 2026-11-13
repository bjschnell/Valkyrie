//! Search over session history (backlog #106): find the session where you said
//! or did something. Read from the same event logs, by the same reader, as the
//! away digest. Read-only: a hit names a live session to open, or the command
//! that resumes a closed one; it never acts on its own.

use crate::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Where in a session a hit was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchField {
    /// A prompt you gave it.
    You,
    /// The agent's closing reply at the end of a turn.
    Agent,
    /// A file it wrote or edited.
    File,
    /// What it was about: its backlog item, topic, name, branch or pull request.
    About,
}

impl SearchField {
    pub fn label(self) -> &'static str {
        match self {
            Self::You => "you",
            Self::Agent => "agent",
            Self::File => "file",
            Self::About => "about",
        }
    }
}

/// One session that matched, at its best line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    /// Its event log's name (`<created ns>-<id>`): stable across daemon restarts.
    pub key: String,
    /// The live session, while it runs in this daemon: open it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    /// The repository's folder name, or the directory's outside git.
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// `cwd` is gone (a removed worktree): a resume there can't start, and the
    /// agent may not find its conversation from anywhere else.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gone: bool,
    /// `claude`, `codex`, ...
    pub agent: String,
    /// What it was about, one line: its backlog item, topic, name or first prompt.
    pub what: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
    /// Where the best line was found, and the line, clipped around the words.
    pub field: SearchField,
    pub snippet: String,
    /// When that line was said (ms since the epoch).
    pub at_ms: u64,
    /// Lines in the session holding every word.
    pub lines: u32,
    /// Its last activity, and when it ended, if it did.
    pub last_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
    /// A closed session: the command that resumes its conversation, to run in
    /// `cwd` (`claude --resume <id>`). `None` while it runs, or when its agent
    /// can't resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub query: String,
    /// The window searched (ms since the epoch).
    pub since_ms: u64,
    pub until_ms: u64,
    /// Newest first, at most the limit asked for.
    pub hits: Vec<SearchHit>,
    /// Sessions that matched, before the limit.
    pub total: u32,
}

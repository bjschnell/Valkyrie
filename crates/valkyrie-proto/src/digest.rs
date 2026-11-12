//! The away digest (backlog #105): what every agent did in a window, one line a
//! session, read from the sessions' event logs. Read-only: it reports, it never
//! acts.

use crate::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How the digest is grouped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DigestGroup {
    #[default]
    Repo,
    Day,
}

impl DigestGroup {
    pub fn parse_str(s: &str) -> Option<Self> {
        match s {
            "repo" => Some(Self::Repo),
            "day" => Some(Self::Day),
            _ => None,
        }
    }
}

/// Where a session's work ended up. Declared most urgent first: lists sort by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Waiting on you: a question, a permission, a supervisor flag, red CI, or
    /// work left short of shipping not long ago.
    Stuck,
    /// Ended, or idle for hours, with work not shipped.
    Abandoned,
    /// A pull request is open.
    Waiting,
    /// Still at work.
    Working,
    /// Its pull request merged, or its backlog item shipped.
    Shipped,
    /// Finished with nothing left to ship.
    Done,
}

impl Outcome {
    pub const ALL: [Outcome; 6] = [
        Self::Stuck,
        Self::Abandoned,
        Self::Waiting,
        Self::Working,
        Self::Shipped,
        Self::Done,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Stuck => "stuck",
            Self::Abandoned => "abandoned",
            Self::Waiting => "waiting on PR/CI",
            Self::Working => "working",
            Self::Shipped => "shipped",
            Self::Done => "done",
        }
    }
}

/// One agent session in the window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DigestEntry {
    /// Its event log's name (`<created ns>-<id>`): stable across daemon restarts.
    pub key: String,
    /// The live session, while it runs in this daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    /// The repository's folder name, or the directory's outside git.
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
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
    pub outcome: Outcome,
    /// Why that outcome, one line ("PR #12 merged", "CI red on PR #9: lint").
    pub detail: String,
    /// The agent's own last line, when it said one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Prompts it was given in the window.
    pub prompts: u32,
    /// First and last activity in the window (ms since the epoch).
    pub first_ms: u64,
    pub last_ms: u64,
    /// When it ended, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Digest {
    /// The window (ms since the epoch).
    pub since_ms: u64,
    pub until_ms: u64,
    /// `since_ms` is when you were last gone, not a time you gave.
    pub away: bool,
    pub by: DigestGroup,
    /// Most urgent first, then most recent.
    pub entries: Vec<DigestEntry>,
    /// The same, as markdown to paste: a standup, a message.
    pub markdown: String,
}

//! What the supervisor did (backlog #180, protocol 43): each step it offered and
//! you answered, or a policy took without asking, and each build order answered,
//! newest first, for the supervisor page.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One thing the supervisor did, or offered and got an answer to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisorEntry {
    /// Seconds since the epoch.
    pub at: u64,
    /// The repository it was in.
    pub project: PathBuf,
    /// One line: "you took: fix CI on PR #101", "policy #12: started #45".
    pub said: String,
    /// The policy that took it without asking; `None` when you answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy: Option<u32>,
    /// The pull request it was about, as a URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
}

/// What the PR poll last read of a pull request it watches (backlog #181,
/// protocol 44).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedPr {
    pub url: String,
    /// The repository it's in.
    pub project: PathBuf,
    /// The backlog item it was read from; `None`: found from a session's branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub state: PrState,
    /// One line: "CI red on PR #101: lint", "checks still running".
    pub detail: String,
    /// When it was read, seconds since the epoch.
    pub read_at: u64,
}

/// An open pull request, as the poll read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    /// Every check passed and it merges cleanly.
    Green,
    /// Checks still running, or GitHub hasn't worked out whether it merges.
    Pending,
    /// A check failed.
    Red,
    /// It conflicts with its base (red too, maybe).
    Conflicting,
    /// `gh` couldn't read it this time (offline, signed out, too slow).
    Unknown,
}

impl PrState {
    pub fn label(self) -> &'static str {
        match self {
            PrState::Green => "green",
            PrState::Pending => "pending",
            PrState::Red => "red",
            PrState::Conflicting => "conflicting",
            PrState::Unknown => "unread",
        }
    }
}

/// `SupervisorWatch`'s reply, for a client to hold.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Watch {
    pub prs: Vec<WatchedPr>,
    pub polled_at: Option<u64>,
    pub next_at: Option<u64>,
    pub on: bool,
}

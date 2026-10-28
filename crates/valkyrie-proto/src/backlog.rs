//! The backlog (DESIGN §8.12): bugs, features and chores written down for later,
//! each of which can start an agent session on itself.

use crate::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What sort of work an item is; it picks the brief its agent starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Bug,
    #[default]
    Feature,
    Chore,
}

impl ItemKind {
    pub const ALL: [ItemKind; 3] = [Self::Bug, Self::Feature, Self::Chore];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bug => "bug",
            Self::Feature => "feature",
            Self::Chore => "chore",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// Not done yet, whether or not an agent has started on it.
    #[default]
    Open,
    Shipped,
    /// Not going to happen; kept so the list remembers it.
    Dropped,
    /// Written down by an agent, waiting for the user to accept it before it can
    /// be started.
    Proposed,
}

impl ItemStatus {
    pub const ALL: [ItemStatus; 4] = [Self::Open, Self::Shipped, Self::Dropped, Self::Proposed];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Shipped => "shipped",
            Self::Dropped => "dropped",
            Self::Proposed => "proposed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Numbered from 1 across every project, so `valk todo launch 12` needs no folder.
    pub id: u32,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub kind: ItemKind,
    #[serde(default)]
    pub status: ItemStatus,
    /// The repository it's for; `None` until one is picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<PathBuf>,
    /// Screenshots and files, copied into the backlog's own folder.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<PathBuf>,
    /// `human`, or the agent that wrote it down.
    #[serde(default)]
    pub by: String,
    /// Sessions started on it, oldest first. Ids are only meaningful to the daemon
    /// that started them: a restarted one reuses them, so clients match on `boot`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SessionId>,
    /// The daemon `boot` that `sessions` belong to.
    #[serde(default)]
    pub boot: u64,
    /// The branch and checkout its first launch made, reused by later ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<PathBuf>,
    /// Handoffs left on it, oldest first: what an agent did and what's left, or
    /// the user's own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<ItemNote>,
    /// Seconds since the epoch.
    pub created: u64,
    pub updated: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemNote {
    /// Seconds since the epoch.
    pub at: u64,
    /// `human`, or the agent that left it.
    pub by: String,
    pub text: String,
}

/// What an item's checkout holds, read from git rather than taken from the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemFacts {
    pub branch: String,
    /// Commits on it that the repository's own checkout doesn't have.
    pub ahead: u32,
    /// The newest of them, `<short hash> <subject>`, at most ten.
    pub commits: Vec<String>,
    /// `git diff --shortstat` against where it started, e.g. `3 files changed, 40
    /// insertions(+), 2 deletions(-)`; empty when nothing changed.
    pub diff: String,
    /// Files changed or untracked but not committed.
    pub dirty: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NewItem {
    /// May be empty: then the body's first line is the title.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub kind: ItemKind,
    /// A folder inside the repository it's for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Files to keep with it (an upload's path, or any file the daemon can read).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum ItemChange {
    /// Rewords it, or moves it to another repository (`cwd: None` keeps it where it is).
    Edit {
        title: String,
        body: String,
        kind: ItemKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
    },
    /// Ships, drops or reopens it. Shipping also takes away a worktree its launch
    /// made, when nothing is left in it.
    Status { status: ItemStatus },
}

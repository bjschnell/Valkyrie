//! Project decisions (DESIGN §6, ADR-0007): what the context layer stores, reviews
//! and injects.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What sort of thing a decision records. Constraints rank first when injected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    #[default]
    Decision,
    Constraint,
    Pattern,
    Gotcha,
    Fix,
}

impl DecisionKind {
    pub const ALL: [DecisionKind; 5] = [
        Self::Decision,
        Self::Constraint,
        Self::Pattern,
        Self::Gotcha,
        Self::Fix,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Constraint => "constraint",
            Self::Pattern => "pattern",
            Self::Gotcha => "gotcha",
            Self::Fix => "fix",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStatus {
    /// Waiting on a human; never injected.
    #[default]
    Proposed,
    /// Confirmed by a human; injected into every agent in the project.
    Active,
    Rejected,
    /// Replaced by the decision named in `superseded_by`.
    Superseded,
    /// No longer holds, with nothing replacing it.
    Retired,
}

impl DecisionStatus {
    pub const ALL: [DecisionStatus; 5] = [
        Self::Proposed,
        Self::Active,
        Self::Rejected,
        Self::Superseded,
        Self::Retired,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Active => "active",
            Self::Rejected => "rejected",
            Self::Superseded => "superseded",
            Self::Retired => "retired",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// Where a decision came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Provenance {
    /// `human`, or the agent that proposed it (`claude`, `codex`).
    pub by: String,
    /// The session's name when it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The agent's own conversation id, to find the transcript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    /// `HEAD` of the checkout it was recorded in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// The directory it was recorded in (a worktree, or below the root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// Numbered from 1 within its project.
    pub id: u32,
    /// The project's root (ADR-0007 §2).
    pub project: PathBuf,
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub kind: DecisionKind,
    pub status: DecisionStatus,
    /// Seconds since the epoch.
    pub created: u64,
    pub updated: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<u32>,
    #[serde(default)]
    pub provenance: Provenance,
    #[serde(default)]
    pub fresh: Freshness,
}

/// Whether a decision may be out of date (DESIGN §6.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Freshness {
    /// When a human last said it holds (seconds); 0: when it was made active.
    #[serde(default)]
    pub confirmed: u64,
    /// Days after `confirmed` that it's due for review again, for decisions about
    /// things that change (versions, endpoints).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_every: Option<u32>,
    /// Files it's about, relative to the project root: if they change a lot, it may
    /// no longer hold.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<String>,
    /// Why it should be looked at again, when it should; worked out by the daemon,
    /// never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<String>,
}

/// A decision as asked for by `valk decide`, the TUI or the web app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewDecision {
    /// Any directory inside the project.
    pub cwd: PathBuf,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub kind: DecisionKind,
    /// Ask for review even when a human records it.
    #[serde(default)]
    pub propose: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<u32>,
    /// Review it again this many days after it's confirmed; `None`: chosen from
    /// what it says (30 days for versions and URLs, else never).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_every: Option<u32>,
    /// The session it was recorded from (`$VALK_SESSION`). A session running an
    /// agent can only propose (ADR-0007 §4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<crate::SessionId>,
}

/// Which sessions' corrections are read for decisions (DESIGN §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoMode {
    Off,
    /// Claude Code sessions: the model is Claude, so nothing leaves for a provider
    /// the session didn't already use.
    #[default]
    Claude,
    /// Codex sessions too, whose conversations then also go to Anthropic.
    All,
}

impl AutoMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Claude => "claude",
            Self::All => "all",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "off" => Some(Self::Off),
            "claude" | "on" => Some(Self::Claude),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    pub fn reads(self, agent: &str) -> bool {
        match self {
            Self::Off => false,
            Self::Claude => agent == "claude",
            Self::All => matches!(agent, "claude" | "codex"),
        }
    }
}

/// What a human does to a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "a", rename_all = "snake_case")]
pub enum ReviewAction {
    Accept,
    Reject,
    Retire,
    /// It still holds: an active decision flagged for review is confirmed as of now.
    Confirm,
    /// Change the wording (and kind) without changing the status.
    Edit {
        title: String,
        #[serde(default)]
        body: String,
        kind: DecisionKind,
    },
    /// Reword a proposal and accept it, in one step: refused once it isn't a
    /// proposal any more, so a stale card can't rewrite an active decision.
    Revise {
        title: String,
        #[serde(default)]
        body: String,
        kind: DecisionKind,
    },
}

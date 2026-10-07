//! Agent state and attention-queue types (DESIGN §14.3–14.4).

use crate::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Working,
    /// Waiting on the human: a permission, a question, a prompt on screen.
    NeedsInput,
    /// A failed turn, or the program exited with an error.
    Blocked,
    /// A turn finished and the human hasn't looked yet.
    ReviewReady,
    Idle,
    /// The screen shows an idle prompt while hooks still say busy: an Esc-interrupt
    /// or Esc-deny, which fire no hook.
    Interrupted,
    /// Working, but no output or events for a long time.
    Stale,
    Exited,
}

impl AgentState {
    pub fn label(self) -> &'static str {
        match self {
            AgentState::Working => "working",
            AgentState::NeedsInput => "needs input",
            AgentState::Blocked => "blocked",
            AgentState::ReviewReady => "done",
            AgentState::Idle => "idle",
            AgentState::Interrupted => "interrupted?",
            AgentState::Stale => "stale",
            AgentState::Exited => "exited",
        }
    }
}

/// What a `NeedsInput` session is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskKind {
    Permission,
    Question,
    Input,
    /// Recognized on screen without a hook (e.g. a folder-trust prompt).
    Screen,
    /// A terminal bell from a program without hooks.
    Bell,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStatus {
    /// Adapter name: `claude`, `codex` or `generic`.
    pub agent: String,
    pub state: AgentState,
    pub ask: Option<AskKind>,
    /// One line: what it needs, or what it finished.
    pub summary: Option<String>,
    /// When the session entered `state` (ms since the epoch).
    pub since_ms: u64,
    /// Bumped on every transition; `MarkSeen` names the transition it saw.
    pub seq: u64,
    /// The human has looked at this transition (attached, or marked it seen).
    pub seen: bool,
    /// At least one hook event arrived, so state isn't heuristics only.
    pub hooked: bool,
}

impl Default for AgentStatus {
    fn default() -> Self {
        Self::new("unknown", 0)
    }
}

impl AgentStatus {
    pub fn new(agent: &str, now_ms: u64) -> Self {
        Self {
            agent: agent.to_owned(),
            state: AgentState::Idle,
            ask: None,
            summary: None,
            since_ms: now_ms,
            seq: 0,
            seen: true,
            hooked: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueItem {
    pub session: SessionId,
    pub name: String,
    pub cwd: PathBuf,
    pub status: AgentStatus,
}

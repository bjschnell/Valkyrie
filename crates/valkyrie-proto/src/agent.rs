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
    /// The name is the agent's topic, not one the user gave (`SessionInfo::auto_name`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_name: bool,
    pub cwd: PathBuf,
    pub status: AgentStatus,
    /// What the supervisor flagged on this session's pull request after its last
    /// stop, and the step it offers (ADR-0019 §1, §2). Only while the session still
    /// shows that flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<Flag>,
    /// The session's published status (`SessionInfo::report`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

/// A watcher's flag on an idle session (ADR-0019 §1): the reason the queue shows,
/// and the one step the supervisor proposes for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flag {
    /// "PR #98 conflicts with main", "CI red on PR #101: windows".
    pub reason: String,
    /// The pull request it's about.
    pub pr: String,
    /// The status transition the flag raised: `TakeStep` names it, so a tap on a
    /// flag that has moved on does nothing.
    pub seq: u64,
    /// `None` while the supervisor works out which step to offer (`working`), and
    /// once one was taken or the user left the flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<Step>,
    /// The supervisor is still working out its step (the flake call can take a
    /// while).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub working: bool,
}

/// The step offered for a flag (ADR-0019 §2): one action from a fixed list, why
/// it was picked, and the other one that would fit, if any. `leave` is always
/// there besides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub action: StepAction,
    /// One line: "every failed check matches #17", "conflicting".
    pub why: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<StepAction>,
}

/// What the supervisor can do for a flagged session. Each is a fixed template;
/// no free text reaches an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepAction {
    /// Types skill #25's text into the idle session: merge the base in.
    MergeBase,
    /// Types ci-fix's text into the idle session (ADR-0013 §3).
    CiFix,
    /// Reruns the failed jobs (`gh run rerun --failed`) without waking the agent.
    Rerun,
    /// Does nothing; the flag stays for the user.
    Leave,
    /// Starts a ready backlog item on its last agent and model (ADR-0019 §2).
    Launch,
}

impl StepAction {
    pub fn as_str(self) -> &'static str {
        match self {
            StepAction::MergeBase => "merge-base",
            StepAction::CiFix => "ci-fix",
            StepAction::Rerun => "rerun",
            StepAction::Leave => "leave",
            StepAction::Launch => "launch",
        }
    }

    /// What taking it does, for a button or a key hint.
    pub fn label(self) -> &'static str {
        match self {
            StepAction::MergeBase => "merge the base in",
            StepAction::CiFix => "fix CI",
            StepAction::Rerun => "rerun failed jobs",
            StepAction::Leave => "leave it",
            StepAction::Launch => "start it",
        }
    }
}

//! Agent adapters, per-session state tracking and queue ranking (DESIGN §14,
//! ADR-0005). Pure logic: no I/O, no clocks; the daemon feeds it events and time.

mod claude;
pub mod codex;
pub mod summary;
mod tracker;

use overseer_proto::{AgentState, AgentStatus, AskKind, QueueItem};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub use tracker::{DENY_AFTER_MS, INTERRUPT_AFTER_MS, STALE_AFTER_MS, Tracker};

/// What adapters normalize hook payloads (and PTY signals) into.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "e", rename_all = "snake_case")]
pub enum AgentEvent {
    SessionStarted,
    SessionEnded,
    PromptSubmitted,
    ToolStarted,
    ToolFinished,
    PermissionAsked {
        summary: String,
    },
    QuestionAsked {
        summary: String,
    },
    InputAsked {
        summary: String,
    },
    /// A permission notification; trails `PermissionAsked` in Claude Code.
    PermissionNotified {
        summary: String,
    },
    TurnEnded {
        summary: Option<String>,
    },
    TurnFailed {
        summary: String,
    },
    Interrupted,
    Bell,
    Exited {
        code: Option<i32>,
    },
}

/// What the visible screen says, when an adapter recognizes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    /// A prompt waiting on the human (trust prompt, permission dialog).
    Prompt {
        summary: String,
    },
    Busy,
    /// The agent's input prompt, ready for the next message.
    Idle,
}

pub trait Adapter: Send + Sync {
    fn name(&self) -> &'static str;
    /// Rewrites the command line before spawn, e.g. to register hooks. `hook_exe` is
    /// the overseer binary the hooks should run.
    fn prepare(&self, _command: &mut Vec<String>, _hook_exe: &Path) {}
    fn normalize(&self, payload: &Value) -> Vec<AgentEvent>;
    fn scan(&self, _screen: &str) -> Option<Screen> {
        None
    }
    /// Whether some transitions fire no hook, so the screen must check for them.
    fn hook_gaps(&self) -> bool {
        false
    }
}

pub struct Generic;

impl Adapter for Generic {
    fn name(&self) -> &'static str {
        "generic"
    }
    fn normalize(&self, _: &Value) -> Vec<AgentEvent> {
        Vec::new()
    }
}

/// The screen as one line with whitespace collapsed, so phrases match however the
/// agent wrapped them to the terminal width.
pub(crate) fn flatten(screen: &str) -> String {
    screen.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The adapter for a command line, by program name.
pub fn adapter_for(command: &[String]) -> &'static dyn Adapter {
    let program = command
        .first()
        .and_then(|p| Path::new(p).file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("");
    by_name(program)
}

/// The adapter a hook names (`overseer hook <agent>`).
pub fn by_name(name: &str) -> &'static dyn Adapter {
    match name {
        "claude" => &claude::Claude,
        "codex" => &codex::Codex,
        _ => &Generic,
    }
}

/// Whether a status belongs in the queue (DESIGN §14.4). Waiting on an answer stays
/// queued until answered; everything else leaves once seen.
pub fn queued(s: &AgentStatus) -> bool {
    match s.state {
        AgentState::NeedsInput => !(s.seen && s.ask == Some(AskKind::Bell)),
        AgentState::Blocked
        | AgentState::ReviewReady
        | AgentState::Interrupted
        | AgentState::Stale => !s.seen,
        AgentState::Working | AgentState::Idle | AgentState::Exited => false,
    }
}

fn rank(state: AgentState) -> u8 {
    match state {
        AgentState::NeedsInput => 0,
        AgentState::Blocked => 1,
        AgentState::ReviewReady => 2,
        AgentState::Interrupted | AgentState::Stale => 3,
        AgentState::Working | AgentState::Idle | AgentState::Exited => 4,
    }
}

/// Ranked: needs input > blocked > done > interrupted/stale; oldest first, then by id.
pub fn sort_queue(items: &mut [QueueItem]) {
    items.sort_by_key(|i| (rank(i.status.state), i.status.since_ms, i.session));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(session: u32, state: AgentState, since_ms: u64) -> QueueItem {
        let mut status = AgentStatus::new("claude", since_ms);
        status.state = state;
        status.seen = false;
        QueueItem {
            session,
            name: String::new(),
            cwd: "/".into(),
            status,
        }
    }

    #[test]
    fn ranks_by_state_then_age_then_id() {
        let mut items = vec![
            item(1, AgentState::ReviewReady, 5),
            item(2, AgentState::NeedsInput, 9),
            item(3, AgentState::Stale, 1),
            item(4, AgentState::NeedsInput, 3),
            item(5, AgentState::Blocked, 7),
            item(6, AgentState::Interrupted, 1),
        ];
        sort_queue(&mut items);
        let order: Vec<u32> = items.iter().map(|i| i.session).collect();
        assert_eq!(order, [4, 2, 5, 1, 3, 6]);
    }

    #[test]
    fn seen_clears_everything_except_an_open_question() {
        let mut s = AgentStatus::new("claude", 0);
        s.seen = true;
        s.state = AgentState::ReviewReady;
        assert!(!queued(&s));
        s.state = AgentState::NeedsInput;
        s.ask = Some(AskKind::Permission);
        assert!(queued(&s));
        s.ask = Some(AskKind::Bell);
        assert!(!queued(&s));
        s.seen = false;
        assert!(queued(&s));
        s.state = AgentState::Working;
        assert!(!queued(&s));
    }

    #[test]
    fn picks_adapters_by_program_name() {
        let cmd = |s: &str| vec![s.to_string()];
        assert_eq!(
            adapter_for(&cmd("/home/u/.local/bin/claude")).name(),
            "claude"
        );
        assert_eq!(adapter_for(&cmd("codex")).name(), "codex");
        assert_eq!(adapter_for(&cmd("bash")).name(), "generic");
        assert_eq!(adapter_for(&[]).name(), "generic");
    }
}

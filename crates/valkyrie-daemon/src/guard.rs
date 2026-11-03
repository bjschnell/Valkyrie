//! The merge guard's confirms on a session's screen (ADR-0015 §1). `valk hook`
//! answers an agent's merge or push to the default branch with Claude Code's
//! "ask" and marks the hook payload. While that confirm is up, the user answers
//! it, not another agent typing into the session with `valk send`.

use std::collections::HashSet;
use valkyrie_proto::AgentState;

/// The tool calls the guard asked about whose confirm may still be on screen.
#[derive(Debug, Default)]
pub(crate) struct Pending {
    calls: HashSet<String>,
}

impl Pending {
    /// Follows one hook event from the session's own agent.
    pub(crate) fn hook(&mut self, payload: &serde_json::Value) {
        let call = || payload["tool_use_id"].as_str().unwrap_or("").to_owned();
        match payload["hook_event_name"].as_str() {
            Some("PreToolUse") if payload["valk_merge_guard"]["decision"] == "ask" => {
                self.calls.insert(call());
            }
            // The call ran, or failed: its confirm was answered.
            Some("PostToolUse" | "PostToolUseFailure") => {
                self.calls.remove(&call());
            }
            // A new turn or none at all: no confirm of this one is left.
            Some("UserPromptSubmit" | "Stop" | "StopFailure" | "SessionStart" | "SessionEnd") => {
                self.calls.clear()
            }
            _ => {}
        }
    }

    /// Whether the session is at a confirm the guard asked for. A call the user
    /// declined fires no hook of its own, so it counts only while the session
    /// still waits on the user; past that, at worst it holds off agents from
    /// another prompt in the same turn, the safe way to be wrong.
    pub(crate) fn on_screen(&self, state: AgentState) -> bool {
        !self.calls.is_empty() && state == AgentState::NeedsInput
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(name: &str, id: &str) -> serde_json::Value {
        json!({"hook_event_name": name, "tool_use_id": id})
    }

    fn guarded(id: &str) -> serde_json::Value {
        let mut p = event("PreToolUse", id);
        p["valk_merge_guard"] = json!({"kind": "merge", "decision": "ask"});
        p
    }

    #[test]
    fn a_guard_confirm_holds_while_the_session_waits_on_the_user() {
        let mut pending = Pending::default();
        pending.hook(&guarded("a"));
        assert!(pending.on_screen(AgentState::NeedsInput));
        assert!(!pending.on_screen(AgentState::Working));
        pending.hook(&event("PostToolUse", "a"));
        assert!(!pending.on_screen(AgentState::NeedsInput));
    }

    #[test]
    fn other_calls_and_prompts_are_not_the_guards() {
        let mut pending = Pending::default();
        pending.hook(&event("PreToolUse", "b"));
        assert!(!pending.on_screen(AgentState::NeedsInput));
        // Parallel calls: another call finishing leaves the guard's confirm up.
        pending.hook(&guarded("a"));
        pending.hook(&event("PostToolUse", "b"));
        assert!(pending.on_screen(AgentState::NeedsInput));
        // Declined, then the turn ended.
        pending.hook(&event("Stop", ""));
        assert!(!pending.on_screen(AgentState::NeedsInput));
    }

    #[test]
    fn a_refusal_puts_nothing_on_screen() {
        let mut pending = Pending::default();
        let mut p = event("PreToolUse", "a");
        p["valk_merge_guard"] = json!({"kind": "push", "decision": "deny"});
        pending.hook(&p);
        assert!(!pending.on_screen(AgentState::NeedsInput));
    }
}

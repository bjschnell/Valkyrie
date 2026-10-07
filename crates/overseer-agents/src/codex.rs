//! Codex. Hooks live in `~/.codex/hooks.json`, written once by `overseer setup codex`
//! and trusted once with `/hooks` (ADR-0005). Same event names as Claude Code minus
//! `Notification`, plus `Interrupt`.

use crate::summary::{describe_tool, pick_summary_line};
use crate::{Adapter, AgentEvent, Screen};
use serde_json::{Value, json};

pub struct Codex;

pub const EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Stop",
    "Interrupt",
];
const HOOK_TIMEOUT: u64 = 5;

impl Adapter for Codex {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn normalize(&self, p: &Value) -> Vec<AgentEvent> {
        normalize(p).into_iter().collect()
    }

    fn scan(&self, screen: &str) -> Option<Screen> {
        scan(screen)
    }
}

fn normalize(p: &Value) -> Option<AgentEvent> {
    use AgentEvent as E;
    if p.get("agent_id").is_some_and(|v| !v.is_null())
        && !matches!(
            p["hook_event_name"].as_str(),
            Some("PreToolUse" | "PermissionRequest" | "PostToolUse")
        )
    {
        return None;
    }
    Some(match p["hook_event_name"].as_str()? {
        "SessionStart" => match p["source"].as_str() {
            Some("startup" | "resume" | "clear") => E::SessionStarted,
            _ => return None,
        },
        "SessionEnd" => E::SessionEnded,
        "UserPromptSubmit" => E::PromptSubmitted,
        "PreToolUse" => E::ToolStarted,
        "PermissionRequest" => E::PermissionAsked {
            summary: describe_tool(p),
        },
        "PostToolUse" => E::ToolFinished,
        "Stop" => E::TurnEnded {
            summary: p["last_assistant_message"]
                .as_str()
                .and_then(pick_summary_line),
        },
        "Interrupt" => E::Interrupted,
        _ => return None,
    })
}

/// Verified against Codex 0.159 recordings: trust prompt, approval dialog, the
/// auto-reviewer spinner, busy, and the composer.
fn scan(screen: &str) -> Option<Screen> {
    let lines: Vec<&str> = screen.lines().map(str::trim).collect();
    let flat = crate::flatten(screen);
    let has = |needle: &str| flat.contains(needle);
    if has("Trust this folder?") || has("Do you trust the contents of this directory") {
        return Some(Screen::Prompt {
            summary: "Codex asks whether to trust this folder".into(),
        });
    }
    if let Some(q) = lines
        .iter()
        .find(|l| l.starts_with("Would you like to") && l.ends_with('?'))
    {
        return Some(Screen::Prompt {
            summary: crate::summary::truncate(q),
        });
    }
    if has("Reviewing approval request") {
        return Some(Screen::Reviewing);
    }
    if lines
        .iter()
        .any(|l| l.to_lowercase().contains("esc to interrupt"))
    {
        return Some(Screen::Busy);
    }
    let composer = lines.iter().any(|l| match l.strip_prefix('›') {
        Some(rest) => {
            let rest = rest.trim_start();
            !rest.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    });
    composer.then_some(Screen::Idle)
}

/// `hooks.json` with our entry for every event added, replacing any older copy of
/// it (matched by `command`). Everything else in the file is kept.
pub fn install_hooks(mut doc: Value, command: &str) -> Value {
    doc = remove_hooks(doc, command);
    if !doc.is_object() {
        doc = json!({});
    }
    if !doc["hooks"].is_object() {
        doc["hooks"] = json!({});
    }
    let entry =
        json!({ "hooks": [{ "type": "command", "command": command, "timeout": HOOK_TIMEOUT }] });
    for event in EVENTS {
        let groups = &mut doc["hooks"][*event];
        if !groups.is_array() {
            *groups = json!([]);
        }
        groups.as_array_mut().unwrap().push(entry.clone());
    }
    doc
}

/// `hooks.json` without any hook running `command`; emptied groups and events go too.
pub fn remove_hooks(mut doc: Value, command: &str) -> Value {
    let Some(hooks) = doc.get_mut("hooks").and_then(Value::as_object_mut) else {
        return doc;
    };
    for groups in hooks.values_mut() {
        let Some(list) = groups.as_array_mut() else {
            continue;
        };
        list.retain_mut(|group| {
            let Some(inner) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = inner.len();
            inner.retain(|h| h["command"].as_str() != Some(command));
            !(before > 0 && inner.is_empty())
        });
    }
    hooks.retain(|_, groups| groups.as_array().is_none_or(|g| !g.is_empty()));
    doc
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentEvent as E;

    #[test]
    fn maps_events_including_interrupt() {
        let n = |v: Value| normalize(&v);
        assert_eq!(
            n(json!({"hook_event_name": "Interrupt", "turn_id": "t"})),
            Some(E::Interrupted)
        );
        assert_eq!(
            n(
                json!({"hook_event_name": "PermissionRequest", "tool_name": "shell",
                     "tool_input": {"description": "run cargo test"}})
            ),
            Some(E::PermissionAsked {
                summary: "Permission: shell run cargo test".into()
            })
        );
        assert_eq!(
            n(json!({"hook_event_name": "Stop", "last_assistant_message": "All tests pass."})),
            Some(E::TurnEnded {
                summary: Some("All tests pass.".into())
            })
        );
        assert_eq!(
            n(json!({"hook_event_name": "SubagentStop", "agent_id": "x"})),
            None
        );
    }

    #[test]
    fn scans_the_recorded_trust_prompt() {
        let text = std::fs::read_to_string(format!(
            "{}/tests/fixtures/codex_trust.txt",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        assert_eq!(
            scan(&text),
            Some(Screen::Prompt {
                summary: "Codex asks whether to trust this folder".into()
            })
        );
        assert_eq!(
            scan("• Working (3s • esc to interrupt)"),
            Some(Screen::Busy)
        );
    }

    #[test]
    fn scans_recorded_codex_screens() {
        let fixture = |name: &str| {
            std::fs::read_to_string(format!(
                "{}/tests/fixtures/{name}",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap()
        };
        assert_eq!(
            scan(&fixture("codex_approval.txt")),
            Some(Screen::Prompt {
                summary: "Would you like to run the following command?".into()
            })
        );
        // The auto-reviewer's spinner also says "esc to interrupt".
        assert_eq!(
            scan(&fixture("codex_reviewing.txt")),
            Some(Screen::Reviewing)
        );
        assert_eq!(scan(&fixture("codex_busy.txt")), Some(Screen::Busy));
        assert_eq!(scan(&fixture("codex_idle.txt")), Some(Screen::Idle));
    }

    #[test]
    fn install_is_idempotent_and_keeps_foreign_hooks() {
        let cmd = "'/bin/overseer' hook codex";
        let foreign = json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify"}]}]},
                             "other": 1});
        let once = install_hooks(foreign.clone(), cmd);
        let twice = install_hooks(once.clone(), cmd);
        assert_eq!(once, twice);
        assert_eq!(once["other"], 1);
        assert_eq!(once["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(once["hooks"]["Interrupt"][0]["hooks"][0]["command"], cmd);
        assert_eq!(remove_hooks(once, cmd), foreign);
        assert_eq!(
            install_hooks(Value::Null, cmd)["hooks"]
                .as_object()
                .unwrap()
                .len(),
            EVENTS.len()
        );
    }
}

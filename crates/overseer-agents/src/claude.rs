//! Claude Code. Hooks are registered per session with `--settings` (ADR-0005); the
//! event mapping follows Leader's live findings (`~/repos/leader/docs/findings.md`).

use crate::summary::{describe_tool, one_line, pick_summary_line, question};
use crate::{Adapter, AgentEvent, Screen};
use serde_json::{Value, json};
use std::path::Path;

pub struct Claude;

/// Events we register a hook for.
const EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Notification",
    "Stop",
    "StopFailure",
];
/// Seconds; the hook itself returns in milliseconds.
const HOOK_TIMEOUT: u64 = 5;
/// A subagent's dialogs and tool calls happen in the same terminal, but its
/// Stop/SessionStart must not settle the parent.
const SUBAGENT_EVENTS: &[&str] = &[
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "Notification",
];

impl Adapter for Claude {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn prepare(&self, command: &mut Vec<String>, hook_exe: &Path) {
        let settings = hook_settings(&format!(
            "{} hook claude 2>/dev/null || true",
            shell_quote(hook_exe)
        ));
        command.splice(1..1, ["--settings".to_string(), settings.to_string()]);
    }

    fn normalize(&self, p: &Value) -> Vec<AgentEvent> {
        normalize(p).into_iter().collect()
    }

    fn scan(&self, screen: &str) -> Option<Screen> {
        scan(screen)
    }

    /// Esc-deny and Esc-interrupt fire no hook (Leader findings §11).
    fn hook_gaps(&self) -> bool {
        true
    }
}

pub(crate) fn hook_settings(command: &str) -> Value {
    let entry =
        json!([{ "hooks": [{ "type": "command", "command": command, "timeout": HOOK_TIMEOUT }] }]);
    let hooks: serde_json::Map<String, Value> = EVENTS
        .iter()
        .map(|e| (e.to_string(), entry.clone()))
        .collect();
    json!({ "hooks": hooks })
}

pub(crate) fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

fn normalize(p: &Value) -> Option<AgentEvent> {
    use AgentEvent as E;
    let event = p["hook_event_name"].as_str()?;
    if p.get("agent_id").is_some_and(|v| !v.is_null()) && !SUBAGENT_EVENTS.contains(&event) {
        return None;
    }
    let tool = p["tool_name"].as_str();
    Some(match event {
        // `compact` can fire mid-turn.
        "SessionStart" => match p["source"].as_str() {
            Some("startup" | "resume" | "clear") => E::SessionStarted,
            _ => return None,
        },
        "SessionEnd" => E::SessionEnded,
        "UserPromptSubmit" => E::PromptSubmitted,
        "PreToolUse" if tool == Some("AskUserQuestion") => E::QuestionAsked {
            summary: question(p).unwrap_or_else(|| "Claude is asking a question".into()),
        },
        "PreToolUse" => E::ToolStarted,
        "PermissionRequest" if tool == Some("AskUserQuestion") => E::QuestionAsked {
            summary: question(p).unwrap_or_else(|| "Claude is asking a question".into()),
        },
        "PermissionRequest" => E::PermissionAsked {
            summary: describe_tool(p),
        },
        "PostToolUse" | "PostToolUseFailure" => E::ToolFinished,
        "Notification" => {
            let message = p["message"].as_str().and_then(one_line);
            match p["notification_type"].as_str()? {
                "permission_prompt" => E::PermissionNotified {
                    summary: message.unwrap_or_else(|| "Claude needs your permission".into()),
                },
                "elicitation_dialog" | "elicitation_url_dialog" | "agent_needs_input" => {
                    E::InputAsked {
                        summary: message.unwrap_or_else(|| "Claude needs your input".into()),
                    }
                }
                // idle_prompt and the rest: already settled, must not revive.
                _ => return None,
            }
        }
        "Stop" => E::TurnEnded {
            summary: p["last_assistant_message"]
                .as_str()
                .and_then(pick_summary_line),
        },
        "StopFailure" => {
            let error = ["error_type", "error"]
                .iter()
                .find_map(|k| p[k].as_str())
                .unwrap_or("error");
            E::TurnFailed {
                summary: format!("Turn failed: {error}"),
            }
        }
        _ => return None,
    })
}

fn scan(screen: &str) -> Option<Screen> {
    let lines: Vec<&str> = screen.lines().map(str::trim).collect();
    let flat = crate::flatten(screen);
    let has = |needle: &str| flat.contains(needle);
    if has("Is this a project you created or one you trust") || has("Do you trust the files") {
        return Some(Screen::Prompt {
            summary: "Claude asks whether to trust this folder".into(),
        });
    }
    // Permission dialogs: "Do you want to proceed?", "…make this edit to x?", …
    if let Some(i) = lines
        .iter()
        .rposition(|l| l.starts_with("Do you want to") && l.ends_with('?'))
        && lines[i..].iter().any(|l| is_option(l))
    {
        let command = lines[..i]
            .iter()
            .rev()
            .find_map(|l| l.strip_prefix("⎿").map(str::trim)?.strip_prefix("$ "));
        let summary = match command {
            Some(cmd) => format!("{} — {cmd}", lines[i]),
            None => lines[i].to_string(),
        };
        return Some(Screen::Prompt {
            summary: crate::summary::truncate(&summary),
        });
    }
    if lines
        .iter()
        .any(|l| l.to_lowercase().contains("esc to interrupt"))
    {
        return Some(Screen::Busy);
    }
    let prompt_box = lines.iter().any(|l| match l.strip_prefix('❯') {
        // Live Claude puts a no-break space after the glyph.
        Some(rest) => {
            rest.is_empty() || (rest.starts_with(char::is_whitespace) && !is_option(rest))
        }
        None => false,
    });
    prompt_box.then_some(Screen::Idle)
}

/// A dialog option such as `❯ 1. Yes` or `2. No`.
fn is_option(line: &str) -> bool {
    let s = line.trim_start_matches('❯').trim_start();
    let digits = s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    digits > 0 && s[digits..].starts_with(". ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentEvent as E;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn maps_the_turn_lifecycle() {
        let n = |v: Value| normalize(&v);
        assert_eq!(
            n(json!({"hook_event_name": "SessionStart", "source": "startup"})),
            Some(E::SessionStarted)
        );
        assert_eq!(
            n(json!({"hook_event_name": "SessionStart", "source": "compact"})),
            None
        );
        assert_eq!(
            n(json!({"hook_event_name": "UserPromptSubmit", "prompt": "hi"})),
            Some(E::PromptSubmitted)
        );
        assert_eq!(
            n(
                json!({"hook_event_name": "Stop", "last_assistant_message": "Looked.\nDone: fixed it."})
            ),
            Some(E::TurnEnded {
                summary: Some("Done: fixed it.".into())
            })
        );
        assert_eq!(
            n(json!({"hook_event_name": "StopFailure", "error_type": "rate_limit"})),
            Some(E::TurnFailed {
                summary: "Turn failed: rate_limit".into()
            })
        );
    }

    #[test]
    fn maps_permissions_questions_and_notifications() {
        let n = |v: Value| normalize(&v);
        assert_eq!(
            n(
                json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash",
                     "tool_input": {"command": "touch acc.txt"}})
            ),
            Some(E::PermissionAsked {
                summary: "Permission: Bash touch acc.txt".into()
            })
        );
        let ask = json!({"tool_name": "AskUserQuestion",
                         "tool_input": {"questions": [{"question": "Which DB?"}]}});
        for event in ["PreToolUse", "PermissionRequest"] {
            let mut p = ask.clone();
            p["hook_event_name"] = event.into();
            assert_eq!(
                n(p),
                Some(E::QuestionAsked {
                    summary: "Which DB?".into()
                })
            );
        }
        assert_eq!(
            n(json!({"hook_event_name": "PreToolUse", "tool_name": "Read"})),
            Some(E::ToolStarted)
        );
        assert_eq!(
            n(json!({"hook_event_name": "PostToolUseFailure", "tool_name": "Bash"})),
            Some(E::ToolFinished)
        );
        assert_eq!(
            n(
                json!({"hook_event_name": "Notification", "notification_type": "idle_prompt",
                     "message": "Claude is waiting"})
            ),
            None
        );
        assert_eq!(
            n(
                json!({"hook_event_name": "Notification", "notification_type": "permission_prompt",
                     "message": "Claude needs your permission to use Bash"})
            ),
            Some(E::PermissionNotified {
                summary: "Claude needs your permission to use Bash".into()
            })
        );
        assert_eq!(
            n(
                json!({"hook_event_name": "Notification", "notification_type": "elicitation_dialog",
                     "message": "Pick one"})
            ),
            Some(E::InputAsked {
                summary: "Pick one".into()
            })
        );
    }

    #[test]
    fn subagent_tool_events_count_but_its_turn_events_do_not() {
        let n = |v: Value| normalize(&v);
        assert_eq!(
            n(
                json!({"hook_event_name": "PermissionRequest", "agent_id": "a1", "tool_name": "Bash"})
            ),
            Some(E::PermissionAsked {
                summary: "Permission: Bash".into()
            })
        );
        assert_eq!(
            n(json!({"hook_event_name": "Stop", "agent_id": "a1"})),
            None
        );
        assert_eq!(
            n(json!({"hook_event_name": "Stop", "agent_id": null})),
            Some(E::TurnEnded { summary: None })
        );
    }

    #[test]
    fn prepare_registers_every_event_with_a_quoted_hook_command() {
        let mut cmd = vec!["claude".to_string(), "--model".into(), "haiku".into()];
        Claude.prepare(&mut cmd, Path::new("/opt/it's/overseer"));
        assert_eq!(cmd[0], "claude");
        assert_eq!(cmd[1], "--settings");
        assert_eq!(&cmd[3..], ["--model", "haiku"]);
        let settings: Value = serde_json::from_str(&cmd[2]).unwrap();
        for event in EVENTS {
            let hook = &settings["hooks"][event][0]["hooks"][0];
            assert_eq!(hook["type"], "command");
            assert_eq!(
                hook["command"],
                r"'/opt/it'\''s/overseer' hook claude 2>/dev/null || true"
            );
        }
    }

    #[test]
    fn scans_recorded_screens() {
        assert_eq!(
            scan(&fixture("claude_trust.txt")),
            Some(Screen::Prompt {
                summary: "Claude asks whether to trust this folder".into()
            })
        );
        assert_eq!(
            scan(&fixture("claude_permission_bash.txt")),
            Some(Screen::Prompt {
                summary: "Do you want to proceed? — touch acc.txt".into()
            })
        );
        assert_eq!(scan(&fixture("claude_done_list.txt")), Some(Screen::Idle));
        assert_eq!(scan(&fixture("claude_idle_live.txt")), Some(Screen::Idle));
        assert_eq!(
            scan(&fixture("claude_done_wrapped.txt")),
            Some(Screen::Idle)
        );
        let busy = fixture("claude_done_wrapped.txt").replace(
            "✻ Sautéed for 5s · done 1:21 p.m.",
            "✻ Sautéing… (5s · ↑ 1.2k tokens · esc to interrupt)",
        );
        assert_eq!(scan(&busy), Some(Screen::Busy));
        assert_eq!(scan("$ ls\nfoo\n"), None);
        // Wrapped at a narrow width.
        assert!(matches!(
            scan("Quick safety check: Is this a\nproject you created or one you\ntrust?"),
            Some(Screen::Prompt { .. })
        ));
    }
}

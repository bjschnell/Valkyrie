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
    fn restore(&self, command: &[String], conversation: Option<&str>) -> Option<Vec<String>> {
        crate::restore::claude(command, conversation)
    }
    fn name(&self) -> &'static str {
        "claude"
    }

    fn prepare(&self, command: &mut Vec<String>, hook_exe: &Path) {
        let exe = shell_quote(hook_exe);
        let settings = hook_settings(
            &format!("{exe} hook claude 2>/dev/null || true"),
            Some(&format!("{exe} context-hook claude 2>/dev/null || true")),
        );
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

/// `command` observes every event; `context`, at session start, prints the
/// project's decisions as added context (ADR-0007).
pub(crate) fn hook_settings(command: &str, context: Option<&str>) -> Value {
    let hook =
        |command: &str| json!({ "type": "command", "command": command, "timeout": HOOK_TIMEOUT });
    let hooks: serde_json::Map<String, Value> = EVENTS
        .iter()
        .map(|&e| {
            let mut list = vec![hook(command)];
            if matches!(e, "SessionStart" | "UserPromptSubmit")
                && let Some(context) = context
            {
                list.push(hook(context));
            }
            (e.to_string(), json!([{ "hooks": list }]))
        })
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

/// The spinner (`✽ Drizzling… (8s · ↓ 140 tokens)`, `(1m 5s · …` past a minute;
/// 2.1.292 dropped "esc to interrupt" from it) or a running tool's background hint.
/// The prompt box stays drawn under both, so without this a busy screen reads as idle.
/// Anchored to Claude's own chrome, not any text on screen: a glyph, one word ending
/// in `…`, then `(<duration>`; or the hint line on its own.
fn is_busy_line(line: &str) -> bool {
    if line.to_lowercase().contains("esc to interrupt") || line == "(ctrl+b to run in background)" {
        return true;
    }
    let Some(rest) = strip_glyph(line) else {
        return false;
    };
    // Retrying a failed request: `✻ Waiting for API response · will retry in 2m 40s`.
    if rest.starts_with("Waiting for API response") {
        return true;
    }
    let Some((word, rest)) = rest.split_once('…') else {
        return false;
    };
    // The timer is left off for a moment at the start (`· Fermenting…`).
    !word.is_empty()
        && word.chars().all(char::is_alphabetic)
        && (rest.is_empty() || rest.strip_prefix(" (").is_some_and(has_timer))
}

/// Whether one ` · `-separated part of the spinner's parentheses is the timer alone:
/// `5s · ↓ 1k tokens)`, or `running Stop hooks… 0/4 · 2m 20s · …)` at a turn's end.
fn has_timer(inner: &str) -> bool {
    let inner = inner.split_once(')').map_or(inner, |(i, _)| i);
    inner
        .split(" · ")
        .any(|part| strip_duration(part) == Some(""))
}

/// The text after a status line's leading glyph (`✻`, `✶`, `*`, `·`, …). `●` starts
/// Claude's own messages, not status.
fn strip_glyph(line: &str) -> Option<&str> {
    let mut chars = line.chars();
    chars
        .next()
        .is_some_and(|c| !c.is_alphanumeric() && !c.is_whitespace() && !"⎿│❯>-(●".contains(c))
        .then(|| chars.as_str().trim_start())
}

/// Strips a leading `5s`, `1m 5s` or `1h 2m 5s`.
fn strip_duration(s: &str) -> Option<&str> {
    let mut rest = s;
    loop {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let unit = rest[digits..].chars().next()?;
        if digits == 0 || !"dhms".contains(unit) {
            return None;
        }
        rest = &rest[digits + 1..];
        if unit == 's' {
            return Some(rest);
        }
        rest = rest.strip_prefix(' ')?;
    }
}

/// The line a finished turn leaves above the prompt box while Claude waits on agents
/// it started, and will take their results back up: `✻ Waiting for 1 background
/// agent to finish`. Earlier turns' lines stay in the scrollback, so only the one
/// right above the box counts. `✻ Cooked for 44s · done … · 1 shell still running`
/// is not one: a shell is often a server left up on purpose, and the turn is over.
fn is_background_line(line: &str) -> bool {
    strip_glyph(line)
        .is_some_and(|rest| rest.starts_with("Waiting for ") && rest.ends_with(" to finish"))
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
    // AskUserQuestion: a question over numbered options, between two borders, and
    // its own footer below. Numbered lists in the scrollback above aren't options.
    // Only as the screen's last line: tool output can quote the footer.
    let footer = lines.iter().rposition(|l| !l.is_empty());
    if let Some(footer) = footer.filter(|&i| lines[i].starts_with("Enter to select")) {
        let bottom = lines[..footer].iter().rposition(|l| is_border(l));
        let top = bottom.and_then(|b| lines[..b].iter().rposition(|l| is_border(l)));
        let dialog = &lines[top.map_or(0, |t| t + 1)..bottom.unwrap_or(footer)];
        let summary = dialog
            .iter()
            .position(|l| is_option(l))
            .and_then(|first| dialog[..first].iter().rev().find(|l| !l.is_empty()))
            .map(|l| l.trim_start_matches(['│', ' ']))
            .unwrap_or("Claude is asking a question");
        return Some(Screen::Prompt {
            summary: crate::summary::truncate(summary),
        });
    }
    if lines.iter().any(|l| is_busy_line(l)) {
        return Some(Screen::Busy);
    }
    let is_input = |l: &str| match l.strip_prefix('❯') {
        // Live Claude puts a no-break space after the glyph.
        Some(rest) => {
            rest.is_empty() || (rest.starts_with(char::is_whitespace) && !is_option(rest))
        }
        None => false,
    };
    if !lines.iter().any(|l| is_input(l)) {
        return None;
    }
    // The line above the prompt box's top border.
    let above = lines
        .iter()
        .enumerate()
        .rposition(|(i, l)| i > 0 && is_border(lines[i - 1]) && is_input(l))
        .and_then(|i| lines[..i - 1].iter().rev().find(|l| !l.is_empty()));
    if above.is_some_and(|l| is_background_line(l)) {
        return Some(Screen::Background);
    }
    Some(Screen::Idle)
}

fn is_border(line: &str) -> bool {
    !line.is_empty() && line.chars().all(|c| c == '─')
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
        Claude.prepare(&mut cmd, Path::new("/opt/it's/valk"));
        assert_eq!(cmd[0], "claude");
        assert_eq!(cmd[1], "--settings");
        assert_eq!(&cmd[3..], ["--model", "haiku"]);
        let settings: Value = serde_json::from_str(&cmd[2]).unwrap();
        for event in EVENTS {
            let hook = &settings["hooks"][event][0]["hooks"][0];
            assert_eq!(hook["type"], "command");
            assert_eq!(
                hook["command"],
                r"'/opt/it'\''s/valk' hook claude 2>/dev/null || true"
            );
        }
        let start = settings["hooks"]["SessionStart"][0]["hooks"]
            .as_array()
            .unwrap();
        assert_eq!(start.len(), 2);
        assert_eq!(
            start[1]["command"],
            r"'/opt/it'\''s/valk' context-hook claude 2>/dev/null || true"
        );
        let prompt = settings["hooks"]["UserPromptSubmit"][0]["hooks"]
            .as_array()
            .unwrap();
        assert_eq!(prompt[1]["command"], start[1]["command"]);
        assert_eq!(
            settings["hooks"]["Stop"][0]["hooks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn a_spinner_past_a_minute_is_busy() {
        // Live: every turn over a minute read as done once the timer gained minutes.
        assert_eq!(
            scan(&fixture("claude_busy_minutes.txt")),
            Some(Screen::Busy)
        );
        assert!(is_busy_line("✽ Canoodling… (6m 38s · ↓ 38.1k tokens)"));
        assert!(is_busy_line("· Scurrying… (1m 0s · ↓ 5.1k tokens)"));
        assert!(is_busy_line("✶ Brewing… (1h 2m 5s · ↓ 90k tokens)"));
        assert!(is_busy_line("✢ Pondering… (2m 3s)"));
        assert!(!is_busy_line("• synced… (4m ago)"));
        assert!(is_busy_line(
            "✻ Orchestrating… (running Stop hooks… 0/4 · 2m 20s · ↓ 11.4k tokens)"
        ));
        assert!(is_busy_line(
            "✻ Waiting for API response · will retry in 2m 40s · check your network"
        ));
        assert!(!is_busy_line("✶ Wrote… (5s timeout · retries)"));
        // Before the timer appears.
        assert!(is_busy_line("· Fermenting…"));
        assert!(!is_busy_line("● Fermenting…"));
        assert!(!is_busy_line("· Fermenting… the dough"));
        assert!(!is_busy_line("✻ Brewed for 2m 20s · done 11:01 p.m."));
    }

    #[test]
    fn waiting_on_background_agents_keeps_the_turn_open() {
        let shell = fixture("claude_background_shell.txt");
        let agents = shell.replace(
            "✻ Cooked for 44s · done 10:16 a.m. · 1 shell still running",
            "✻ Waiting for 2 background agents to finish",
        );
        assert_eq!(scan(&agents), Some(Screen::Background));
        // A shell left running is often a server: the turn itself is over.
        assert_eq!(scan(&shell), Some(Screen::Idle));
        // An earlier turn's line in the scrollback no longer counts.
        let scrolled = fixture("claude_background_scrolled.txt").replace(
            "✻ Churned for 16s · done 10:57 p.m. · 1 shell still running",
            "✻ Waiting for 1 background agent to finish",
        );
        assert_eq!(scan(&scrolled), Some(Screen::Idle));
        // Claude's own message is not status.
        assert!(!is_background_line("● Waiting for CI to finish"));
    }

    #[test]
    fn a_question_dialog_is_a_prompt() {
        assert_eq!(
            scan(&fixture("claude_question.txt")),
            Some(Screen::Prompt {
                summary: "How should the release branch go out?".into()
            })
        );
        // Tabs over several questions, a numbered list in the scrollback above.
        let Some(Screen::Prompt { summary }) = scan(&fixture("claude_question_tabs.txt")) else {
            panic!("expected a prompt");
        };
        assert!(
            summary.starts_with("The installer and macOS port are ready."),
            "{summary}"
        );
        // Tool output quoting the footer is not a dialog.
        let quoted = fixture("claude_busy_minutes.txt").replace(
            "✶ Elucidating…",
            "  Enter to select · Esc to cancel\n  1. Yes\n✶ Elucidating…",
        );
        assert_eq!(scan(&quoted), Some(Screen::Busy));
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
        assert_eq!(scan(&fixture("claude_busy_live.txt")), Some(Screen::Busy));
        assert!(is_busy_line("✶ Thinking… (12s · ↑ 1.2k tokens)"));
        assert!(!is_busy_line("✻ Sautéed for 5s · done 1:21 p.m."));
        assert!(!is_busy_line("I wrote… (see above) the file"));
        assert!(!is_busy_line("⎿  Compiling… (3s)"));
        assert!(!is_busy_line("Waiting… (5s timeout)"));
        assert!(!is_busy_line("• synced… (4s ago)"));
        assert!(!is_busy_line("Press ctrl+b to run in background, it said."));
        assert!(is_busy_line("(ctrl+b to run in background)"));
        assert!(is_busy_line("✢ Pondering… (3s)"));
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

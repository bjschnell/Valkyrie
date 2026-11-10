//! Claude Code. Hooks are registered per session with `--settings` (ADR-0005); the
//! event mapping follows Leader's live findings.

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
/// The events the context hook answers (ADR-0007), and on `PreToolUse` only
/// for the tools that open a file (ADR-0014 §2), so a shell command never
/// starts it.
const CONTEXT_EVENTS: &[(&str, Option<&str>)] = &[
    ("SessionStart", None),
    ("UserPromptSubmit", None),
    ("PreToolUse", Some(FILE_TOOLS)),
];
/// The tools whose file decisions are scoped to; the context hook keeps the
/// same list.
const FILE_TOOLS: &str = "Read|Edit|Write|MultiEdit|NotebookEdit";
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
            let mut groups = vec![json!({ "hooks": [hook(command)] })];
            let answers = CONTEXT_EVENTS.iter().find(|(event, _)| *event == e);
            if let (Some((_, matcher)), Some(context)) = (answers, context) {
                match matcher {
                    None => groups[0]["hooks"]
                        .as_array_mut()
                        .unwrap()
                        .push(hook(context)),
                    Some(matcher) => {
                        groups.push(json!({ "matcher": matcher, "hooks": [hook(context)] }))
                    }
                }
            }
            (e.to_string(), Value::Array(groups))
        })
        .collect();
    json!({ "hooks": hooks })
}

/// The hooks Valkyrie keeps in Claude Code's own settings, for a `claude` typed
/// into a Valkyrie shell (which Valkyrie never launched, so could not hand
/// `--settings`): the same two as a session's own, `valk hook` (the agent's
/// events, which drive its state, the correction reader and what the other
/// agents hear) and `valk context-hook` (the project's decisions). Both do nothing
/// outside a Valkyrie session; `--ambient` also keeps them quiet where the session
/// carries them already (`$VALK_SESSION_HOOKS`).
pub fn ambient_commands(exe: &Path) -> (String, String) {
    let exe = shell_quote(exe);
    (
        format!("{exe} hook claude --ambient 2>/dev/null || true"),
        format!("{exe} context-hook claude --ambient 2>/dev/null || true"),
    )
}

fn is_ambient(command: &str) -> bool {
    command.contains(" hook claude --ambient ")
        || command.contains(" context-hook claude --ambient ")
}

/// Claude Code's settings with Valkyrie's ambient hooks running `exe`, replacing
/// any earlier copy (from a `valk` that has moved). Everything else in the
/// settings is kept.
pub fn install_ambient(doc: Value, exe: &Path) -> Value {
    let mut doc = remove_ambient(doc);
    if !doc.is_object() {
        doc = json!({});
    }
    if !doc["hooks"].is_object() {
        doc["hooks"] = json!({});
    }
    let (observe, context) = ambient_commands(exe);
    let ours = hook_settings(&observe, Some(&context));
    for (event, ours) in ours["hooks"].as_object().into_iter().flatten() {
        let groups = &mut doc["hooks"][event];
        if !groups.is_array() {
            *groups = json!([]);
        }
        groups
            .as_array_mut()
            .unwrap()
            .extend(ours.as_array().into_iter().flatten().cloned());
    }
    // The status line is the one place Claude Code says how much of the plan is
    // used, so `valk statusline` reads it there, then runs the user's own.
    match doc.get("statusLine") {
        None => {
            doc["statusLine"] = json!({"type": "command", "command": statusline_command(exe, None)})
        }
        Some(line) if line["type"] == "command" => {
            let theirs = line["command"].as_str().map(str::to_owned);
            doc["statusLine"]["command"] = statusline_command(exe, theirs.as_deref()).into();
        }
        // A kind of status line this doesn't know: left alone, without the quota.
        Some(_) => {}
    }
    doc
}

/// Claude Code's settings without Valkyrie's ambient hooks, wherever `valk` was,
/// and with the user's own status line back as it was.
pub fn remove_ambient(doc: Value) -> Value {
    let mut doc = crate::codex::remove_where(doc, is_ambient);
    let ours = doc["statusLine"]["command"]
        .as_str()
        .and_then(unwrap_statusline);
    match ours {
        Some(Some(theirs)) => doc["statusLine"]["command"] = theirs.into(),
        Some(None) => {
            doc.as_object_mut().map(|map| map.remove("statusLine"));
        }
        None => {}
    }
    doc
}

/// `valk statusline`, then (`--then`) the user's own status line, if they had one.
pub fn statusline_command(exe: &Path, then: Option<&str>) -> String {
    let mut command = format!("{} statusline", shell_quote(exe));
    if let Some(then) = then {
        command.push_str(" --then ");
        command.push_str(&quote(then));
    }
    command
}

/// For a `statusline_command`, the user's own command it runs, if any; `None` when
/// the status line isn't Valkyrie's.
fn unwrap_statusline(command: &str) -> Option<Option<String>> {
    // Only Valkyrie's: other tools have a `statusline` too (`ccusage statusline`).
    let rest = command.strip_prefix('\'')?;
    let (exe, rest) = rest.split_once("' statusline")?;
    let name = exe.rsplit(['/', '\\']).next()?;
    if name != "valk" && name != "valk.exe" {
        return None;
    }
    if rest.is_empty() {
        return Some(None);
    }
    let then = rest.strip_prefix(" --then '")?.strip_suffix('\'')?;
    Some(Some(then.replace(r"'\''", "'")))
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The plan's windows in a status line payload (`rate_limits`), shortest first:
/// each `{used_percentage, resets_at}` Claude Code gives, whatever it's called.
/// `None` where it gives none (an API key, Bedrock, an older Claude Code).
pub fn statusline_quota(payload: &Value) -> Option<Vec<valkyrie_proto::QuotaLimit>> {
    let mut limits: Vec<_> = payload["rate_limits"]
        .as_object()?
        .iter()
        .filter_map(|(name, window)| {
            let used = window["used_percentage"].as_f64()?;
            Some(valkyrie_proto::QuotaLimit {
                name: name.clone(),
                used: used.round().clamp(0.0, 100.0) as u8,
                resets_at: window["resets_at"].as_u64(),
            })
        })
        .collect();
    limits.sort_by(|a, b| (a.name != "five_hour", &a.name).cmp(&(b.name != "five_hour", &b.name)));
    (!limits.is_empty()).then_some(limits)
}

pub(crate) fn shell_quote(path: &Path) -> String {
    let path = path.to_string_lossy();
    // Claude Code on Windows runs hooks in Git Bash; `C:/…` reads the same there
    // and in any other shell it might use, where `C:\…` may not.
    let path = if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.into_owned()
    };
    format!("'{}'", path.replace('\'', r"'\''"))
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
        // A policy blocked this stop (ADR-0013 §5): the turn goes on.
        "Stop" if !p["valk_continued"].is_null() => return None,
        // `ci-fix` gave up on a red pull request (ADR-0013 §3): the user's now.
        "Stop" if !p["valk_ci_fix_spent"].is_null() => {
            let spent = &p["valk_ci_fix_spent"];
            let url = spent["pr"].as_str().unwrap_or("");
            let pr = url
                .rsplit_once("/pull/")
                .map_or("its pull request".to_owned(), |(_, n)| format!("PR #{n}"));
            E::TurnFailed {
                summary: format!(
                    "CI still red on {pr} after {} fixes",
                    spent["blocks"].as_u64().unwrap_or(0)
                ),
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
        // A stop a policy kept going isn't a turn end.
        assert_eq!(
            n(json!({"hook_event_name": "Stop", "valk_continued": {"policy": 3}})),
            None
        );
        // One `ci-fix` gave up on: the user's, not finished.
        assert_eq!(
            n(
                json!({"hook_event_name": "Stop", "last_assistant_message": "Still red.",
                "valk_ci_fix_spent": {"pr": "https://github.com/o/r/pull/93", "blocks": 2}})
            ),
            Some(E::TurnFailed {
                summary: "CI still red on PR #93 after 2 fixes".into()
            })
        );
    }

    #[test]
    fn ambient_hooks_are_idempotent_follow_a_moved_binary_and_keep_the_rest() {
        let (observe, context) = ambient_commands(Path::new("/old/valk"));
        assert_eq!(
            observe,
            "'/old/valk' hook claude --ambient 2>/dev/null || true"
        );
        assert_eq!(
            context,
            "'/old/valk' context-hook claude --ambient 2>/dev/null || true"
        );
        let user = json!({
            "model": "opus",
            "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify"}]}],
                      "SessionStart": [{"hooks": [{"type": "command", "command": "mine"}]}]}
        });
        let once = install_ambient(user.clone(), Path::new("/old/valk"));
        assert_eq!(install_ambient(once.clone(), Path::new("/old/valk")), once);
        let again = install_ambient(once, Path::new("/new/valk"));
        assert!(!again.to_string().contains("/old/valk"));
        // Theirs and ours: the observer and the context hook at session start.
        assert_eq!(
            again["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            "mine"
        );
        assert_eq!(
            again["hooks"]["SessionStart"][1]["hooks"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(again["hooks"]["Stop"].as_array().unwrap().len(), 2);
        // The observer, and the context hook for the tools that open a file.
        assert_eq!(again["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
        assert_eq!(again["hooks"]["PreToolUse"][1]["matcher"], FILE_TOOLS);
        assert_eq!(again["model"], "opus");
        assert_eq!(again["statusLine"]["command"], "'/new/valk' statusline");
        assert_eq!(remove_ambient(again), user);
    }

    #[test]
    fn the_status_line_wraps_the_users_own_and_gives_it_back() {
        let user = json!({
            "statusLine": {"type": "command", "command": "node 'my hud'.mjs", "padding": 0}
        });
        let once = install_ambient(user.clone(), Path::new("/old/valk"));
        assert_eq!(
            once["statusLine"],
            json!({"type": "command", "padding": 0,
                   "command": r"'/old/valk' statusline --then 'node '\''my hud'\''.mjs'"})
        );
        assert_eq!(install_ambient(once.clone(), Path::new("/old/valk")), once);
        let moved = install_ambient(once, Path::new("/new/valk"));
        assert!(!moved.to_string().contains("/old/valk"));
        let back = remove_ambient(moved);
        assert_eq!(back["statusLine"], user["statusLine"]);
        // Another tool's own `statusline` subcommand is theirs, not ours.
        for theirs in [
            "'/home/u/.bun/bin/ccusage' statusline",
            "'/opt/tool' statusline --then 'x'",
        ] {
            let doc = json!({"statusLine": {"type": "command", "command": theirs}});
            assert_eq!(remove_ambient(doc.clone()), doc);
            let wrapped = install_ambient(doc, Path::new("/v/valk"));
            assert_eq!(remove_ambient(wrapped)["statusLine"]["command"], theirs);
        }
        // One it can't run before its own is left as it is.
        let other = json!({"statusLine": {"type": "static", "text": "hi"}});
        assert_eq!(
            install_ambient(other.clone(), Path::new("/v"))["statusLine"],
            other["statusLine"]
        );
    }

    #[test]
    fn quota_reads_every_window_the_status_line_gives() {
        let payload = json!({"rate_limits": {
            "seven_day": {"used_percentage": 10, "resets_at": 1794754800},
            "five_hour": {"used_percentage": 26.6, "resets_at": 1794276000},
            "seven_day_opus": {"used_percentage": 140},
            "seven_day_oauth_apps": null,
        }});
        let limits = statusline_quota(&payload).unwrap();
        let got: Vec<_> = limits
            .iter()
            .map(|l| (l.name.as_str(), l.used, l.resets_at))
            .collect();
        assert_eq!(
            got,
            [
                ("five_hour", 27, Some(1794276000)),
                ("seven_day", 10, Some(1794754800)),
                ("seven_day_opus", 100, None),
            ]
        );
        assert_eq!(statusline_quota(&json!({"rate_limits": null})), None);
        assert_eq!(statusline_quota(&json!({"rate_limits": {}})), None);
        assert_eq!(statusline_quota(&json!({"model": "x"})), None);
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
        // On a tool call, only one that opens a file starts the context hook
        // (ADR-0014 §2); every call still reaches the observer.
        let pre = settings["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2);
        assert!(pre[0].get("matcher").is_none());
        assert_eq!(pre[0]["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(pre[1]["matcher"], "Read|Edit|Write|MultiEdit|NotebookEdit");
        assert_eq!(pre[1]["hooks"][0]["command"], start[1]["command"]);
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

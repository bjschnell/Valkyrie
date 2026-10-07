//! One-line "what it needs / what it finished" text (DESIGN §14.4).
//!
//! Semantics ported from Leader (`queue/summarize.py`, `hooks/claude_hook.py`):
//! permission → `Permission: <tool> <command>`, question → its text, finished turn →
//! the most conclusion-like line of the final reply.

use serde_json::Value;

pub const MAX_LEN: usize = 160;

/// First non-blank line, whitespace collapsed, truncated.
pub fn one_line(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(truncate(
        &line.split_whitespace().collect::<Vec<_>>().join(" "),
    ))
}

pub fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_LEN {
        return text.to_owned();
    }
    let cut: String = text.chars().take(MAX_LEN - 1).collect();
    format!("{}…", cut.trim_end())
}

/// `Permission: Bash touch acc.txt`, from a PreToolUse/PermissionRequest payload.
pub fn describe_tool(payload: &Value) -> String {
    let tool = payload["tool_name"].as_str().unwrap_or("tool");
    let detail = [
        "command",
        "file_path",
        "url",
        "pattern",
        "description",
        "prompt",
    ]
    .iter()
    .filter_map(|k| payload["tool_input"][k].as_str())
    .find(|s| !s.trim().is_empty());
    match detail.and_then(one_line) {
        Some(detail) => truncate(&format!("Permission: {tool} {detail}")),
        None => format!("Permission: {tool}"),
    }
}

/// The first question of an AskUserQuestion call.
pub fn question(payload: &Value) -> Option<String> {
    let input = &payload["tool_input"];
    input["questions"][0]["question"]
        .as_str()
        .or_else(|| input["question"].as_str())
        .and_then(one_line)
}

/// The most summary-like line of an assistant message: the last prose line that
/// reads like a conclusion, else the intro of a list-heavy answer, else the last line.
pub fn pick_summary_line(text: &str) -> Option<String> {
    let lines: Vec<String> = paragraph_lines(text)
        .iter()
        .map(|l| clean(l))
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let prose: Vec<&String> = lines.iter().filter(|l| !is_list_item(l)).collect();
    if let Some(line) = prose.iter().rev().find(|l| is_conclusion(l)) {
        return Some(truncate(line));
    }
    let line = match prose.first() {
        Some(first) if prose.len() < lines.len() => first,
        Some(_) => prose[prose.len() - 1],
        None => &lines[0],
    };
    Some(truncate(line))
}

/// Joins hard-wrapped continuation lines (Claude indents them by two spaces).
fn paragraph_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.lines() {
        if raw.trim().is_empty() {
            out.push(String::new());
            continue;
        }
        let continues = raw.starts_with("  ")
            && !is_list_item(raw)
            && out.last().is_some_and(|l| !l.is_empty());
        match out.last_mut() {
            Some(last) if continues => {
                last.truncate(last.trim_end().len());
                last.push(' ');
                last.push_str(raw.trim());
            }
            _ => out.push(raw.trim_end().to_owned()),
        }
    }
    out
}

fn clean(line: &str) -> String {
    let line = line.replace("**", "").replace("__", "").replace('`', "");
    let line = line.trim().trim_start_matches('#').trim();
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `1.` `2)` `-` `*` `•` `+` `[ ]` `[x]` followed by whitespace.
fn is_list_item(line: &str) -> bool {
    let s = line.trim_start();
    let rest = if let Some(r) = s.strip_prefix(['-', '*', '•', '+']) {
        r
    } else if let Some(r) = ["[ ]", "[x]", "[X]"].iter().find_map(|p| s.strip_prefix(p)) {
        r
    } else {
        let digits = s.len() - s.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            return false;
        }
        match s[digits..].strip_prefix(['.', ')']) {
            Some(r) => r,
            None => return false,
        }
    };
    rest.starts_with(char::is_whitespace)
}

fn is_conclusion(line: &str) -> bool {
    const WORDS: &[&str] = &[
        "done",
        "all",
        "fixed",
        "added",
        "implemented",
        "updated",
        "created",
        "completed",
        "finished",
        "shipped",
        "summary",
        "result",
        "test",
        "tests",
        "build",
        "i've",
        "i have",
        "the fix",
        "the change",
        "the migration",
        "the refactor",
        "it now",
        "it is",
        "this now",
        "this is",
        "no changes",
        "no issues",
        "no errors",
    ];
    let lower = line.to_lowercase();
    WORDS.iter().any(|w| {
        lower.starts_with(w)
            && !lower[w.len()..]
                .chars()
                .next()
                .is_some_and(char::is_alphanumeric)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn permission_names_the_tool_and_its_command() {
        let p = json!({"tool_name": "Bash", "tool_input": {"command": "touch acc.txt\nmore", "description": "x"}});
        assert_eq!(describe_tool(&p), "Permission: Bash touch acc.txt");
        assert_eq!(
            describe_tool(&json!({"tool_name": "Edit"})),
            "Permission: Edit"
        );
    }

    #[test]
    fn question_text_from_either_shape() {
        let p = json!({"tool_input": {"questions": [{"question": "Which DB?"}]}});
        assert_eq!(question(&p).as_deref(), Some("Which DB?"));
        let p = json!({"tool_input": {"question": "Ok?"}});
        assert_eq!(question(&p).as_deref(), Some("Ok?"));
        assert_eq!(question(&json!({})), None);
    }

    #[test]
    fn picks_the_conclusion_line() {
        let text = "I looked at the parser.\n\n- changed a\n- changed b\n\nDone: parser handles **CRLF** now.\nLet me know.";
        assert_eq!(
            pick_summary_line(text).as_deref(),
            Some("Done: parser handles CRLF now.")
        );
    }

    #[test]
    fn list_heavy_answer_uses_its_intro_and_plain_answer_its_last_line() {
        assert_eq!(
            pick_summary_line("Three options:\n1. a\n2. b").as_deref(),
            Some("Three options:")
        );
        assert_eq!(
            pick_summary_line("Canberra is the capital.\nIt was chosen in 1908.").as_deref(),
            Some("It was chosen in 1908.")
        );
        assert_eq!(pick_summary_line("1. a\n2. b").as_deref(), Some("1. a"));
        assert_eq!(pick_summary_line("  \n"), None);
    }

    #[test]
    fn rejoins_wrapped_lines_and_respects_word_boundaries() {
        let text = "Alligators are big.\nThe fix is in place and\n  covers both paths.";
        assert_eq!(
            pick_summary_line(text).as_deref(),
            Some("The fix is in place and covers both paths.")
        );
        assert!(!is_conclusion("allocations grew"));
        assert!(is_conclusion("All green."));
    }

    #[test]
    fn truncates_long_lines() {
        let long = "x".repeat(400);
        let out = one_line(&long).unwrap();
        assert_eq!(out.chars().count(), MAX_LEN);
        assert!(out.ends_with('…'));
    }
}

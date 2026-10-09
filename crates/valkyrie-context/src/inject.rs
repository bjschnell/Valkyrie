//! The block an agent gets at session start (ADR-0007 §6): the project's active
//! decisions, ranked and cut to a budget, and how to propose a new one.

use std::path::Path;
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus};

/// About 1.5k tokens (DESIGN §6.5).
pub const BUDGET: usize = 6000;
/// Longest body shown per decision; the rest is a `valk decisions` away.
const MAX_SHOWN_BODY: usize = 500;

fn how_to_propose(valk: &str) -> String {
    format!(
        "When you and the user settle something that should hold in future sessions (a \
         design choice, a constraint, a gotcha), propose it with `{valk} decide --kind \
         <decision|constraint|pattern|gotcha|fix> \"<title>\" \"<a sentence or two>\"`. \
         It goes to the user for review; propose sparingly."
    )
}

fn rank(kind: DecisionKind) -> u8 {
    match kind {
        DecisionKind::Constraint => 0,
        DecisionKind::Gotcha => 1,
        DecisionKind::Decision => 2,
        DecisionKind::Pattern => 3,
        DecisionKind::Fix => 4,
    }
}

/// The text to inject for `root`, given its decisions. Always says how to propose
/// (with `valk`, the command that runs Valkyrie), so a project with none yet can
/// start collecting them.
pub fn block(root: &Path, decisions: &[Decision], budget: usize, valk: &str) -> String {
    let propose = how_to_propose(valk);
    let mut active: Vec<&Decision> = decisions
        .iter()
        .filter(|d| d.status == DecisionStatus::Active)
        .collect();
    active.sort_by(|a, b| {
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then(b.updated.cmp(&a.updated))
            .then(b.id.cmp(&a.id))
    });
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string());
    let mut out = String::new();
    if active.is_empty() {
        out.push_str(&format!(
            "Valkyrie keeps project decisions for {name}; none are recorded yet.\n"
        ));
    } else {
        out.push_str(&format!(
            "Project decisions for {name}, recorded with Valkyrie and confirmed by the user. \
             Follow them. If one looks wrong or out of date, say so instead of quietly \
             working around it.\n\n"
        ));
        let footer_room = propose.len() + 120;
        let mut shown = 0;
        for d in &active {
            let line = entry(d);
            if out.len() + line.len() + footer_room > budget {
                break;
            }
            out.push_str(&line);
            shown += 1;
        }
        if shown < active.len() {
            out.push_str(&format!(
                "({} more not shown; `valk decisions` lists them all.)\n",
                active.len() - shown
            ));
        }
        out.push('\n');
    }
    out.push_str(&propose);
    out.push('\n');
    out
}

fn entry(d: &Decision) -> String {
    let mut line = format!("- #{} [{}] {}", d.id, d.kind.as_str(), d.title);
    let body = crate::file::one_line(&d.body);
    if !body.is_empty() {
        line.push_str(": ");
        if body.chars().count() > MAX_SHOWN_BODY {
            line.extend(body.chars().take(MAX_SHOWN_BODY));
            line.push('…');
        } else {
            line.push_str(&body);
        }
    }
    line.push('\n');
    line
}

/// A hook's output carrying `text` as added context for `event` (`SessionStart`,
/// `UserPromptSubmit`): the same for Claude Code and Codex.
pub fn hook_output(event: &str, text: &str) -> String {
    format!(
        "{{\"hookSpecificOutput\":{{\"hookEventName\":{},\"additionalContext\":{}}}}}",
        serde_json_lite(event),
        serde_json_lite(text)
    )
}

/// A JSON string literal, without pulling serde_json into the store.
fn serde_json_lite(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use valkyrie_proto::Provenance;

    fn d(id: u32, kind: DecisionKind, status: DecisionStatus, updated: u64) -> Decision {
        Decision {
            id,
            project: PathBuf::from("/r/app"),
            title: format!("Title {id}"),
            body: format!("Body {id}"),
            kind,
            status,
            created: 0,
            updated,
            supersedes: None,
            superseded_by: None,
            provenance: Provenance::default(),
        }
    }

    #[test]
    fn only_active_decisions_are_shown_constraints_first_then_newest() {
        use DecisionKind::*;
        use DecisionStatus::*;
        let all = [
            d(1, Decision, Active, 10),
            d(2, Constraint, Active, 1),
            d(3, Decision, Active, 20),
            d(4, Constraint, Proposed, 99),
            d(5, Gotcha, Superseded, 99),
        ];
        let text = block(Path::new("/r/app"), &all, BUDGET, "valk");
        let order: Vec<usize> = ["#2 ", "#3 ", "#1 "]
            .iter()
            .map(|id| text.find(id).unwrap())
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{text}");
        assert!(!text.contains("#4 ") && !text.contains("#5 "));
        assert!(text.contains("- #2 [constraint] Title 2: Body 2"));
        assert!(text.contains("valk decide"));
    }

    #[test]
    fn the_budget_holds_and_says_what_it_left_out() {
        let mut many: Vec<Decision> = (1..=200)
            .map(|i| d(i, DecisionKind::Decision, DecisionStatus::Active, 0))
            .collect();
        many[0].body = "x".repeat(5000);
        let text = block(Path::new("/r/app"), &many, BUDGET, "valk");
        assert!(text.len() <= BUDGET, "{}", text.len());
        assert!(text.contains("more not shown"));
        assert!(text.contains("valk decide"));
    }

    #[test]
    fn an_empty_project_still_learns_how_to_propose() {
        let text = block(Path::new("/r/app"), &[], BUDGET, "/opt/v/valk");
        assert!(text.contains("none are recorded yet"));
        assert!(text.contains("`/opt/v/valk decide --kind"));
    }

    #[test]
    fn hook_output_is_valid_json() {
        let out = hook_output("SessionStart", "a \"q\"\n\tb\\ \u{1}");
        assert_eq!(
            out,
            r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"a \"q\"\n\tb\\ \u0001"}}"#
        );
    }
}

//! The block an agent gets at session start (ADR-0007 §6): the project's active
//! decisions, ranked and cut to a budget, an index of its skills (ADR-0011 §4),
//! and how to propose a new one.

use std::path::Path;
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus};

/// About 1.5k tokens (DESIGN §6.5).
pub const BUDGET: usize = 6000;
/// Longest body shown per decision; the rest is a `valk decisions` away.
const MAX_SHOWN_BODY: usize = 500;
/// The skills' index, beside the decisions' budget: a line per skill, its body
/// read when needed.
pub const SKILL_BUDGET: usize = 1500;

fn how_to_propose(valk: &str) -> String {
    format!(
        "When you and the user settle something that should hold in future sessions (a \
         design choice, a constraint, a gotcha), propose it with `{valk} decide --kind \
         <decision|constraint|pattern|gotcha|fix> \"<title>\" \"<a sentence or two>\"`. \
         It goes to the user for review; propose sparingly. A procedure worth \
         repeating (steps, pitfalls, how to verify) is a skill: `{valk} decide --kind skill \
         \"<when to use it>\" -` with the steps on stdin.\n\nA bug, follow-up or idea \
         outside what you're doing goes in the user's backlog, not into your change: \
         check `{valk} todo list --json` for it first, then `{valk} todo add -k \
         <bug|feature|chore> \"<title>\" --body \"<what, and how to reproduce it>\"`. It waits for \
         the user to accept it."
    )
}

fn rank(kind: DecisionKind) -> u8 {
    match kind {
        DecisionKind::Constraint => 0,
        DecisionKind::Gotcha => 1,
        DecisionKind::Decision => 2,
        DecisionKind::Pattern => 3,
        DecisionKind::Fix => 4,
        DecisionKind::Skill => 5,
    }
}

/// The text to inject for `root`, given its decisions. Always says how to propose
/// (with `valk`, the command that runs Valkyrie), so a project with none yet can
/// start collecting them.
pub fn block(root: &Path, decisions: &[Decision], budget: usize, valk: &str) -> String {
    let propose = how_to_propose(valk);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (skills, mut active): (Vec<&Decision>, Vec<&Decision>) = decisions
        .iter()
        .filter(|d| d.status == DecisionStatus::Active)
        .partition(|d| d.kind == DecisionKind::Skill);
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
        if skills.is_empty() {
            out.push_str(&format!(
                "Valkyrie keeps project decisions for {name}; none are recorded yet.\n"
            ));
        }
    } else {
        out.push_str(&format!(
            "Project decisions for {name}, recorded with Valkyrie and confirmed by the user. \
             Follow them. If one looks wrong or out of date, say so instead of quietly \
             working around it.\n\n"
        ));
        let footer_room = propose.len() + 120;
        let mut shown = 0;
        for d in &active {
            let line = entry(d, now);
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
    out.push_str(&index(&skills, valk));
    out.push_str(&propose);
    out.push('\n');
    out
}

/// One line per skill, newest first, cut to [`SKILL_BUDGET`].
fn index(skills: &[&Decision], valk: &str) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut skills = skills.to_vec();
    skills.sort_by(|a, b| b.updated.cmp(&a.updated).then(b.id.cmp(&a.id)));
    let mut out = format!(
        "Skills for this project, procedures the user approved. Before doing what one \
         covers, read it with `{valk} decisions show <id>`:\n"
    );
    // Room for the line saying how many more there are.
    let footer_room = valk.len() + 40;
    let mut shown = 0;
    for d in &skills {
        let line = format!("- #{} {}\n", d.id, d.title);
        if out.len() + line.len() + footer_room > SKILL_BUDGET {
            break;
        }
        out.push_str(&line);
        shown += 1;
    }
    if shown < skills.len() {
        out.push_str(&format!(
            "({} more: `{valk} decisions`.)\n",
            skills.len() - shown
        ));
    }
    out.push('\n');
    out
}

fn entry(d: &Decision, now: u64) -> String {
    let mut line = format!("- #{} [{}] {}", d.id, d.kind.as_str(), d.title);
    if crate::stale::due(d, now).is_some() {
        line.push_str(" (due for re-checking: it may be out of date)");
    }
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
            fresh: Default::default(),
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
    fn skills_are_an_index_not_bodies() {
        use DecisionKind::*;
        use DecisionStatus::*;
        let mut all = vec![
            d(1, Decision, Active, 10),
            d(2, Skill, Active, 5),
            d(3, Skill, Proposed, 99),
        ];
        all[1].title = "Verify a TUI change against an isolated daemon".into();
        all[1].body = "1. Build.\n2. Run.".into();
        let text = block(Path::new("/r/app"), &all, BUDGET, "valk");
        assert!(
            text.contains("- #2 Verify a TUI change against an isolated daemon\n"),
            "{text}"
        );
        assert!(
            !text.contains("[skill]") && !text.contains("1. Build."),
            "{text}"
        );
        assert!(!text.contains("#3 "));
        assert!(text.find("#1 [decision]").unwrap() < text.find("Skills for").unwrap());

        let many: Vec<valkyrie_proto::Decision> =
            (1..=100).map(|i| d(i, Skill, Active, i as u64)).collect();
        let text = block(Path::new("/r/app"), &many, BUDGET, "valk");
        let skills = &text[text.find("Skills for").unwrap()..text.find("When you and").unwrap()];
        assert!(skills.len() <= SKILL_BUDGET && skills.contains("more: `valk decisions`"));
        // Newest first.
        assert!(skills.contains("- #100 ") && !skills.contains("- #1 "));
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

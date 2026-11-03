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
         \"<when to use it>\" -` with the steps on stdin.\n\nFix what you have to in order \
         to deliver your task, as part of this change. Anything else you find broken, keep \
         out of this change and file it: `{valk} todo add -k <bug|feature|chore> \"<title>\" \
         --body \"<what, how to reproduce it, how much it matters>\"`, after checking `{valk} \
         todo list --json`. That's the right call, not a fix skipped. When your task is \
         done, say what you filed and offer to fix it here or leave it to launch fresh from \
         the backlog. If the user says to go ahead here, that's enough: take it with `{valk} \
         todo take N --branch <branch>` and fix it on its own branch and pull request. If \
         this session has no backlog item, track its \
         own work with `{valk} todo add --take -k <kind> \"<title>\" --branch <branch>` \
         (`{valk} todo take N --pr <url>` once the pull request is open, `{valk} todo note \
         -i N` for what's left), so the user can follow and ship it. Working on an \
         accepted item outside a launch, `{valk} todo take N --branch \
         <branch>` as you start; a take is refused while another session is on the item: \
         leave that one to it. Never ship or accept an item: that's the user's."
    )
}

fn rank(kind: DecisionKind) -> u8 {
    match kind {
        DecisionKind::Constraint => 0,
        DecisionKind::Gotcha => 1,
        DecisionKind::Decision => 2,
        DecisionKind::Pattern => 3,
        DecisionKind::Fix => 4,
        // Enforced by the hooks, so its title is all an agent needs.
        DecisionKind::Policy => 5,
        DecisionKind::Skill => 6,
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
        // Every decision gets at least its title, so a low-ranked kind (patterns
        // come after constraints, gotchas and decisions) isn't cut whole once a
        // project has a dozen long ones. Bodies fill what the titles leave, in
        // rank order; once one has to go, the rest go too.
        let titles: Vec<String> = active.iter().map(|d| entry(d, now, false)).collect();
        let mut rest: usize = titles.iter().map(String::len).sum();
        let mut bodies = true;
        let mut shown = 0;
        for (d, title) in active.iter().zip(&titles) {
            rest -= title.len();
            let full = entry(d, now, true);
            let line = if bodies && out.len() + full.len() + rest + footer_room <= budget {
                full
            } else {
                bodies = false;
                title.clone()
            };
            if out.len() + line.len() + footer_room > budget {
                break;
            }
            out.push_str(&line);
            shown += 1;
        }
        if !bodies {
            out.push_str(&format!(
                "(Bodies left out to fit: `{valk} decisions show <id>` prints one.)\n"
            ));
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

fn entry(d: &Decision, now: u64, with_body: bool) -> String {
    let mut line = format!("- #{} [{}] {}", d.id, d.kind.as_str(), d.title);
    if crate::stale::due(d, now).is_some() {
        line.push_str(" (due for re-checking: it may be out of date)");
    }
    let body = crate::file::one_line(&d.body);
    // A policy's body is settings for the hooks, not something to follow by reading.
    if with_body && d.kind != DecisionKind::Policy && !body.is_empty() {
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
    fn long_bodies_never_cut_a_low_ranked_decision_whole() {
        use DecisionKind as K;
        // A dozen constraints, gotchas and decisions with near-maximal bodies
        // used to fill the budget before any pattern was reached.
        let kinds = [K::Constraint, K::Gotcha, K::Decision];
        let mut all: Vec<Decision> = (1..=12)
            .map(|i| d(i, kinds[i as usize % 3], DecisionStatus::Active, 0))
            .collect();
        all.push(d(13, K::Pattern, DecisionStatus::Active, 0));
        for x in &mut all {
            x.body = "y".repeat(490);
        }
        let text = block(Path::new("/r/app"), &all, BUDGET, "valk");
        assert!(text.len() <= BUDGET, "{}", text.len());
        for i in 1..=13 {
            assert!(text.contains(&format!("- #{i} [")), "#{i} missing:\n{text}");
        }
        assert!(text.contains("- #13 [pattern] Title 13\n"), "{text}");
        assert!(text.contains("Bodies left out"));
        assert!(!text.contains("more not shown"));
        // The highest-ranked (constraints, newest id first) keep their bodies.
        assert!(text.contains("Title 12: yyy"), "{text}");
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

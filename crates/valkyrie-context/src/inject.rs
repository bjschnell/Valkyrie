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
         <decision|constraint|pattern|gotcha|fix> \"<title>\" \"<a sentence or two>\"`, \
         with `--scope <glob>` for the files it's about (agents then get it when they open \
         one). It goes to the user for review; propose sparingly. A procedure worth \
         repeating (steps, pitfalls, how to verify) is a skill: `{valk} decide --kind skill \
         \"<when to use it>\" -` with the steps on stdin.\n\nFix what you have to in order \
         to deliver your task, as part of this change. Anything else you find broken, keep \
         out of this change and file it: `{valk} todo add -k <bug|feature|chore> \"<title>\" \
         --body \"<what, how to reproduce it, how much it matters>\"`, after checking `{valk} \
         todo list --json`. That's the right call, not a fix skipped. When your task is \
         done, say in a line what you filed (number and title). Offer to fix a find here \
         only when it's worth acting on now: it blocks the user's next step, or it's a real \
         bug the user would want dealt with before moving on. Otherwise leave it on the \
         backlog and don't ask. If the user says to go ahead here, that's enough: take it \
         with `{valk} todo take N --branch <branch>` and fix it on its own branch and pull \
         request. If \
         this session isn't on a backlog item and the user asks for work, check `{valk} todo \
         list --json` for an open item it matches; if one does, say which and take it with \
         `{valk} todo take N --branch <branch>` as you start. A take is refused while another \
         session is on the item: leave that one to it. If none matches, track the work with \
         `{valk} todo add --take -k <kind> \"<title>\" --branch <branch>`. Either way, \
         `{valk} todo take N --pr <url>` once the pull request is open and `{valk} todo note \
         -i N` for what's left, so the user can follow and ship it. Never ship, accept or \
         merge on your own: those are the user's.\n\nYou run in a Valkyrie tab: `{valk} \
         pane` splits it and runs, reads and waits on commands in the other panes \
         (`{valk} pane --help`)."
    )
}

pub(crate) fn rank(kind: DecisionKind) -> u8 {
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
    laid_out(root, decisions, budget, valk).0
}

/// The decisions a block showed: by id, with their body or as a title only.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Shown {
    /// Shown with their body (or with no body to show).
    pub whole: Vec<u32>,
    /// Shown as a title line only.
    pub titles: Vec<u32>,
}

/// [`block`], and which decisions it showed and how, for the serving log
/// (ADR-0014 §5). Skills aren't in it: their reads are measured instead.
pub fn laid_out(root: &Path, decisions: &[Decision], budget: usize, valk: &str) -> (String, Shown) {
    let mut seen = Shown::default();
    let propose = how_to_propose(valk);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (skills, mut active): (Vec<&Decision>, Vec<&Decision>) = decisions
        .iter()
        .filter(|d| d.status == DecisionStatus::Active)
        .partition(|d| d.kind == DecisionKind::Skill);
    // What holds everywhere comes first: constraints, and what the user pinned.
    active.sort_by(|a, b| {
        everywhere(b)
            .cmp(&everywhere(a))
            .then(rank(a.kind).cmp(&rank(b.kind)))
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
             working around it. Constraints and the ones the user pinned are in full; the \
             rest are titles. Before you start, read the ones that bear on your task with \
             `{valk} decisions show <id>` (MCP `get_decision`), or search them all with \
             `{valk} decisions search <words>` (MCP `search_decisions`).\n\n"
        ));
        let footer_room = propose.len() + 120;
        // An index (ADR-0014 §1): every decision gets its title, and only what
        // holds everywhere its body, in rank order, from what the titles leave;
        // once one has to go, the rest go too.
        let titles: Vec<String> = active.iter().map(|d| entry(d, now, false)).collect();
        let mut rest: usize = titles.iter().map(String::len).sum();
        let mut bodies = true;
        let mut shown = 0;
        for (d, title) in active.iter().zip(&titles) {
            rest -= title.len();
            let full = entry(d, now, true);
            let fits = out.len() + full.len() + rest + footer_room <= budget;
            let body = everywhere(d) && bodies && fits;
            if everywhere(d) && !body {
                bodies = false;
            }
            // A title is all of a decision without a body.
            let whole = body || full == *title;
            let line = if body { full } else { title.clone() };
            if out.len() + line.len() + footer_room > budget {
                break;
            }
            out.push_str(&line);
            if whole {
                seen.whole.push(d.id);
            } else {
                seen.titles.push(d.id);
            }
            shown += 1;
        }
        if !bodies {
            out.push_str(&format!(
                "(Bodies left out to fit: `{valk} decisions show <id>` prints one.)\n"
            ));
        }
        if shown < active.len() {
            out.push_str(&format!(
                "({} more not shown; `{valk} decisions` lists them all.)\n",
                active.len() - shown
            ));
        }
        out.push('\n');
    }
    out.push_str(&index(&skills, valk));
    out.push_str(&propose);
    out.push('\n');
    (out, seen)
}

/// Whether a decision's body goes in every block: a constraint, or pinned.
fn everywhere(d: &Decision) -> bool {
    match d.kind {
        DecisionKind::Constraint => true,
        DecisionKind::Skill | DecisionKind::Policy => false,
        _ => d.pinned,
    }
}

/// Most of the user's habits listed; they're few, and each is a title.
const MAX_HABITS: usize = 10;

/// The user's active habits (ADR-0016 §1), which hold in every project: a title a
/// line, put before a project's block. Empty when there are none.
pub fn habits(user: &[Decision], valk: &str) -> String {
    let mut active: Vec<&Decision> = user
        .iter()
        .filter(|d| d.status == DecisionStatus::Active)
        .collect();
    if active.is_empty() {
        return String::new();
    }
    active.sort_by(|a, b| b.updated.cmp(&a.updated).then(b.id.cmp(&a.id)));
    let mut out = format!(
        "The user's habits, which hold in every project (`{valk} decisions --user show <id>` \
         prints one):\n"
    );
    for d in active.iter().take(MAX_HABITS) {
        out.push_str(&format!("- #{} [{}] {}\n", d.id, d.kind.as_str(), d.title));
    }
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
            pinned: false,
            scope: Vec::new(),
            proposed_scope: None,
            scope_asked: false,
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
    fn only_constraints_and_pins_get_bodies_and_every_decision_its_title() {
        use DecisionKind as K;
        let kinds = [K::Constraint, K::Gotcha, K::Decision];
        let mut all: Vec<Decision> = (1..=12)
            .map(|i| d(i, kinds[i as usize % 3], DecisionStatus::Active, 0))
            .collect();
        all.push(d(13, K::Pattern, DecisionStatus::Active, 0));
        all[12].pinned = true;
        for x in &mut all {
            x.body = "y".repeat(490);
        }
        let (text, shown) = laid_out(Path::new("/r/app"), &all, BUDGET, "valk");
        assert!(text.len() <= BUDGET, "{}", text.len());
        for i in 1..=13 {
            assert!(text.contains(&format!("- #{i} [")), "#{i} missing:\n{text}");
        }
        // Constraints (3, 6, 9, 12) and the pinned pattern in full, first.
        let mut whole = shown.whole.clone();
        whole.sort();
        assert_eq!(whole, [3, 6, 9, 12, 13]);
        assert!(text.find("Title 13: yyy").unwrap() < text.find("Title 1\n").unwrap());
        assert!(text.contains("- #1 [gotcha] Title 1\n"), "{text}");
        assert!(!text.contains("Bodies left out") && !text.contains("more not shown"));
        assert!(text.contains("valk decisions show <id>"));
        for id in &shown.titles {
            assert!(text.contains(&format!("Title {id}\n")), "#{id}");
        }
    }

    #[test]
    fn too_many_pins_lose_their_bodies_in_rank_order_and_say_so() {
        let mut all: Vec<Decision> = (1..=20)
            .map(|i| d(i, DecisionKind::Decision, DecisionStatus::Active, 0))
            .collect();
        for x in &mut all {
            x.pinned = true;
            x.body = "z".repeat(490);
        }
        let (text, shown) = laid_out(Path::new("/r/app"), &all, BUDGET, "valk");
        assert!(text.len() <= BUDGET, "{}", text.len());
        assert!(!shown.whole.is_empty() && !shown.titles.is_empty());
        assert_eq!(shown.whole.len() + shown.titles.len(), 20);
        assert!(text.contains("Bodies left out"));
    }

    #[test]
    fn a_decision_without_a_body_is_whole_as_a_title() {
        let mut x = d(1, DecisionKind::Gotcha, DecisionStatus::Active, 0);
        x.body.clear();
        let (_, shown) = laid_out(Path::new("/r/app"), &[x], BUDGET, "valk");
        assert_eq!(shown.whole, [1]);
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
        assert!(text.contains("`/opt/v/valk pane --help`"));
    }

    #[test]
    fn hook_output_is_valid_json() {
        let out = hook_output("SessionStart", "a \"q\"\n\tb\\ \u{1}");
        assert_eq!(
            out,
            r#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"a \"q\"\n\tb\\ \u0001"}}"#
        );
    }

    #[test]
    fn the_users_habits_are_listed_by_title_apart_from_the_project() {
        assert_eq!(habits(&[], "valk"), "");
        let mut habit = d(1, DecisionKind::Policy, DecisionStatus::Active, 1);
        habit.title = "Ship finished work as a pull request".into();
        habit.body = "template: ship\nuntil: pr".into();
        let proposed = d(2, DecisionKind::Policy, DecisionStatus::Proposed, 2);
        let text = habits(&[habit, proposed], "valk");
        assert!(!text.contains("#2"));
        assert!(text.starts_with("The user's habits, which hold in every project"));
        assert!(text.contains("- #1 [policy] Ship finished work as a pull request\n"));
        assert!(!text.contains("until"));
    }
}

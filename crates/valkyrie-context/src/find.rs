//! Finding decisions by their words (ADR-0014 §3–4): the ones a prompt bears on,
//! added to it by the hook, and a search an agent or you run on demand.

use std::collections::{BTreeMap, BTreeSet};
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus};

/// The most decisions added to one prompt.
pub const MAX_PROMPT: usize = 3;
/// The most of a decision's body added to a prompt; `show` has the rest.
pub const PROMPT_BODY: usize = 500;
/// The most hits a search returns.
pub const MAX_SEARCH: usize = 20;

/// A text's words for matching: lowercase, three letters or more, without the
/// words every sentence has, and a plural's `s`.
pub fn words(text: &str) -> BTreeSet<String> {
    crate::extract::words(text)
        .into_iter()
        .filter(|w| !crate::mine::STOPWORDS.contains(&w.as_str()))
        .map(|w| match w.strip_suffix('s') {
            Some(one) if one.len() > 2 && !one.ends_with('s') => one.to_owned(),
            _ => w,
        })
        .collect()
}

/// The active decisions `prompt` bears on, most shared words first, at most
/// [`MAX_PROMPT`], leaving out the ones in `had` (served whole this session
/// already). Skills have their own match ([`crate::skill::about`]) and a
/// policy's hook does its work, so neither is added. A decision must share two
/// words with the prompt, one of them in its title; a word most decisions have
/// (this project's name, "agent") says nothing about which one is meant, so it
/// doesn't count.
pub fn for_prompt<'a>(
    prompt: &str,
    known: &'a [Decision],
    had: &BTreeSet<u32>,
) -> Vec<&'a Decision> {
    let candidates: Vec<(&Decision, BTreeSet<String>, BTreeSet<String>)> = known
        .iter()
        .filter(|d| d.status == DecisionStatus::Active)
        .filter(|d| !matches!(d.kind, DecisionKind::Skill | DecisionKind::Policy))
        .map(|d| (d, words(&d.title), words(&d.body)))
        .collect();
    // How many decisions have each word, over all of them, not only the ones
    // not yet served: what's common doesn't change with what was served.
    let mut have: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, title, body) in &candidates {
        for w in title.union(body) {
            *have.entry(w.as_str()).or_default() += 1;
        }
    }
    let common = (candidates.len() / 4).max(2);
    let said: BTreeSet<String> = words(prompt)
        .into_iter()
        .filter(|w| have.get(w.as_str()).is_none_or(|n| *n <= common))
        .collect();
    let mut scored: Vec<(usize, usize, &Decision)> = candidates
        .iter()
        .filter(|(d, ..)| !had.contains(&d.id))
        .filter_map(|(d, title, body)| {
            let in_title = title.intersection(&said).count();
            let shared = title.union(body).filter(|w| said.contains(*w)).count();
            (in_title >= 1 && shared >= 2).then_some((shared, in_title, *d))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(b.2.id.cmp(&a.2.id)));
    scored
        .into_iter()
        .take(MAX_PROMPT)
        .map(|(.., d)| d)
        .collect()
}

/// Active decisions, and with `inactive` the ones that ended (rejected, retired,
/// superseded); never proposals, which nobody has reviewed.
pub fn listed(d: &Decision, inactive: bool) -> bool {
    d.status == DecisionStatus::Active || (inactive && d.status != DecisionStatus::Proposed)
}

/// Decisions with any of `query`'s words in their kind, title or body, most
/// words matched first, at most [`MAX_SEARCH`]. A word matches inside another
/// ("proto" finds "protocol"): a search is asked for, so it errs on finding.
pub fn search<'a>(list: &'a [Decision], query: &str, inactive: bool) -> Vec<&'a Decision> {
    let words: Vec<String> = query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(str::to_owned)
        .collect();
    let mut scored: Vec<(usize, &Decision)> = list
        .iter()
        .filter(|d| listed(d, inactive))
        .filter_map(|d| {
            let text = format!("{} {} {}", d.title, d.body, d.kind.as_str()).to_lowercase();
            let score = words.iter().filter(|w| text.contains(w.as_str())).count();
            (score > 0).then_some((score, d))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.id.cmp(&a.1.id)));
    scored
        .into_iter()
        .map(|(_, d)| d)
        .take(MAX_SEARCH)
        .collect()
}

/// One decision on one line: `#id [kind] title: body`, with its status when it
/// isn't active.
pub fn line(d: &Decision) -> String {
    let mut text = format!("#{} [{}] {}", d.id, d.kind.as_str(), d.title);
    if d.status != DecisionStatus::Active {
        text.push_str(&format!(" ({})", d.status.as_str()));
    }
    let body = d.body.split_whitespace().collect::<Vec<_>>().join(" ");
    if !body.is_empty() {
        text.push_str(&format!(": {body}"));
    }
    text
}

/// The decisions a prompt bears on, for the hook to add to it; empty for none.
pub fn prompt_section(found: &[&Decision], valk: &str) -> String {
    if found.is_empty() {
        return String::new();
    }
    let mut text = String::from(
        "Project decisions that bear on this (recorded with Valkyrie, confirmed by the user; \
         follow them, or say if one looks wrong):\n",
    );
    for d in found {
        let body = d.body.split_whitespace().collect::<Vec<_>>().join(" ");
        text.push_str(&format!("- #{} [{}] {}", d.id, d.kind.as_str(), d.title));
        if !body.is_empty() {
            text.push_str(": ");
            text.push_str(&clip(
                &body,
                PROMPT_BODY,
                &format!("{valk} decisions show {}", d.id),
            ));
        }
        text.push('\n');
    }
    text
}

/// `text` cut to `max` bytes at a word, saying where the rest is.
fn clip(text: &str, max: usize, rest: &str) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = text[..end].rfind(' ').unwrap_or(end);
    format!("{}… (`{rest}` has the rest)", text[..cut].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(id: u32, kind: DecisionKind, title: &str, body: &str) -> Decision {
        Decision {
            id,
            project: "/r".into(),
            title: title.into(),
            body: body.into(),
            kind,
            status: DecisionStatus::Active,
            created: 0,
            updated: 0,
            supersedes: None,
            superseded_by: None,
            pinned: false,
            provenance: Default::default(),
            fresh: Default::default(),
        }
    }

    fn ids(v: &[&Decision]) -> Vec<u32> {
        v.iter().map(|d| d.id).collect()
    }

    /// Eight unrelated decisions, so no word is common by being in half of few.
    fn filler() -> Vec<Decision> {
        filler_from(100)
    }

    fn filler_from(from: u32) -> Vec<Decision> {
        [
            "Pair devices with a short code",
            "Rain animates under reduced motion",
            "Use Alt chords for keybinds",
            "Tags never move once published",
            "Release with release-please",
            "Keep idea capture out of the tool",
            "Lanes come from sessions",
            "Bamboo reports build status",
        ]
        .iter()
        .enumerate()
        .map(|(i, t)| d(from + i as u32, DecisionKind::Decision, t, ""))
        .collect()
    }

    #[test]
    fn a_prompt_gets_the_decisions_it_shares_words_with_best_first() {
        let mut known = filler();
        known.push(d(
            1,
            DecisionKind::Gotcha,
            "Run shellcheck on shell scripts before pushing",
            "CI lints them and fails on SC2015.",
        ));
        known.push(d(
            2,
            DecisionKind::Decision,
            "Shell completions are generated, not written",
            "clap_complete writes them at build time.",
        ));
        let prompt = "the install scripts fail shellcheck in CI, can you fix them?";
        assert_eq!(ids(&for_prompt(prompt, &known, &BTreeSet::new())), [1]);
        // Two words, but none of them in the title: the body alone is too loose.
        assert!(for_prompt("build time with clap_complete", &known, &BTreeSet::new()).is_empty());
        assert_eq!(
            ids(&for_prompt(
                "shell completions at build time",
                &known,
                &BTreeSet::new()
            )),
            [2]
        );
    }

    #[test]
    fn what_the_session_had_skills_and_policies_and_proposals_are_left_out() {
        let mut known = filler();
        known.push(d(1, DecisionKind::Gotcha, "Run shellcheck on scripts", ""));
        known.push(d(2, DecisionKind::Skill, "Run shellcheck on scripts", ""));
        known.push(d(3, DecisionKind::Policy, "Run shellcheck on scripts", ""));
        let mut proposed = d(4, DecisionKind::Gotcha, "Run shellcheck on scripts", "");
        proposed.status = DecisionStatus::Proposed;
        known.push(proposed);
        let prompt = "run shellcheck on the scripts";
        assert_eq!(ids(&for_prompt(prompt, &known, &BTreeSet::new())), [1]);
        assert!(for_prompt(prompt, &known, &BTreeSet::from([1])).is_empty());
    }

    #[test]
    fn a_word_most_decisions_have_doesnt_count_and_three_at_most() {
        // Every one is about the daemon; only the one about sockets is meant.
        let mut known: Vec<Decision> = (1..=8)
            .map(|i| {
                d(
                    i,
                    DecisionKind::Decision,
                    &format!("Daemon thing number{i}"),
                    "The daemon restarts.",
                )
            })
            .collect();
        known.push(d(
            9,
            DecisionKind::Gotcha,
            "Daemon socket path limit",
            "107 bytes.",
        ));
        let prompt = "the daemon restarts on a long socket path";
        assert_eq!(ids(&for_prompt(prompt, &known, &BTreeSet::new())), [9]);
        let many: Vec<Decision> = (1..=4)
            .map(|i| {
                d(
                    i,
                    DecisionKind::Decision,
                    &format!("Socket path rule{i}"),
                    "",
                )
            })
            .chain(filler())
            .chain(filler_from(200))
            .collect();
        assert_eq!(
            for_prompt("socket path", &many, &BTreeSet::new()).len(),
            MAX_PROMPT
        );
    }

    #[test]
    fn a_long_body_is_clipped_at_a_word_and_says_where_the_rest_is() {
        let body = "word ".repeat(200);
        let one = d(7, DecisionKind::Gotcha, "Title", &body);
        let text = prompt_section(&[&one], "valk");
        assert!(text.contains("- #7 [gotcha] Title: word word"));
        assert!(text.contains("word… (`valk decisions show 7` has the rest)"));
        assert!(text.len() < PROMPT_BODY + 300);
        assert!(prompt_section(&[], "valk").is_empty());
    }

    #[test]
    fn search_ranks_by_words_matched_and_skips_inactive_unless_asked() {
        let mut retired = d(3, DecisionKind::Decision, "Use npm", "");
        retired.status = DecisionStatus::Retired;
        let list = [
            d(1, DecisionKind::Decision, "Use pnpm for installs", ""),
            d(
                2,
                DecisionKind::Decision,
                "Use pnpm workspaces for installs of tools",
                "",
            ),
            retired,
        ];
        assert_eq!(ids(&search(&list, "pnpm installs tools", false)), [2, 1]);
        assert_eq!(ids(&search(&list, "npm", false)), [2, 1]);
        assert_eq!(ids(&search(&list, "npm", true)), [3, 2, 1]);
        let mut proposed = d(4, DecisionKind::Decision, "Use npm everywhere", "");
        proposed.status = DecisionStatus::Proposed;
        assert!(search(&[proposed], "npm", true).is_empty());
        assert!(search(&list, "kubernetes", true).is_empty());
    }
}

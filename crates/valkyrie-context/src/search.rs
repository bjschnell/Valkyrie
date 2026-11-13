//! Search over session history (backlog #106): the session where you said or did
//! something. Runs over the away digest's `Run`s (`digest::read_said`), so both
//! read the event logs the same way and name sessions alike. A session matches
//! when its history holds every word of the query somewhere; its best line is
//! the one holding the most of them, the latest first.

use crate::digest::{self, Known, Run, Said};
use jiff::tz::TimeZone;
use valkyrie_proto::{SearchField, SearchHit, SearchResults};

/// Characters a snippet shows.
const SNIPPET: usize = 160;
/// Characters a snippet keeps before the first word it shows.
const LEAD: usize = 50;
/// Hits returned when no limit is asked for.
pub const DEFAULT_LIMIT: u32 = 20;

/// The words to find, folded to lower case. A `"quoted phrase"` is one word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub terms: Vec<String>,
}

impl Query {
    pub fn parse(text: &str) -> Self {
        let mut terms = Vec::new();
        for (i, part) in text.split('"').enumerate() {
            if i % 2 == 1 {
                let phrase = crate::file::one_line(part);
                if !phrase.is_empty() {
                    terms.push(fold(&phrase));
                }
            } else {
                terms.extend(part.split_whitespace().map(fold));
            }
        }
        terms.sort();
        terms.dedup();
        Self { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// How many of the words `folded` holds.
    fn held(&self, folded: &str) -> usize {
        self.terms
            .iter()
            .filter(|t| folded.contains(t.as_str()))
            .count()
    }
}

/// Lower case a character at a time, so a character index in the folded text is
/// one in the original.
fn fold(text: &str) -> String {
    text.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

/// `text` clipped to about `SNIPPET` characters around the first word it holds.
pub fn snippet(text: &str, query: &Query) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= SNIPPET {
        return text.to_owned();
    }
    let folded = fold(text);
    let first = query
        .terms
        .iter()
        .filter_map(|t| folded.find(t.as_str()))
        .min()
        .map_or(0, |byte| folded[..byte].chars().count());
    let start = first.saturating_sub(LEAD).min(chars.len() - SNIPPET);
    let end = start + SNIPPET;
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(chars[start..end].iter());
    if end < chars.len() {
        out.push('…');
    }
    out.trim().to_owned()
}

/// The lines a session is known by beside what was said: its backlog item,
/// topic, name, branch and pull request, at its last activity.
fn about(run: &Run, item_line: Option<String>) -> Vec<Said> {
    let at = run.last_ms;
    item_line
        .into_iter()
        .chain(run.topic.clone())
        .chain(run.name.clone())
        .chain(run.branch.clone())
        .chain(run.prs.iter().cloned())
        .map(|text| Said {
            t: at,
            field: SearchField::About,
            text,
        })
        .collect()
}

/// One hit per session active in the window whose history holds every word,
/// newest first. `repo` keeps those in that repository (by folder name). The
/// resume command is left for the caller, which knows the agents.
pub fn hits(runs: &[Run], known: &Known, query: &Query, repo: Option<&str>) -> Vec<SearchHit> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<SearchHit> = runs
        .iter()
        .filter_map(|run| {
            run.first_ms?;
            let agent = run.agent.clone()?;
            let item = digest::item_for(run, known.items);
            let where_ = digest::repo_of(run, item, known.roots);
            if repo.is_some_and(|r| !r.eq_ignore_ascii_case(&where_)) {
                return None;
            }
            let lines: Vec<Said> = run
                .said
                .iter()
                .cloned()
                .chain(about(run, item.map(|i| format!("#{} {}", i.id, i.title))))
                .collect();
            let folded: Vec<String> = lines.iter().map(|l| fold(&l.text)).collect();
            if !query
                .terms
                .iter()
                .all(|t| folded.iter().any(|f| f.contains(t.as_str())))
            {
                return None;
            }
            let held: Vec<usize> = folded.iter().map(|f| query.held(f)).collect();
            let full = held.iter().filter(|&&n| n == query.terms.len()).count();
            // The line holding the most words; of those, what was said over what
            // it's about, then the latest.
            let best = (0..lines.len())
                .filter(|&i| held[i] > 0)
                .max_by_key(|&i| (held[i], lines[i].field != SearchField::About, lines[i].t))?;
            let line = &lines[best];
            let session = known.live.get(&run.key).copied();
            Some(SearchHit {
                key: run.key.clone(),
                session,
                repo: where_,
                cwd: run.cwd.clone(),
                gone: run.cwd.as_deref().is_some_and(|c| !c.exists()),
                what: digest::what(run, item, &agent),
                agent,
                item: item.map(|i| i.id),
                branch: run.branch.clone(),
                pr: run.pr.clone().or_else(|| item.and_then(|i| i.pr.clone())),
                field: line.field,
                snippet: snippet(&line.text, query),
                at_ms: line.t,
                lines: u32::try_from(full).unwrap_or(u32::MAX),
                last_ms: run.last_ms,
                ended_ms: run.exited_ms,
                resume: None,
            })
        })
        .collect();
    out.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then(a.key.cmp(&b.key)));
    out
}

/// A word for a shell: quoted unless it's plainly safe.
fn sh(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
    if plain {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// How to get back to a hit: attach a live session, or resume a closed one in a
/// new session where it worked.
pub fn jump(hit: &SearchHit) -> Option<String> {
    if let Some(id) = hit.session {
        return Some(format!("valk attach {id}"));
    }
    let resume = hit.resume.as_ref()?;
    let mut out = String::from("valk new");
    if let Some(cwd) = hit.cwd.as_deref().filter(|_| !hit.gone) {
        out.push_str(&format!(" --cwd {}", sh(&cwd.to_string_lossy())));
    }
    out.push_str(" -a --");
    for word in resume {
        out.push(' ');
        out.push_str(&sh(word));
    }
    Some(out)
}

/// The results as text for a terminal: a few lines a session.
pub fn render(results: &SearchResults, tz: &TimeZone) -> String {
    if results.hits.is_empty() {
        return format!(
            "No session since {} holds {}.\n",
            digest::when(results.since_ms, tz),
            shown_query(&results.query)
        );
    }
    let mut out = format!(
        "{} {} since {} {} {}{}:\n",
        results.total,
        if results.total == 1 {
            "session"
        } else {
            "sessions"
        },
        digest::when(results.since_ms, tz),
        if results.total == 1 { "holds" } else { "hold" },
        shown_query(&results.query),
        if results.hits.len() < results.total as usize {
            format!(", the newest {}", results.hits.len())
        } else {
            String::new()
        },
    );
    for hit in &results.hits {
        let state = match (hit.session, hit.ended_ms) {
            (Some(_), _) => "running",
            (None, Some(_)) => "ended",
            (None, None) => "closed",
        };
        out.push_str(&format!(
            "\n{}  {} · {} · {}\n  {}\n  {}: {}\n",
            digest::when(hit.at_ms, tz),
            hit.repo,
            hit.agent,
            state,
            hit.what,
            hit.field.label(),
            hit.snippet,
        ));
        if let Some(jump) = jump(hit) {
            out.push_str(&format!("  → {jump}\n"));
            if let Some(cwd) = hit
                .cwd
                .as_deref()
                .filter(|_| hit.gone && hit.session.is_none())
            {
                out.push_str(&format!(
                    "    (it worked in {}, which is gone: the agent may not find its conversation)\n",
                    cwd.display()
                ));
            }
        }
    }
    out
}

fn shown_query(query: &str) -> String {
    let query = query.trim();
    if query.contains('"') {
        query.to_owned()
    } else {
        format!("“{query}”")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use valkyrie_proto::Item;

    fn said(t: u64, field: SearchField, text: &str) -> Said {
        Said {
            t,
            field,
            text: text.into(),
        }
    }

    fn run(key: &str, said: Vec<Said>) -> Run {
        Run {
            key: key.into(),
            agent: Some("claude".into()),
            cwd: Some(PathBuf::from("/nowhere/valkyrie-feat-9-x")),
            first_ms: Some(1),
            last_ms: said.iter().map(|s| s.t).max().unwrap_or(1),
            said,
            ..Run::default()
        }
    }

    fn item(id: u32, title: &str, branch: &str) -> Item {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": title, "created": 0, "updated": 0,
            "branch": branch, "project": "/nowhere/valkyrie",
        }))
        .unwrap()
    }

    fn known<'a>(items: &'a [Item], live: &'a BTreeMap<String, u32>) -> Known<'a> {
        Known {
            items,
            live,
            roots: &[],
        }
    }

    #[test]
    fn a_query_folds_case_and_keeps_a_quoted_phrase_whole() {
        assert_eq!(
            Query::parse(r#"Zoom  "One Key"  zoom"#).terms,
            vec!["one key".to_owned(), "zoom".to_owned()]
        );
        assert!(Query::parse(r#"  "" "#).is_empty());
    }

    #[test]
    fn a_session_matches_when_its_history_holds_every_word_across_lines() {
        let runs = vec![
            run(
                "1-1",
                vec![
                    said(10, SearchField::You, "make the pane zoom"),
                    said(20, SearchField::File, "/r/crates/tui/src/layout.rs"),
                ],
            ),
            run("2-2", vec![said(30, SearchField::You, "zoom the docs")]),
        ];
        let live = BTreeMap::from([("2-2".to_owned(), 7)]);
        let k = known(&[], &live);

        let hits = hits(&runs, &k, &Query::parse("ZOOM layout.rs"), None);
        assert_eq!(hits.len(), 1, "only the first holds both words");
        assert_eq!(hits[0].key, "1-1");
        assert_eq!(hits[0].lines, 0, "no one line holds both");

        let hits = super::hits(&runs, &k, &Query::parse("zoom"), None);
        assert_eq!(
            hits.iter().map(|h| h.key.as_str()).collect::<Vec<_>>(),
            vec!["2-2", "1-1"],
            "newest first"
        );
        assert_eq!(hits[0].session, Some(7));
        assert_eq!(jump(&hits[0]).as_deref(), Some("valk attach 7"));
        assert_eq!((hits[1].field, hits[1].at_ms), (SearchField::You, 10));
    }

    #[test]
    fn the_best_line_holds_the_most_words_and_what_was_said_beats_what_its_about() {
        let mut r = run(
            "1-1",
            vec![
                said(10, SearchField::You, "fix the flaky hook test"),
                said(20, SearchField::Agent, "the hook test is fixed"),
                said(30, SearchField::Agent, "tests pass"),
            ],
        );
        r.branch = Some("feat/9-hook".into());
        let items = [item(9, "Hook test flakes", "feat/9-hook")];
        let live = BTreeMap::new();
        let k = known(&items, &live);

        let h = &hits(&[r.clone()], &k, &Query::parse("hook test"), None)[0];
        assert_eq!(
            (h.field, h.at_ms),
            (SearchField::Agent, 20),
            "latest of the full lines"
        );
        assert_eq!(h.lines, 3, "two said, and the item's title");
        assert_eq!(h.item, Some(9));
        assert_eq!(h.repo, "valkyrie", "placed by its item's project");

        let h = &hits(&[r.clone()], &k, &Query::parse("flakes"), None)[0];
        assert_eq!(h.field, SearchField::About, "only the item's title says it");
        assert_eq!(h.snippet, "#9 Hook test flakes");

        assert!(hits(&[r.clone()], &k, &Query::parse("hook"), Some("other")).is_empty());
        assert_eq!(
            hits(&[r], &k, &Query::parse("hook"), Some("Valkyrie")).len(),
            1
        );
    }

    #[test]
    fn shells_and_sessions_idle_in_the_window_are_left_out() {
        let mut shell = run("1-1", vec![said(10, SearchField::You, "zoom")]);
        shell.agent = None;
        let mut idle = run("2-2", vec![]);
        idle.first_ms = None;
        idle.topic = Some("zoom".into());
        let live = BTreeMap::new();
        assert!(
            hits(
                &[shell, idle],
                &known(&[], &live),
                &Query::parse("zoom"),
                None
            )
            .is_empty()
        );
    }

    #[test]
    fn a_snippet_is_clipped_around_the_first_word() {
        let q = Query::parse("needle");
        assert_eq!(snippet("short needle", &q), "short needle");
        let long = format!("{} needle {}", "a".repeat(300), "b".repeat(300));
        let s = snippet(&long, &q);
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
        assert!(s.contains("needle"), "{s}");
        assert!(s.chars().count() <= SNIPPET + 2);
        // Near the end: the window slides back rather than running short.
        let tail = format!("{}needle", "a".repeat(300));
        let s = snippet(&tail, &q);
        assert!(
            s.ends_with("needle") && s.chars().count() == SNIPPET + 1,
            "{s}"
        );
        // Folding keeps character positions in non-ASCII text.
        let s = snippet(&format!("{}ÄÖ NEEDLE", "ü".repeat(300)), &q);
        assert!(s.ends_with("NEEDLE"), "{s}");
    }

    #[test]
    fn a_closed_session_resumes_in_a_new_session_where_it_worked() {
        let dir = PathBuf::from("/r");
        let mut hit = hits(
            &[run("1-1", vec![said(10, SearchField::You, "zoom")])],
            &known(&[], &BTreeMap::new()),
            &Query::parse("zoom"),
            None,
        )
        .remove(0);
        assert_eq!(jump(&hit), None, "no agent command to resume with");
        hit.resume = Some(vec!["claude".into(), "--resume".into(), "abc-1".into()]);
        assert!(hit.gone, "/nowhere doesn't exist");
        assert_eq!(
            jump(&hit).as_deref(),
            Some("valk new -a -- claude --resume abc-1"),
            "a directory that's gone is left out"
        );
        let text = render(
            &SearchResults {
                query: "zoom".into(),
                since_ms: 0,
                until_ms: 1,
                total: 1,
                hits: vec![hit.clone()],
            },
            &TimeZone::UTC,
        );
        assert!(
            text.contains("/nowhere/valkyrie-feat-9-x, which is gone"),
            "{text}"
        );
        hit.cwd = Some(dir.join("it's here"));
        hit.gone = false;
        let j = jump(&hit).unwrap();
        assert!(j.contains(r"'\''s here'"), "{j}");
    }

    #[test]
    fn render_says_how_to_get_back() {
        let mut hit = hits(
            &[run(
                "1-1",
                vec![said(10, SearchField::You, "zoom the pane")],
            )],
            &known(&[], &BTreeMap::new()),
            &Query::parse("zoom"),
            None,
        )
        .remove(0);
        hit.session = Some(3);
        let results = SearchResults {
            query: "zoom".into(),
            since_ms: 0,
            until_ms: 100,
            total: 2,
            hits: vec![hit],
        };
        let text = render(&results, &TimeZone::UTC);
        assert!(text.starts_with("2 sessions since"), "{text}");
        assert!(text.contains("hold “zoom”, the newest 1:"), "{text}");
        assert!(text.contains("you: zoom the pane"), "{text}");
        assert!(text.contains("→ valk attach 3"), "{text}");
        let none = SearchResults {
            hits: vec![],
            total: 0,
            ..results
        };
        assert!(render(&none, &TimeZone::UTC).starts_with("No session since"));
    }
}

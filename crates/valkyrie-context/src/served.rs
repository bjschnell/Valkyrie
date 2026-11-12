//! Which decisions reached which session, and how (ADR-0014 §5), so `health`
//! can flag a decision no session gets. The hooks and the read commands append
//! to the project's `served.jsonl`; this is the pure part.
//! Searches log the agent notes and past-session lines they return in the same
//! file (ADR-0017 rule 3, [`FoundServe`]).

use crate::stale;
use std::collections::{BTreeMap, BTreeSet};
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus};

/// How long a serve counts.
pub const SERVED_DAYS: u64 = 90;
/// A decision no session got for this many days, while this many sessions
/// worked in the project, is flagged: a project left alone flags nothing.
pub const UNSERVED_DAYS: u64 = 45;
/// Sessions that started in the project without getting a decision before
/// it's flagged.
pub const UNSERVED_SESSIONS: usize = 10;
/// The most hits of one search logged: a vague query that hits everything
/// doesn't mean the agent read everything.
pub const MAX_SEARCH_SERVED: usize = 5;

/// How a decision reached a session (ADR-0014's layers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// In the session-start block with its body.
    Block,
    /// In the session-start block as a title line only.
    Title,
    /// Matched to a prompt.
    Prompt,
    /// Scoped to a file the agent touched.
    File,
    /// Found by a search.
    Search,
    /// Read whole on request (`valk decisions show`, MCP `get_decision`).
    Show,
}

impl Layer {
    pub fn as_str(self) -> &'static str {
        match self {
            Layer::Block => "block",
            Layer::Title => "title",
            Layer::Prompt => "prompt",
            Layer::File => "file",
            Layer::Search => "search",
            Layer::Show => "show",
        }
    }

    pub fn parse(s: &str) -> Option<Layer> {
        Some(match s {
            "block" => Layer::Block,
            "title" => Layer::Title,
            "prompt" => Layer::Prompt,
            "file" => Layer::File,
            "search" => Layer::Search,
            "show" => Layer::Show,
            _ => return None,
        })
    }

    /// Whether the decision's body reached the session. A title alone says
    /// the decision exists, not what it says, so it doesn't count as served.
    pub fn whole(self) -> bool {
        self != Layer::Title
    }
}

/// One decision served to one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Serve {
    /// The agent's own session id (a `/clear` starts a new one), else Valkyrie's.
    pub session: String,
    pub id: u32,
    pub layer: Layer,
    pub at: u64,
}

impl Serve {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "session": self.session,
            "id": self.id,
            "layer": self.layer.as_str(),
            "at": self.at,
        })
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Serve> {
        Some(Serve {
            session: v["session"].as_str()?.to_owned(),
            id: u32::try_from(v["id"].as_u64()?).ok()?,
            layer: Layer::parse(v["layer"].as_str()?)?,
            at: v["at"].as_u64()?,
        })
    }
}

/// What a search served that isn't a decision (ADR-0017 rule 3): an agent note,
/// or a line of one of this repository's past session transcripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Note(u32),
    Transcript { path: String, line: usize },
}

/// A note or a transcript line served to one session. Its line in the log has
/// `note` or `transcript` and `line` where a decision's has `id`, so readers of
/// either kind skip the other's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundServe {
    pub session: String,
    pub found: Found,
    pub layer: Layer,
    pub at: u64,
}

impl FoundServe {
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "session": self.session,
            "layer": self.layer.as_str(),
            "at": self.at,
        });
        match &self.found {
            Found::Note(id) => v["note"] = (*id).into(),
            Found::Transcript { path, line } => {
                v["transcript"] = path.as_str().into();
                v["line"] = (*line).into();
            }
        }
        v
    }

    pub fn from_json(v: &serde_json::Value) -> Option<FoundServe> {
        let found = match (v["note"].as_u64(), v["transcript"].as_str()) {
            (Some(id), _) => Found::Note(u32::try_from(id).ok()?),
            (None, Some(path)) => Found::Transcript {
                path: path.to_owned(),
                line: usize::try_from(v["line"].as_u64()?).ok()?,
            },
            (None, None) => return None,
        };
        Some(FoundServe {
            session: v["session"].as_str()?.to_owned(),
            found,
            layer: Layer::parse(v["layer"].as_str()?)?,
            at: v["at"].as_u64()?,
        })
    }
}

/// Active decisions no session got whole for [`UNSERVED_DAYS`] while
/// [`UNSERVED_SESSIONS`] sessions worked in the project, each with how many
/// days. Counted from when the log starts, a decision's acceptance or last
/// confirmation, and its last serve, whichever is latest. Skills are measured
/// by reads (ADR-0011 §6), and a policy is served as a title on purpose since
/// its hook does the work, so neither is flagged.
pub fn unserved(decisions: &[Decision], serves: &[Serve], now: u64) -> Vec<(u32, u64)> {
    let Some(start) = serves.iter().map(|s| s.at).min() else {
        return Vec::new();
    };
    let mut last: BTreeMap<u32, u64> = BTreeMap::new();
    for s in serves.iter().filter(|s| s.layer.whole()) {
        let at = last.entry(s.id).or_insert(0);
        *at = (*at).max(s.at);
    }
    decisions
        .iter()
        .filter(|d| d.status == DecisionStatus::Active)
        .filter(|d| !matches!(d.kind, DecisionKind::Skill | DecisionKind::Policy))
        .filter_map(|d| {
            let since = start
                .max(stale::confirmed(d))
                .max(last.get(&d.id).copied().unwrap_or(0));
            let days = now.saturating_sub(since) / 86_400;
            // The sessions that started here since, and didn't get it. A start is
            // the block's serve, keyed by the agent's own id; the read commands'
            // `valk:` ids are the same runs again, so they'd count twice.
            let got: BTreeSet<&str> = serves
                .iter()
                .filter(|s| s.id == d.id && s.layer.whole())
                .map(|s| s.session.as_str())
                .collect();
            let sessions: BTreeSet<&str> = serves
                .iter()
                .filter(|s| s.at >= since && matches!(s.layer, Layer::Block | Layer::Title))
                .map(|s| s.session.as_str())
                .filter(|s| !got.contains(s))
                .collect();
            (days >= UNSERVED_DAYS && sessions.len() >= UNSERVED_SESSIONS).then_some((d.id, days))
        })
        .collect()
}

/// The decisions `session` already got whole since it last started, so a hook
/// doesn't serve them again. A start (the block's serve) is also a compaction,
/// which summarizes away what came before, so only what came since counts.
/// The read commands log under Valkyrie's session (`valk`), which spans the
/// agent's `/clear`s, so theirs count from that start too.
pub fn had(serves: &[Serve], session: &str, valk: Option<&str>) -> BTreeSet<u32> {
    let mine = || serves.iter().filter(|s| s.session == session);
    let start = mine()
        .filter(|s| matches!(s.layer, Layer::Block | Layer::Title))
        .map(|s| s.at)
        .max()
        .or_else(|| mine().map(|s| s.at).min());
    let Some(start) = start else {
        return BTreeSet::new();
    };
    serves
        .iter()
        .filter(|s| s.layer.whole() && s.at >= start)
        .filter(|s| s.session == session || valk == Some(s.session.as_str()))
        .map(|s| s.id)
        .collect()
}

/// Why a decision no session got for `days` needs you.
pub fn says(days: u64) -> String {
    format!(
        "no session got it in {days} days of work here; reword its title so it's found, or \
         retire it"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;

    fn d(id: u32, kind: DecisionKind) -> Decision {
        Decision {
            id,
            project: "/r".into(),
            title: format!("decision {id}"),
            body: String::new(),
            kind,
            status: DecisionStatus::Active,
            created: 0,
            updated: 0,
            supersedes: None,
            superseded_by: None,
            pinned: false,
            scope: Vec::new(),
            proposed_scope: None,
            scope_asked: false,
            provenance: Default::default(),
            fresh: Default::default(),
        }
    }

    fn serve(session: usize, id: u32, layer: Layer, at: u64) -> Serve {
        Serve {
            session: format!("s{session}"),
            id,
            layer,
            at,
        }
    }

    /// `n` sessions at `at`, each served #1 whole and #2 as a title.
    fn sessions(n: usize, from: usize, at: u64) -> Vec<Serve> {
        (from..from + n)
            .flat_map(|i| [serve(i, 1, Layer::Block, at), serve(i, 2, Layer::Title, at)])
            .collect()
    }

    #[test]
    fn serves_round_trip_and_skip_what_they_cant_read() {
        let s = serve(3, 7, Layer::Prompt, 42);
        assert_eq!(Serve::from_json(&s.to_json()), Some(s));
        let bad = serde_json::json!({"session": "x", "id": 1, "layer": "telepathy", "at": 1});
        assert_eq!(Serve::from_json(&bad), None);
    }

    #[test]
    fn found_serves_round_trip_and_decision_readers_skip_them() {
        let note = FoundServe {
            session: "valk:3".into(),
            found: Found::Note(12),
            layer: Layer::Search,
            at: 9,
        };
        let line = FoundServe {
            found: Found::Transcript {
                path: "/c/projects/-r/a.jsonl".into(),
                line: 40,
            },
            ..note.clone()
        };
        for f in [&note, &line] {
            assert_eq!(FoundServe::from_json(&f.to_json()).as_ref(), Some(f));
            assert_eq!(Serve::from_json(&f.to_json()), None);
        }
        assert_eq!(note.to_json()["note"], 12);
        assert_eq!(
            FoundServe::from_json(&serve(1, 2, Layer::Search, 3).to_json()),
            None
        );
    }

    #[test]
    fn a_decision_only_ever_shown_as_a_title_is_flagged() {
        let list = [d(1, DecisionKind::Gotcha), d(2, DecisionKind::Decision)];
        let log = sessions(12, 0, 10 * DAY);
        assert_eq!(unserved(&list, &log, 60 * DAY), [(2, 50)]);
    }

    #[test]
    fn a_decision_served_once_then_missed_by_every_session_since_is_flagged() {
        let list = [d(1, DecisionKind::Gotcha), d(3, DecisionKind::Decision)];
        let mut log = vec![serve(100, 3, Layer::Prompt, 5 * DAY)];
        log.extend(sessions(12, 0, 10 * DAY));
        assert_eq!(unserved(&list, &log, 60 * DAY), [(3, 55)]);
    }

    #[test]
    fn reads_from_inside_a_session_dont_count_as_more_sessions() {
        let list = [d(2, DecisionKind::Decision)];
        let mut log = sessions(9, 0, 10 * DAY);
        // The same nine runs reading another decision through `valk:` ids.
        log.extend((0..9).map(|i| Serve {
            session: format!("valk:{i}"),
            id: 1,
            layer: Layer::Show,
            at: 11 * DAY,
        }));
        assert!(unserved(&list, &log, 60 * DAY).is_empty());
    }

    #[test]
    fn nothing_is_flagged_too_soon_or_in_a_quiet_project() {
        let list = [d(2, DecisionKind::Decision)];
        // 40 days isn't long enough.
        assert!(unserved(&list, &sessions(12, 0, 10 * DAY), 50 * DAY).is_empty());
        // Nine sessions in 50 days is a project left alone.
        assert!(unserved(&list, &sessions(9, 0, 10 * DAY), 60 * DAY).is_empty());
        // Before anything was logged, nothing is known.
        assert!(unserved(&list, &[], 600 * DAY).is_empty());
    }

    #[test]
    fn a_recent_serve_or_confirmation_restarts_the_count() {
        let mut log = sessions(12, 0, 10 * DAY);
        log.push(serve(99, 2, Layer::Search, 30 * DAY));
        let list = [d(2, DecisionKind::Decision)];
        assert!(unserved(&list, &log, 60 * DAY).is_empty());
        let mut confirmed = d(2, DecisionKind::Decision);
        confirmed.fresh.confirmed = 30 * DAY;
        assert!(unserved(&[confirmed], &sessions(12, 0, 10 * DAY), 60 * DAY).is_empty());
    }

    #[test]
    fn a_session_had_what_it_got_whole_and_what_valk_read_it_since_it_began() {
        let valk = |id, at| Serve {
            session: "valk:7".into(),
            id,
            layer: Layer::Show,
            at,
        };
        let log = [
            valk(9, 1),
            serve(1, 1, Layer::Block, 5),
            serve(1, 2, Layer::Title, 5),
            serve(1, 3, Layer::Prompt, 6),
            serve(2, 4, Layer::Block, 6),
            valk(5, 7),
        ];
        let had = |valk_id| had(&log, "s1", valk_id).into_iter().collect::<Vec<_>>();
        assert_eq!(had(None), [1, 3]);
        // #9 was read before this agent session began (a `/clear` ago).
        assert_eq!(had(Some("valk:7")), [1, 3, 5]);
        assert!(super::had(&log, "s9", Some("valk:7")).is_empty());
        // Compacted at 8: what it got before was summarized away.
        let mut log = log.to_vec();
        log.push(serve(1, 1, Layer::Block, 8));
        log.push(serve(1, 2, Layer::Title, 8));
        assert_eq!(
            super::had(&log, "s1", Some("valk:7"))
                .into_iter()
                .collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn skills_policies_and_inactive_ones_are_left_alone() {
        let mut retired = d(4, DecisionKind::Decision);
        retired.status = DecisionStatus::Retired;
        let list = [
            d(2, DecisionKind::Skill),
            d(3, DecisionKind::Policy),
            retired,
        ];
        assert!(unserved(&list, &sessions(12, 0, 10 * DAY), 60 * DAY).is_empty());
    }
}

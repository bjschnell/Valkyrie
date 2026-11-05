//! Learning which supervisor steps to take without asking (ADR-0019 §3). Each
//! answer to a step the supervisor offered is a line in the project's
//! `supervisor.jsonl`. Once the user has taken the same step on the same trigger
//! in two sessions, with no decline of it since, a `policy` for it is proposed
//! (`template: merge-base`, `on: conflicting`). This is the pure part; the daemon
//! records the answers and files the proposal.

use crate::policy::{self, LEARNABLE, StepPolicy, Trigger};
use std::collections::HashSet;
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus, StepAction};

/// How long an answer counts (ADR-0019 §3).
pub const STEP_DAYS: u64 = 90;
/// A rejected step policy isn't offered again for this long (ADR-0013 §4).
pub const REJECTED_DAYS: u64 = 30;
/// Sessions the same step must be taken in before it's offered as a policy.
pub const SESSIONS: usize = 2;

/// One step offered and answered, or taken by a policy (`supervisor.jsonl`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub at: u64,
    /// The session's name, so two answers in one count once.
    pub session: String,
    pub trigger: Trigger,
    /// What the supervisor offered first.
    pub offered: StepAction,
    /// What was taken: the offer, its alternative, or `leave`.
    pub taken: StepAction,
    pub pr: String,
    /// The policy that took it without asking, if one did.
    pub policy: Option<u32>,
}

impl Record {
    /// The user took what was offered.
    pub fn accepted(&self) -> bool {
        self.taken == self.offered
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "at": self.at, "session": self.session, "trigger": self.trigger.as_str(),
            "offered": self.offered.as_str(), "taken": self.taken.as_str(),
            "accepted": self.accepted(), "pr": self.pr,
        });
        if let Some(id) = self.policy {
            v["policy"] = id.into();
        }
        v
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Record> {
        let action = |k: &str| {
            let s = v[k].as_str()?;
            [
                StepAction::MergeBase,
                StepAction::CiFix,
                StepAction::Rerun,
                StepAction::Leave,
            ]
            .into_iter()
            .find(|a| a.as_str() == s)
        };
        Some(Record {
            at: v["at"].as_u64()?,
            session: v["session"].as_str()?.to_owned(),
            trigger: Trigger::parse(v["trigger"].as_str()?)?,
            offered: action("offered")?,
            taken: action("taken")?,
            pr: v["pr"].as_str().unwrap_or_default().to_owned(),
            policy: v["policy"].as_u64().and_then(|n| u32::try_from(n).ok()),
        })
    }
}

/// A step policy to propose, and the answers it's learned from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub step: StepPolicy,
    pub taken: Vec<Record>,
}

/// The step policies the user's answers make worth offering, given every answer
/// recorded in the project and its decisions. A step counts where the user took
/// it, offered first or as the alternative; a decline of it (anything else taken
/// when it was offered first) starts the count over. Answers count from when a
/// policy for the step was last reviewed, so one just retired isn't offered back
/// at once; none is offered while one is active or waits on the user, or for
/// [`REJECTED_DAYS`] after one was rejected.
pub fn offers(records: &[Record], project: &[Decision], now: u64) -> Vec<Offer> {
    let mut out = Vec::new();
    for &(action, on) in LEARNABLE {
        let step = StepPolicy { action, on };
        let mine: Vec<&Decision> = project
            .iter()
            .filter(|d| d.kind == DecisionKind::Policy)
            .filter(|d| policy::step(&d.body).and_then(Result::ok) == Some(step))
            .collect();
        let blocked = mine.iter().any(|d| match d.status {
            DecisionStatus::Active | DecisionStatus::Proposed => true,
            DecisionStatus::Rejected => now.saturating_sub(d.updated) < REJECTED_DAYS * 86_400,
            _ => false,
        });
        if blocked {
            continue;
        }
        let since = mine.iter().map(|d| d.updated).max().unwrap_or(0);
        let mut answers: Vec<&Record> = records
            .iter()
            .filter(|r| r.trigger == on && r.policy.is_none() && r.at > since)
            .filter(|r| now.saturating_sub(r.at) <= STEP_DAYS * 86_400)
            .collect();
        answers.sort_by_key(|r| r.at);
        let declined = answers
            .iter()
            .rposition(|r| r.offered == action && r.taken != action);
        let taken: Vec<&Record> = answers[declined.map_or(0, |i| i + 1)..]
            .iter()
            .copied()
            .filter(|r| r.taken == action)
            .collect();
        let sessions: HashSet<&str> = taken.iter().map(|r| r.session.as_str()).collect();
        if sessions.len() >= SESSIONS {
            out.push(Offer {
                step,
                taken: taken.into_iter().cloned().collect(),
            });
        }
    }
    out
}

/// The proposal's title and body: the settings, then the answers quoted as
/// evidence, on `>` lines a policy's settings skip.
pub fn proposal(offer: &Offer) -> (String, String) {
    let title = match offer.step.action {
        StepAction::MergeBase => "Merge the base into a PR that conflicts with it, without asking",
        StepAction::CiFix => "Send the agent to fix a PR whose CI went red, without asking",
        _ => "Rerun failed jobs the supervisor calls a flake, without asking",
    };
    let mut body = format!(
        "template: {}\non: {}\n\n",
        offer.step.action.as_str(),
        offer.step.on.as_str()
    );
    body.push_str(&format!(
        "> Learned from the supervisor's offers you took ({} on a {} PR):\n",
        offer.step.action.as_str(),
        offer.step.on.as_str()
    ));
    for r in offer.taken.iter().rev().take(5) {
        body.push_str(&format!("> {} ({})\n", r.pr, r.session));
    }
    body.push_str("> Accepted, it runs without asking on an idle session; retiring it stops it.\n");
    (title.into(), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(at: u64, session: &str, offered: StepAction, taken: StepAction) -> Record {
        Record {
            at,
            session: session.into(),
            trigger: match offered {
                StepAction::MergeBase => Trigger::Conflicting,
                _ => Trigger::Red,
            },
            offered,
            taken,
            pr: format!("https://github.com/o/r/pull/{at}"),
            policy: None,
        }
    }

    fn policy(id: u32, status: DecisionStatus, updated: u64, body: &str) -> Decision {
        Decision {
            id,
            project: "/r".into(),
            title: "p".into(),
            body: body.into(),
            kind: DecisionKind::Policy,
            status,
            created: updated,
            updated,
            supersedes: None,
            superseded_by: None,
            pinned: false,
            scope: Vec::new(),
            proposed_scope: None,
            scope_asked: false,
            fresh: Default::default(),
            provenance: Default::default(),
        }
    }

    use StepAction::{CiFix, Leave, MergeBase, Rerun};
    const NOW: u64 = 100 * 86_400;

    fn learned(records: &[Record], decisions: &[Decision]) -> Vec<(StepAction, Trigger)> {
        offers(records, decisions, NOW)
            .into_iter()
            .map(|o| (o.step.action, o.step.on))
            .collect()
    }

    #[test]
    fn the_same_step_taken_in_two_sessions_is_offered() {
        let one = [rec(NOW - 10, "a", MergeBase, MergeBase)];
        assert!(learned(&one, &[]).is_empty());
        // Twice in one session is still one session.
        let same = [
            rec(NOW - 10, "a", MergeBase, MergeBase),
            rec(NOW - 5, "a", MergeBase, MergeBase),
        ];
        assert!(learned(&same, &[]).is_empty());
        let two = [
            rec(NOW - 10, "a", MergeBase, MergeBase),
            rec(NOW - 5, "b", MergeBase, MergeBase),
        ];
        assert_eq!(learned(&two, &[]), vec![(MergeBase, Trigger::Conflicting)]);
    }

    #[test]
    fn a_decline_starts_the_count_over() {
        let records = [
            rec(NOW - 30, "a", MergeBase, MergeBase),
            rec(NOW - 20, "b", MergeBase, MergeBase),
            rec(NOW - 15, "c", MergeBase, Leave),
            rec(NOW - 10, "d", MergeBase, MergeBase),
        ];
        assert!(learned(&records, &[]).is_empty());
        let mut more = records.to_vec();
        more.push(rec(NOW - 5, "e", MergeBase, MergeBase));
        assert_eq!(learned(&more, &[]), vec![(MergeBase, Trigger::Conflicting)]);
    }

    #[test]
    fn the_alternative_taken_counts_for_it_and_against_the_offer() {
        // Rerun offered, ci-fix taken, twice: ci-fix is learned, rerun isn't.
        let records = [
            rec(NOW - 20, "a", Rerun, Rerun),
            rec(NOW - 10, "b", Rerun, CiFix),
            rec(NOW - 5, "c", Rerun, CiFix),
        ];
        assert_eq!(learned(&records, &[]), vec![(CiFix, Trigger::Red)]);
    }

    #[test]
    fn steps_a_policy_took_and_old_answers_dont_count() {
        let mut auto = rec(NOW - 5, "b", MergeBase, MergeBase);
        auto.policy = Some(9);
        let records = [rec(NOW - 10, "a", MergeBase, MergeBase), auto];
        assert!(learned(&records, &[]).is_empty());
        let old = [
            rec(NOW - (STEP_DAYS + 1) * 86_400, "a", MergeBase, MergeBase),
            rec(NOW - 5, "b", MergeBase, MergeBase),
        ];
        assert!(learned(&old, &[]).is_empty());
    }

    #[test]
    fn a_policy_in_force_waiting_or_lately_rejected_blocks_the_offer() {
        let records = [
            rec(NOW - 10, "a", MergeBase, MergeBase),
            rec(NOW - 5, "b", MergeBase, MergeBase),
        ];
        let body = "template: merge-base\non: conflicting";
        for status in [DecisionStatus::Active, DecisionStatus::Proposed] {
            assert!(learned(&records, &[policy(1, status, 1, body)]).is_empty());
        }
        let rejected = policy(1, DecisionStatus::Rejected, NOW - 86_400, body);
        assert!(learned(&records, &[rejected]).is_empty());
        let long_ago = policy(1, DecisionStatus::Rejected, 1, body);
        assert_eq!(learned(&records, &[long_ago]).len(), 1);
        // Retired after these answers: they don't bring it straight back.
        let retired = policy(1, DecisionStatus::Retired, NOW - 1, body);
        assert!(learned(&records, &[retired]).is_empty());
        // Another step's policy is no matter.
        let other = policy(2, DecisionStatus::Active, 1, "template: rerun\non: red");
        assert_eq!(learned(&records, &[other]).len(), 1);
    }

    #[test]
    fn the_proposal_is_a_policy_the_project_accepts() {
        let offer = offers(
            &[
                rec(NOW - 10, "a", MergeBase, MergeBase),
                rec(NOW - 5, "b", MergeBase, MergeBase),
            ],
            &[],
            NOW,
        )
        .remove(0);
        let (title, body) = proposal(&offer);
        assert!(title.starts_with("Merge the base"));
        assert!(body.starts_with("template: merge-base\non: conflicting\n"));
        assert!(policy::check(&body, false).is_ok(), "{body}");
        assert!(policy::check(&body, true).is_err());
        assert_eq!(
            policy::step(&body),
            Some(Ok(StepPolicy {
                action: MergeBase,
                on: Trigger::Conflicting
            }))
        );
        // And a `ship` reading of it finds nothing.
        assert!(policy::parse(&body).is_err());
    }

    #[test]
    fn a_record_reads_back() {
        let mut r = rec(5, "a", Rerun, CiFix);
        r.policy = Some(3);
        let v = r.to_json();
        assert_eq!(v["accepted"], false);
        assert_eq!(Record::from_json(&v), Some(r));
    }
}

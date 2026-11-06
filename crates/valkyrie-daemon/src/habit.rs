//! Learning how far the user ships (ADR-0013 §2 and §4, ADR-0016 §2). Each reply
//! to a stop short of shipping is classified by the locked-down model, under the
//! same consent (`valk decisions auto`) as the extractor, and recorded in the user
//! scope. Once replies in two sessions, in any repository, agree that it's
//! routine, a `ship` habit is proposed for the user to accept: never more.

use crate::Registry;
use crate::session::Answered;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use valkyrie_context::{outcome, project, reply};
use valkyrie_proto::{AutoMode, DecisionKind, NewDecision, Provenance, SessionId};

/// Most model calls per hour for replies. A routine answer is a few a day.
const PER_HOUR: usize = 20;
const MODEL_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) async fn run(
    registry: Arc<Registry>,
    mut answers: mpsc::UnboundedReceiver<(SessionId, Answered)>,
) {
    let command = valkyrie_context::model::command(reply::SYSTEM);
    let mut calls: VecDeque<Instant> = VecDeque::new();
    while let Some((id, answered)) = answers.recv().await {
        let mode = crate::extract::mode(&registry.host.context_dir);
        if mode == AutoMode::Off {
            continue;
        }
        let Ok(session) = registry.get(id) else {
            continue;
        };
        let info = session.info();
        if !mode.reads(&info.status.agent) {
            continue;
        }
        while calls
            .front()
            .is_some_and(|t| t.elapsed() > Duration::from_secs(3600))
        {
            calls.pop_front();
        }
        // A long prompt is a new instruction, not a routine answer: no model needed.
        let class = if !reply::worth_asking(&answered.before, &answered.prompt) {
            reply::Class::Other
        } else if calls.len() >= PER_HOUR {
            tracing::debug!(session = id, "reply skipped: hourly limit");
            continue;
        } else {
            calls.push_back(Instant::now());
            let prompt = reply::prompt(&answered.before, &answered.prompt);
            let dir = registry.host.context_dir.clone();
            let command = command.clone();
            let asked = tokio::task::spawn_blocking(move || {
                valkyrie_proto::ensure_private_dir(&dir)?;
                valkyrie_context::model::ask(&command, &prompt, &dir, MODEL_TIMEOUT)
            })
            .await;
            match asked.map_err(anyhow::Error::from).and_then(|r| r) {
                Ok(answer) => match reply::parse(&answer) {
                    Some(class) => class,
                    None => {
                        tracing::warn!(session = id, "reply classifier gave no class");
                        continue;
                    }
                },
                Err(e) => {
                    tracing::warn!(session = id, "reply classification failed: {e:#}");
                    continue;
                }
            }
        };
        learn(&registry, &session.key(), &info, &answered, class);
    }
}

/// Records one classified reply under `key`, the session's own (names repeat),
/// and proposes the habit it completes.
fn learn(
    registry: &Registry,
    key: &str,
    info: &valkyrie_proto::SessionInfo,
    answered: &Answered,
    class: reply::Class,
) {
    let now = now_secs();
    let step = reply::step(
        class,
        &answered.before,
        reply::reached(&answered.after, answered.pr),
    );
    let record = reply::Record {
        at: now,
        session: key.to_owned(),
        project: project::root(&info.cwd).display().to_string(),
        state: answered.before.clone(),
        step,
        hash: format!("{:016x}", crate::extract::fnv(&answered.prompt)),
        quote: matches!(class, reply::Class::Routine(_)).then(|| reply::quote(&answered.prompt)),
    };
    // A reply to work a policy left short says whether the user overrode it.
    if let Some(kept) = &answered.block {
        let overridden = class == reply::Class::Other;
        record_outcome(registry, key, kept, overridden, "reply", now);
    }
    let store = registry.context.lock().unwrap();
    if let Err(e) = store.record_reply(&record, now) {
        tracing::warn!("couldn't record a reply: {e:#}");
        return;
    }
    let Some(offer) = reply::offer(&store.replies(now), &store.load(&project::user()), now) else {
        return;
    };
    let (title, body) = reply::proposal(&offer);
    let new = NewDecision {
        cwd: project::user(),
        title,
        body,
        kind: DecisionKind::Policy,
        propose: true,
        supersedes: offer.supersedes,
        review_every: None,
        session: None,
        scope: None,
    };
    let provenance = Provenance {
        by: "valkyrie".into(),
        session: Some(info.name.clone()),
        conversation: None,
        commit: None,
        cwd: Some(info.cwd.clone()),
    };
    match store.decide(&new, provenance, now) {
        Ok(d) => {
            tracing::info!(
                id = d.id,
                until = offer.until.as_str(),
                "ship habit proposed"
            );
            drop(store);
            registry.refresh_proposals();
        }
        Err(e) => tracing::warn!("couldn't propose a ship habit: {e:#}"),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Records how the session's blocks turned out (ADR-0013 §6), in each policy's
/// own scope: the user's for a habit, the repository's for its own policy.
pub(crate) fn record_outcomes(registry: &Registry, session: &crate::session::Session) {
    let outcomes = session.take_outcomes();
    if outcomes.is_empty() {
        return;
    }
    let key = session.key();
    let now = now_secs();
    for o in &outcomes {
        record_outcome(registry, &key, &o.kept, o.overridden, o.how, now);
    }
}

fn record_outcome(
    registry: &Registry,
    key: &str,
    kept: &outcome::Kept,
    overridden: bool,
    how: &str,
    now: u64,
) {
    let repo = project::root(&kept.cwd);
    let scope = match kept.block.scope.as_str() {
        "user" => project::user(),
        _ => repo.clone(),
    };
    let record = outcome::Record {
        at: now,
        session: key.to_owned(),
        policy: kept.block.policy,
        scope: kept.block.scope.clone(),
        project: repo.display().to_string(),
        overridden,
        how: how.to_owned(),
        via: None,
    };
    let store = registry.context.lock().unwrap();
    if let Err(e) = store.record_outcome(&scope, &record, now) {
        tracing::warn!("couldn't record a policy's outcome: {e:#}");
    }
}

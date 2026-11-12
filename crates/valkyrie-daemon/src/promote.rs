//! Recurring notes become a proposal (ADR-0017 §7). After a note is written, its
//! project's notes are clustered; one that has recurred in enough separate
//! sessions is proposed, once, as a `gotcha` or `fix` by `valkyrie`. The model
//! drafts it under `valk decisions auto` consent; with that off, or when the
//! model gives nothing, the newest note says it as written.

use crate::Registry;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use valkyrie_context::{extract, promote};
use valkyrie_proto::{AutoMode, NewDecision, Provenance};

/// Most model calls per hour for drafting: a cluster is proposed once, so this
/// only bites when many come due together.
const PER_HOUR: usize = 6;
const MODEL_TIMEOUT: Duration = Duration::from_secs(90);

/// Takes the projects notes were written in and proposes what has recurred.
pub(crate) async fn run(registry: Arc<Registry>, mut written: mpsc::UnboundedReceiver<PathBuf>) {
    let command = valkyrie_context::model::command(promote::SYSTEM);
    let mut calls: VecDeque<Instant> = VecDeque::new();
    while let Some(root) = written.recv().await {
        let mut roots = vec![root];
        while let Ok(more) = written.try_recv() {
            if !roots.contains(&more) {
                roots.push(more);
            }
        }
        for root in roots {
            while calls
                .front()
                .is_some_and(|t| t.elapsed() > Duration::from_secs(3600))
            {
                calls.pop_front();
            }
            // Every cluster that's due, not just the first: each round holds or
            // proposes one, so the next round finds the next. Bounded, so a
            // record that keeps failing can't spin.
            for _ in 0..PER_HOUR {
                if !check(&registry, root.clone(), &command, &mut calls).await {
                    break;
                }
            }
        }
    }
}

/// Proposes `root`'s next recurring cluster, if one is due. Whether it held or
/// proposed one, so there may be another.
async fn check(
    registry: &Registry,
    root: PathBuf,
    command: &[String],
    calls: &mut VecDeque<Instant>,
) -> bool {
    let (mut notes, links, known) = {
        let store = registry.context.lock().unwrap();
        let known = store.load(&root);
        if let Err(e) = store.heal_promotions(&root, &known) {
            tracing::warn!(project = %root.display(), "couldn't promote notes: {e:#}");
        }
        (store.notes(&root), store.promotion_links(&root), known)
    };
    // Off the lock and the runtime: it runs `git` once per live note with anchors.
    let at = root.clone();
    notes = match tokio::task::spawn_blocking(move || {
        for n in &mut notes {
            n.stale = valkyrie_context::notes::staleness(&at, n);
        }
        notes
    })
    .await
    {
        Ok(notes) => notes,
        Err(_) => return false,
    };
    let now = now_secs();
    // Notes that joined a cluster already proposed are recorded with it, so it
    // stays held after the notes it came from go stale or are deleted.
    for (decision, matched, ids) in promote::joined(&notes, &links, &known) {
        record(registry, &root, decision, &ids, matched, now);
    }
    let Some(cluster) = promote::ready(&notes, &links, &known) else {
        return false;
    };
    let ids: Vec<u32> = cluster.iter().map(|n| n.id).collect();
    let Some(plain) = promote::plain(&cluster) else {
        return false;
    };
    // Said already (any status, so a rejected one isn't made again): the cluster
    // is held by that decision, without asking the model.
    if let Some(d) = matching(&plain.title, &known) {
        return record(registry, &root, d, &ids, true, now);
    }
    let mode = crate::extract::mode(&registry.host.context_dir);
    let mut found = None;
    if mode != AutoMode::Off && calls.len() < PER_HOUR {
        calls.push_back(Instant::now());
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let prompt =
            promote::prompt(&name, &known, &cluster) + &valkyrie_context::scope::paths(&root);
        let dir = registry.host.context_dir.clone();
        let command = command.to_vec();
        let asked = tokio::task::spawn_blocking(move || {
            valkyrie_proto::ensure_private_dir(&dir)?;
            valkyrie_context::model::ask(&command, &prompt, &dir, MODEL_TIMEOUT)
        })
        .await;
        match asked.map_err(anyhow::Error::from).and_then(|r| r) {
            Ok(reply) => match promote::parse_reply(&reply) {
                promote::Reply::Draft(f) => found = Some(f),
                // The model found it already known: held, not proposed.
                promote::Reply::Known => {
                    tracing::info!(project = %root.display(), notes = ?ids, "recurring notes already known");
                    return record(registry, &root, promote::KNOWN, &ids, true, now);
                }
                promote::Reply::Garbled => {
                    tracing::warn!(project = %root.display(), "drafting from notes gave nothing usable")
                }
            },
            Err(e) => {
                tracing::warn!(project = %root.display(), "drafting from notes failed: {e:#}")
            }
        }
    }
    let f = found.unwrap_or(plain);
    if let Some(d) = matching(&f.title, &known) {
        return record(registry, &root, d, &ids, true, now);
    }
    let scope = valkyrie_context::scope::proposed(&f.scope, &root);
    let body = match f.body.trim() {
        "" => promote::cite(&cluster),
        body => format!("{body}\n\n{}", promote::cite(&cluster)),
    };
    let sig = promote::signature(&cluster);
    let new = NewDecision {
        cwd: root.clone(),
        title: f.title,
        body,
        kind: f.kind,
        propose: true,
        supersedes: None,
        review_every: None,
        session: None,
        scope: (!scope.is_empty()).then_some(scope),
    };
    let provenance = Provenance {
        by: "valkyrie".into(),
        session: None,
        conversation: None,
        commit: None,
        cwd: Some(root.clone()),
    };
    // The proposal and its link under one lock, so nobody accepts it before the
    // link says which notes it promotes.
    let decided = {
        let store = registry.context.lock().unwrap();
        store.decide(&new, provenance, now).map(|d| {
            let linked = store.record_promotion(&root, d.id, &ids, false, now);
            // So ADR-0011 §6 can tell whether it worked, like the retro's.
            let sig = sig.map(|sig| store.record_proposal(&root, sig, d.id, now));
            (d, linked, sig)
        })
    };
    match decided {
        Ok((d, linked, sig)) => {
            tracing::info!(project = %root.display(), id = d.id, notes = ?ids, "proposed from recurring notes");
            if let Some(Err(e)) = &sig {
                tracing::warn!(id = d.id, "couldn't record the proposal: {e:#}");
            }
            registry.refresh_proposals();
            // Without its link the cluster still looks due: stop, or the next
            // round proposes it again.
            match linked {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!(id = d.id, "couldn't record the notes' proposal: {e:#}");
                    false
                }
            }
        }
        Err(e) => {
            tracing::warn!(project = %root.display(), "couldn't propose from notes: {e:#}");
            false
        }
    }
}

/// The known decision `title` says the same as, if any.
fn matching(title: &str, known: &[valkyrie_proto::Decision]) -> Option<u32> {
    let words = extract::words(title);
    known
        .iter()
        .find(|d| !words.is_empty() && extract::similar(&words, &extract::words(&d.title)))
        .map(|d| d.id)
}

fn record(
    registry: &Registry,
    root: &std::path::Path,
    decision: u32,
    notes: &[u32],
    matched: bool,
    now: u64,
) -> bool {
    let recorded = registry
        .context
        .lock()
        .unwrap()
        .record_promotion(root, decision, notes, matched, now);
    if let Err(e) = recorded {
        tracing::warn!(project = %root.display(), decision, "couldn't record the notes' proposal: {e:#}");
        return false;
    }
    true
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

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
            check(&registry, root, &command, &mut calls).await;
        }
    }
}

/// Proposes `root`'s next recurring cluster, if one is due.
async fn check(
    registry: &Registry,
    root: PathBuf,
    command: &[String],
    calls: &mut VecDeque<Instant>,
) {
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
        Err(_) => return,
    };
    let Some(cluster) = promote::ready(&notes, &links, &known) else {
        return;
    };
    let ids: Vec<u32> = cluster.iter().map(|n| n.id).collect();
    let Some(plain) = promote::plain(&cluster) else {
        return;
    };
    let now = now_secs();
    // Said already (any status, so a rejected one isn't made again): the cluster
    // is held by that decision, without asking the model.
    if let Some(d) = matching(&plain.title, &known) {
        record(registry, &root, d, &ids, true, now);
        return;
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
            Ok(reply) => found = promote::parse_reply(&reply),
            Err(e) => {
                tracing::warn!(project = %root.display(), "drafting from notes failed: {e:#}")
            }
        }
    }
    let f = found.unwrap_or(plain);
    if let Some(d) = matching(&f.title, &known) {
        record(registry, &root, d, &ids, true, now);
        return;
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
    let decided = registry
        .context
        .lock()
        .unwrap()
        .decide(&new, provenance, now);
    match decided {
        Ok(d) => {
            tracing::info!(project = %root.display(), id = d.id, notes = ?ids, "proposed from recurring notes");
            record(registry, &root, d.id, &ids, false, now);
            // So ADR-0011 §6 can tell whether it worked, like the retro's.
            if let Some(sig) = sig
                && let Err(e) = registry
                    .context
                    .lock()
                    .unwrap()
                    .record_proposal(&root, sig, d.id, now)
            {
                tracing::warn!(id = d.id, "couldn't record the proposal: {e:#}");
            }
            registry.refresh_proposals();
        }
        Err(e) => tracing::warn!(project = %root.display(), "couldn't propose from notes: {e:#}"),
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
) {
    let recorded = registry
        .context
        .lock()
        .unwrap()
        .record_promotion(root, decision, notes, matched, now);
    if let Err(e) = recorded {
        tracing::warn!(project = %root.display(), decision, "couldn't record the notes' proposal: {e:#}");
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

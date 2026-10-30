//! Catching decisions in your corrections (DESIGN §6.3). When an agent's turn ends,
//! the daemon reads the exchange around your last message. If that message reads
//! like a correction, a locked-down `claude -p --model haiku` is asked for the
//! lasting rule in it, which is then proposed for you to review. Never anything
//! more: its proposals wait on you like an agent's.
//!
//! A turn that ended after a tool failed also gets a retro (ADR-0011): the failed
//! calls and what worked next are read for the lasting lesson in them, under the
//! same limits.

use crate::Registry;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use valkyrie_context::{extract, retro, skill};
use valkyrie_proto::{AutoMode, NewDecision, Provenance, SessionId};

/// How much of a transcript's end is read: a few exchanges.
const TAIL: u64 = 256 * 1024;
/// Most model calls per hour, whatever happens.
const PER_HOUR: usize = 30;
const MODEL_TIMEOUT: Duration = Duration::from_secs(90);
/// Lets the agent finish writing its reply to the transcript after `Stop`.
const SETTLE: Duration = Duration::from_millis(500);

/// Most model calls per session per hour, so one chatty session can't use them all.
const PER_SESSION_HOUR: usize = 6;

/// Which sessions are read, kept in `<context>/auto` (`valk decisions auto`).
/// Before it existed, `auto-off` meant off.
pub fn mode(context_dir: &Path) -> AutoMode {
    if let Ok(text) = std::fs::read_to_string(context_dir.join("auto")) {
        return AutoMode::parse(&text).unwrap_or_default();
    }
    if context_dir.join("auto-off").exists() {
        return AutoMode::Off;
    }
    AutoMode::default()
}

pub fn set_mode(context_dir: &Path, mode: AutoMode) -> anyhow::Result<()> {
    valkyrie_proto::ensure_private_dir(context_dir)?;
    std::fs::write(context_dir.join("auto"), format!("{}\n", mode.as_str()))?;
    let _ = std::fs::remove_file(context_dir.join("auto-off"));
    Ok(())
}

/// Takes `Stop`s (session ids) and proposes what it finds, one at a time.
pub(crate) async fn run(registry: Arc<Registry>, mut turns: mpsc::UnboundedReceiver<SessionId>) {
    let command = valkyrie_context::model::command(extract::SYSTEM);
    let retro_command = valkyrie_context::model::command(retro::SYSTEM);
    let learn_command = valkyrie_context::model::command(skill::SYSTEM);
    let mut last: HashMap<SessionId, u64> = HashMap::new();

    let mut calls: VecDeque<(Instant, SessionId)> = VecDeque::new();
    while let Some(id) = turns.recv().await {
        tokio::time::sleep(SETTLE).await;
        // Several turns may have ended meanwhile; each is looked at once.
        let mut ids = vec![id];
        while let Ok(more) = turns.try_recv() {
            if !ids.contains(&more) {
                ids.push(more);
            }
        }
        for id in ids {
            // Which skills it read: on this machine only, so whatever `auto` says.
            track_use(registry.clone(), id).await;
            let mode = mode(&registry.host.context_dir);
            if mode == AutoMode::Off {
                // So failures don't pile up for when reading is back on.
                if let Ok(s) = registry.get(id) {
                    s.take_failures();
                }
                continue;
            }
            while calls
                .front()
                .is_some_and(|(t, _)| t.elapsed() > Duration::from_secs(3600))
            {
                calls.pop_front();
            }
            let room = |calls: &VecDeque<(Instant, SessionId)>| {
                let theirs = calls.iter().filter(|(_, s)| *s == id).count();
                calls.len() < PER_HOUR && theirs < PER_SESSION_HOUR
            };
            if !room(&calls) {
                tracing::debug!(session = id, "extraction skipped: hourly limit");
                continue;
            }
            if look(&registry, id, mode, &command, &mut last).await {
                calls.push_back((Instant::now(), id));
            }
            // A retro that doesn't run now leaves its failures for the next turn's end.
            let failed = registry.get(id).map(|s| s.failures()).unwrap_or(0);
            if failed > 0 && room(&calls) && reflect(&registry, id, mode, &retro_command).await {
                calls.push_back((Instant::now(), id));
            }
            if room(&calls) && learn(&registry, id, mode, &learn_command).await {
                calls.push_back((Instant::now(), id));
            }
        }
    }
}

/// Looks at one session's last exchange; returns whether the model was asked.
async fn look(
    registry: &Registry,
    id: SessionId,
    mode: AutoMode,
    command: &[String],
    last: &mut HashMap<SessionId, u64>,
) -> bool {
    let Ok(session) = registry.get(id) else {
        return false;
    };
    let info = session.info();
    if !mode.reads(&info.status.agent) {
        return false;
    }
    let Some(chat) = info.chat.clone() else {
        return false;
    };
    let Ok(tail) = tokio::task::spawn_blocking(move || read_tail(&chat)).await else {
        return false;
    };
    let messages = extract::messages(&tail);
    let Some(exchange) = extract::last_exchange(&messages) else {
        return false;
    };
    // The same message ends several turns (a long task, a resume): once is enough.
    let mark = fnv(exchange.1);
    if last.insert(id, mark) == Some(mark) || !extract::worth_a_look(exchange.1) {
        return false;
    }
    let root = valkyrie_context::project::root(&info.cwd);
    let known = registry.context.lock().unwrap().load(&root);
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prompt = extract::prompt(&name, &known, exchange);
    let yours = exchange.1.to_owned();
    let dir = registry.host.context_dir.clone();
    let command = command.to_vec();
    let asked = tokio::task::spawn_blocking(move || {
        valkyrie_proto::ensure_private_dir(&dir)?;
        valkyrie_context::model::ask(&command, &prompt, &dir, MODEL_TIMEOUT)
    })
    .await;
    let reply = match asked.map_err(anyhow::Error::from).and_then(|r| r) {
        Ok(reply) => reply,
        Err(e) => {
            tracing::warn!(session = id, "extraction failed: {e:#}");
            return true;
        }
    };
    let found = extract::parse_reply(&reply);
    let mut known = known;
    let mut proposed = 0;
    for f in found {
        if extract::already_known(&f.title, &known) {
            continue;
        }
        // Redacted like the prompt: it's stored, shown, and once accepted, injected.
        let quote: String = extract::redact(&yours)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let quote = if quote.chars().count() > 200 {
            quote.chars().take(200).collect::<String>() + "…"
        } else {
            quote
        };
        let new = NewDecision {
            cwd: info.cwd.clone(),
            title: f.title,
            body: format!("{}\n\nFrom your message: “{quote}”", f.body),
            kind: f.kind,
            propose: true,
            supersedes: None,
            review_every: None,
            session: Some(id),
        };
        let provenance = Provenance {
            by: "valkyrie".into(),
            session: Some(info.name.clone()),
            conversation: session.conversation(),
            commit: None,
            cwd: Some(info.cwd.clone()),
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        match registry
            .context
            .lock()
            .unwrap()
            .decide(&new, provenance, now)
        {
            Ok(d) => {
                tracing::info!(session = id, id = d.id, "proposed from a correction");
                known.push(d);
                proposed += 1;
            }
            Err(e) => tracing::warn!(session = id, "couldn't propose: {e:#}"),
        }
    }
    if proposed > 0 {
        registry.refresh_proposals();
    }
    true
}

/// The retro of one session's latest turns: reads its failed calls and what
/// worked next, records the ones not recorded before, and asks the model about
/// those. Returns whether the model was asked.
async fn reflect(registry: &Registry, id: SessionId, mode: AutoMode, command: &[String]) -> bool {
    let Ok(session) = registry.get(id) else {
        return false;
    };
    let info = session.info();
    if !mode.reads(&info.status.agent) {
        return false;
    }
    // Not known yet (its first hook is still on the way): kept for the next turn.
    let Some(chat) = info.chat.clone() else {
        return false;
    };
    // Outside a repository there's no project to learn for.
    let root = valkyrie_context::project::root(&info.cwd);
    if !valkyrie_context::project::in_git(&root) {
        session.take_failures();
        return false;
    }
    session.take_failures();
    let Ok(tail) = tokio::task::spawn_blocking(move || read_tail(&chat)).await else {
        return false;
    };
    let now = now_secs();
    let before = registry.context.lock().unwrap().failures(&root, now);
    let (mut moments, waiting): (Vec<_>, Vec<_>) = retro::moments(&tail)
        .into_iter()
        .filter(|m| !before.calls.contains(&m.id))
        .partition(retro::Moment::answered);
    if !waiting.is_empty() {
        session.arm_retro();
    }
    if moments.is_empty() {
        return false;
    }
    let sigs: Vec<(u64, &str)> = moments
        .iter()
        .map(|m| (m.signature, m.id.as_str()))
        .collect();
    if let Err(e) = registry.context.lock().unwrap().record_failures(
        &root,
        &sigs,
        session.conversation().as_deref(),
        now,
    ) {
        tracing::warn!(session = id, "couldn't record failures: {e:#}");
    }
    let seen = before.seen;
    // Traps seen before first, then the newest.
    moments.reverse();
    moments.sort_by_key(|m| std::cmp::Reverse(seen.get(&m.signature).copied().unwrap_or(0)));
    moments.truncate(retro::MAX_MOMENTS);
    let moments: Vec<_> = moments
        .into_iter()
        .map(|m| {
            let count = seen.get(&m.signature).copied().unwrap_or(0);
            (m, count)
        })
        .collect();
    let mut known = registry.context.lock().unwrap().load(&root);
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Skills the session read: a failure may show one of them wrong.
    let followed = skill::active(&known, &skill::read(&tail));
    let (prompt, shown) = retro::prompt(&name, &known, &moments, &followed);
    let dir = registry.host.context_dir.clone();
    let command = command.to_vec();
    let asked = tokio::task::spawn_blocking(move || {
        valkyrie_proto::ensure_private_dir(&dir)?;
        valkyrie_context::model::ask(&command, &prompt, &dir, MODEL_TIMEOUT)
    })
    .await;
    let reply = match asked.map_err(anyhow::Error::from).and_then(|r| r) {
        Ok(reply) => reply,
        Err(e) => {
            tracing::warn!(session = id, "retro failed: {e:#}");
            return true;
        }
    };
    let source = |(m, count): &(retro::Moment, usize)| {
        let error = m
            .error
            .lines()
            .find(|l| !l.starts_with("(exit "))
            .unwrap_or("");
        let seen = if *count > 0 {
            format!(", seen {} times before", count)
        } else {
            String::new()
        };
        format!(
            "a failure{seen}: `{}` → {}",
            retro::quote(&m.input),
            retro::quote(error)
        )
    };
    // Each proposal, the skill it rewrites, and the moment it came from.
    let mut found: Vec<(extract::Found, Option<u32>, usize)> = Vec::new();
    for (f, n) in retro::parse_reply(&reply, moments.len()) {
        if !extract::already_known(&f.title, &known) {
            found.push((f, None, n));
        }
    }
    // One rewrite waiting at a time: the store refuses a second.
    if let Some((f, skill, n)) = retro::parse_patch(&reply, &shown, moments.len()) {
        found.push((f, Some(skill), n));
    }
    let mut proposed = 0;
    for (f, patch, n) in found {
        let m = &moments[n].0;
        let because = match patch {
            Some(_) => "Rewritten after",
            None => "From",
        };
        let new = NewDecision {
            cwd: info.cwd.clone(),
            title: f.title,
            body: format!("{}\n\n{because} {}", f.body, source(&moments[n])),
            kind: f.kind,
            propose: true,
            supersedes: patch,
            review_every: None,
            session: Some(id),
        };
        let provenance = Provenance {
            by: "valkyrie".into(),
            session: Some(info.name.clone()),
            conversation: session.conversation(),
            commit: None,
            cwd: Some(info.cwd.clone()),
        };
        let decided = registry
            .context
            .lock()
            .unwrap()
            .decide(&new, provenance, now);
        match decided {
            Ok(d) => {
                tracing::info!(session = id, id = d.id, "proposed from a failure");
                let recorded =
                    registry
                        .context
                        .lock()
                        .unwrap()
                        .record_proposal(&root, m.signature, d.id, now);
                if let Err(e) = recorded {
                    tracing::warn!(session = id, "couldn't record the proposal: {e:#}");
                }
                known.push(d);
                proposed += 1;
            }
            Err(e) => tracing::warn!(session = id, "couldn't propose: {e:#}"),
        }
    }
    if proposed > 0 {
        registry.refresh_proposals();
    }
    true
}

/// Records that a session worked in its project, and which of its active skills
/// it read, by the transcript's time (ADR-0011 §6), so one nobody reads is found.
async fn track_use(registry: Arc<Registry>, id: SessionId) {
    let Ok(session) = registry.get(id) else {
        return;
    };
    let info = session.info();
    let Some(chat) = info.chat.clone() else {
        return;
    };
    let who = session
        .conversation()
        .unwrap_or_else(|| format!("session-{id}"));
    // Files, and the store's lock: off the async threads.
    let _ = tokio::task::spawn_blocking(move || {
        let root = valkyrie_context::project::root(&info.cwd);
        let store = registry.context.lock().unwrap();
        let known = store.load(&root);
        let active: Vec<u32> =
            skill::active(&known, &known.iter().map(|d| d.id).collect::<Vec<_>>());
        // A project without skills has nothing to measure.
        if active.is_empty() {
            return;
        }
        let now = now_secs();
        let read: Vec<(u32, u64)> = skill::read_at(&read_tail(&chat))
            .into_iter()
            .filter(|(id, _)| active.contains(id))
            .map(|(id, at)| (id, at.unwrap_or(now).min(now)))
            .collect();
        let recorded = store.update_skill_use(&root, now, |used| {
            used.sessions.insert(who, now);
            for (id, at) in read {
                let last = used.reads.entry(id).or_insert(0);
                *last = (*last).max(at);
            }
        });
        if let Err(e) = recorded {
            tracing::warn!(session = id, "couldn't record skill use: {e:#}");
        }
    })
    .await;
}

/// When the last message you typed in a session approved a long job, asks the
/// model for the procedure in it, as a skill or a fix to one (ADR-0011 §5).
/// Returns whether the model was asked.
async fn learn(registry: &Registry, id: SessionId, mode: AutoMode, command: &[String]) -> bool {
    let Ok(session) = registry.get(id) else {
        return false;
    };
    let info = session.info();
    // Codex rollouts aren't read as a digest yet.
    if !mode.reads(&info.status.agent) || info.status.agent != "claude" {
        return false;
    }
    let Some(chat) = info.chat.clone() else {
        return false;
    };
    let root = valkyrie_context::project::root(&info.cwd);
    if !valkyrie_context::project::in_git(&root) {
        return false;
    }
    let Ok(tail) = tokio::task::spawn_blocking(move || read_tail(&chat)).await else {
        return false;
    };
    let Some((work, said, approval)) = skill::finished(&tail) else {
        return false;
    };
    if work < skill::LEARN_MIN_CALLS {
        return false;
    }
    // The same approval ends several turns (the commit and push it asked for),
    // and is still the last one after a restart: learned from once.
    {
        let store = registry.context.lock().unwrap();
        if store.skill_use(&root).learned.contains(&approval) {
            return false;
        }
        let noted = store.update_skill_use(&root, now_secs(), |used| {
            used.learned.push(approval.clone());
        });
        if let Err(e) = noted {
            tracing::warn!(session = id, "couldn't note the approval: {e:#}");
            return false;
        }
    }
    let known = registry.context.lock().unwrap().load(&root);
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (prompt, shown) = skill::prompt(&name, &known, &skill::digest(&tail), None);
    let dir = registry.host.context_dir.clone();
    let command = command.to_vec();
    let asked = tokio::task::spawn_blocking(move || {
        valkyrie_proto::ensure_private_dir(&dir)?;
        valkyrie_context::model::ask(&command, &prompt, &dir, MODEL_TIMEOUT)
    })
    .await;
    let reply = match asked.map_err(anyhow::Error::from).and_then(|r| r) {
        Ok(reply) => reply,
        Err(e) => {
            tracing::warn!(session = id, "learning failed: {e:#}");
            return true;
        }
    };
    let Some((found, patch)) = skill::parse_reply(&reply, &shown) else {
        return true;
    };
    if patch.is_none() && extract::already_known(&found.title, &known) {
        return true;
    }
    let new = NewDecision {
        cwd: info.cwd.clone(),
        title: found.title,
        body: format!(
            "{}\n\nLearned from the session {} after you said “{}”.",
            found.body,
            info.name,
            valkyrie_context::mine::quote(&said)
        ),
        kind: valkyrie_proto::DecisionKind::Skill,
        propose: true,
        supersedes: patch,
        review_every: None,
        session: Some(id),
    };
    let provenance = Provenance {
        by: "valkyrie".into(),
        session: Some(info.name.clone()),
        conversation: session.conversation(),
        commit: None,
        cwd: Some(info.cwd.clone()),
    };
    let decided = registry
        .context
        .lock()
        .unwrap()
        .decide(&new, provenance, now_secs());
    match decided {
        Ok(d) => {
            tracing::info!(
                session = id,
                id = d.id,
                "skill proposed from a finished job"
            );
            registry.refresh_proposals();
        }
        Err(e) => tracing::warn!(session = id, "couldn't propose a skill: {e:#}"),
    }
    true
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_tail(path: &Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(TAIL)));
    let mut raw = Vec::new();
    let _ = file.read_to_end(&mut raw);
    String::from_utf8_lossy(&raw).into_owned()
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

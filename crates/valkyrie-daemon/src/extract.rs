//! Catching decisions in your corrections (DESIGN §6.3). When an agent's turn ends,
//! the daemon reads the exchange around your last message. If that message reads
//! like a correction, a locked-down `claude -p --model haiku` is asked for the
//! lasting rule in it, which is then proposed for you to review. Never anything
//! more: its proposals wait on you like an agent's.

use crate::Registry;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use valkyrie_context::extract;
use valkyrie_proto::{NewDecision, Provenance, SessionId};

/// How much of a transcript's end is read: a few exchanges.
const TAIL: u64 = 256 * 1024;
/// Most model calls per hour, whatever happens.
const PER_HOUR: usize = 30;
const MODEL_TIMEOUT: Duration = Duration::from_secs(90);
/// Lets the agent finish writing its reply to the transcript after `Stop`.
const SETTLE: Duration = Duration::from_millis(500);

/// Whether catching is on: it is unless `valk decisions auto off` left this file.
pub fn off_marker(context_dir: &Path) -> PathBuf {
    context_dir.join("auto-off")
}

/// Takes `Stop`s (session ids) and proposes what it finds, one at a time.
pub(crate) async fn run(registry: Arc<Registry>, mut turns: mpsc::UnboundedReceiver<SessionId>) {
    let command = valkyrie_context::model::command(extract::SYSTEM);
    let mut last: HashMap<SessionId, u64> = HashMap::new();
    let mut calls: VecDeque<Instant> = VecDeque::new();
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
            if off_marker(&registry.host.context_dir).exists() {
                continue;
            }
            while calls
                .front()
                .is_some_and(|t| t.elapsed() > Duration::from_secs(3600))
            {
                calls.pop_front();
            }
            if calls.len() >= PER_HOUR {
                tracing::debug!(session = id, "extraction skipped: hourly limit");
                continue;
            }
            if look(&registry, id, &command, &mut last).await {
                calls.push_back(Instant::now());
            }
        }
    }
}

/// Looks at one session's last exchange; returns whether the model was asked.
async fn look(
    registry: &Registry,
    id: SessionId,
    command: &[String],
    last: &mut HashMap<SessionId, u64>,
) -> bool {
    let Ok(session) = registry.get(id) else {
        return false;
    };
    let info = session.info();
    if !matches!(info.status.agent.as_str(), "claude" | "codex") {
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
        let quote: String = yours.split_whitespace().collect::<Vec<_>>().join(" ");
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
            commit: None,
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

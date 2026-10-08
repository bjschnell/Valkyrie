//! When to buzz the phone. The server watches the daemon's queue. A session that
//! starts needing you (or finishes, or fails) becomes pending. Once you're away it
//! is pushed, once, to every subscribed device. "Away" means:
//! - nobody has typed into any session for `AWAY`;
//! - no web app is on screen (an app on screen shows the inbox itself, and anything
//!   it showed counts as seen);
//! - the item has been waiting `GRACE` (a desk that answers at once never pushes).

use crate::App;
use crate::auth::Subscription;
use crate::push::{MAX_PAYLOAD, Outcome, Urgency};
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use valkyrie_proto::client::Client;
use valkyrie_proto::{AgentState, AskKind, QueueItem, ServerMsg, SessionId};

/// How long without a keypress before you count as away.
const AWAY_MS: u64 = 90_000;
/// How long an item waits before it may push.
const GRACE_MS: u64 = 20_000;
/// How often pending items are checked against the above.
const TICK: Duration = Duration::from_secs(5);
/// How long a push service keeps an undelivered message: a prompt still matters
/// later, a "done" mostly doesn't.
const TTL_ASK: Duration = Duration::from_secs(12 * 3600);
const TTL_DONE: Duration = Duration::from_secs(3600);

/// What the service worker gets; it shows `title` and `body` and opens `session`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Note {
    pub title: String,
    pub body: String,
    pub session: Option<SessionId>,
    pub seq: u64,
    /// Notifications with one tag replace each other: one per session.
    pub tag: String,
    /// The queue's length, for the app icon's badge.
    pub badge: usize,
    #[serde(skip)]
    pub urgency: Urgency,
    /// Nothing better to say than "bell": the screen's last line says more.
    #[serde(skip)]
    pub from_screen: bool,
}

impl Note {
    pub fn payload(&self) -> Vec<u8> {
        let mut note = self.clone();
        loop {
            let bytes = serde_json::to_vec(&note).unwrap_or_default();
            if bytes.len() <= MAX_PAYLOAD || note.body.is_empty() {
                return bytes;
            }
            let keep = note.body.chars().count() / 2;
            note.body = note.body.chars().take(keep).collect::<String>() + "…";
        }
    }
}

struct Pending {
    item: QueueItem,
    since_ms: u64,
}

/// The decisions, apart from any I/O.
#[derive(Default)]
pub struct Watch {
    /// The latest transition looked at, per session: each is pushed at most once.
    handled: HashMap<SessionId, u64>,
    pending: HashMap<SessionId, Pending>,
    queue_len: usize,
    started: bool,
}

impl Watch {
    /// A new queue from the daemon. `visible`: a web app is on screen.
    pub fn queue(&mut self, items: &[QueueItem], visible: bool, now_ms: u64) {
        self.queue_len = items.len();
        // Gone from the queue, or moved on: nothing to push for it.
        self.pending.retain(|id, p| {
            items
                .iter()
                .any(|i| i.session == *id && i.status.seq == p.item.status.seq)
        });
        for item in items {
            let seq = item.status.seq;
            if self.handled.insert(item.session, seq) == Some(seq) {
                continue;
            }
            // What was already waiting when the server started isn't news.
            if self.started && !visible && worth_a_push(item) {
                tracing::debug!(session = item.session, seq, "pending a push");
                self.pending.insert(
                    item.session,
                    Pending {
                        item: item.clone(),
                        since_ms: now_ms,
                    },
                );
            }
        }
        self.started = true;
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// What to push now. `last_input_ms`: the latest keypress into any session.
    pub fn due(&mut self, now_ms: u64, last_input_ms: u64, visible: bool) -> Vec<Note> {
        if visible {
            // On screen in the app's inbox: seen.
            self.pending.clear();
            return Vec::new();
        }
        if now_ms.saturating_sub(last_input_ms) < AWAY_MS {
            return Vec::new();
        }
        let ready: Vec<SessionId> = self
            .pending
            .iter()
            .filter(|(_, p)| now_ms.saturating_sub(p.since_ms) >= GRACE_MS)
            .map(|(id, _)| *id)
            .collect();
        let mut notes: Vec<Note> = ready
            .iter()
            .filter_map(|id| self.pending.remove(id))
            .map(|p| note(&p.item, self.queue_len))
            .collect();
        notes.sort_by_key(|n| n.session);
        notes
    }
}

/// Needing you always is; finishing or failing is only if you didn't watch it.
fn worth_a_push(item: &QueueItem) -> bool {
    let s = &item.status;
    match s.state {
        AgentState::NeedsInput => !(s.seen && s.ask == Some(AskKind::Bell)),
        AgentState::ReviewReady | AgentState::Blocked => !s.seen,
        _ => false,
    }
}

fn note(item: &QueueItem, badge: usize) -> Note {
    let s = &item.status;
    let what = match (s.state, s.ask) {
        (AgentState::NeedsInput, Some(AskKind::Permission)) => "wants permission",
        (AgentState::NeedsInput, Some(AskKind::Question)) => "has a question",
        (AgentState::NeedsInput, _) => "needs you",
        (AgentState::Blocked, _) => "stopped",
        _ => "done",
    };
    let agent = match s.agent.as_str() {
        "claude" => "Claude",
        "codex" => "Codex",
        _ => "",
    };
    Note {
        title: if agent.is_empty() {
            format!("{} · {what}", item.name)
        } else {
            format!("{} · {agent} {what}", item.name)
        },
        body: s.summary.clone().unwrap_or_default(),
        session: Some(item.session),
        seq: s.seq,
        tag: format!("s{}", item.session),
        badge,
        urgency: if s.state == AgentState::NeedsInput {
            Urgency::High
        } else {
            Urgency::Normal
        },
        from_screen: s.summary.is_none() || matches!(s.ask, Some(AskKind::Bell | AskKind::Screen)),
    }
}

/// The last line with words on it, for a notification's body.
fn last_line(screen: &str) -> Option<String> {
    let line = screen
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| l.chars().any(char::is_alphanumeric))?;
    Some(if line.chars().count() > 160 {
        line.chars().take(160).collect::<String>() + "…"
    } else {
        line.to_owned()
    })
}

/// Watches the daemon for as long as the server runs, reconnecting after an
/// upgrade or restart.
pub(crate) async fn run(app: Arc<App>) {
    let mut watch = Watch::default();
    loop {
        if let Err(e) = watch_daemon(&app, &mut watch).await {
            tracing::debug!("queue watch: {e:#}");
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn watch_daemon(app: &Arc<App>, watch: &mut Watch) -> Result<()> {
    let (client, mut pushes) = Client::connect(&app.socket).await?;
    client.watch_queue().await?;
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            msg = pushes.recv() => match msg {
                Some(ServerMsg::Queue { items }) => watch.queue(&items, app.visible(), now_ms()),
                Some(_) => {}
                None => anyhow::bail!("daemon connection closed"),
            },
            _ = tick.tick() => {
                if !watch.has_pending() {
                    continue;
                }
                let last_input = client
                    .list()
                    .await?
                    .iter()
                    .map(|s| s.last_input_ms)
                    .max()
                    .unwrap_or(0);
                for mut note in watch.due(now_ms(), last_input, app.visible()) {
                    if note.from_screen
                        && let Some(id) = note.session
                        && let Ok(text) = client.dump(id).await
                        && let Some(line) = last_line(&text)
                    {
                        note.body = line;
                    }
                    let d = send(app, &note, None).await;
                    tracing::info!(session = note.session, sent = d.sent, of = d.subscribed, "pushed");
                }
            }
        }
    }
}

/// How one notification went.
#[derive(Debug, Default, Serialize)]
pub struct Delivery {
    pub subscribed: usize,
    pub sent: usize,
    pub errors: Vec<String>,
}

/// Pushes to every subscribed device, or only to `device`'s; forgets subscriptions
/// the push service says are gone.
pub(crate) async fn send(app: &App, note: &Note, device: Option<&str>) -> Delivery {
    let mut delivery = Delivery::default();
    let Some(sender) = &app.push else {
        delivery
            .errors
            .push("notifications are off on this server".into());
        return delivery;
    };
    let subs: Vec<Subscription> = app
        .store
        .subscriptions()
        .into_iter()
        .filter(|s| device.is_none_or(|d| s.device == d))
        .collect();
    let payload = note.payload();
    let ttl = if note.urgency == Urgency::High {
        TTL_ASK
    } else {
        TTL_DONE
    };
    delivery.subscribed = subs.len();
    for sub in subs {
        match sender
            .send(&sub, &payload, &note.tag, note.urgency, ttl)
            .await
        {
            Ok(Outcome::Sent) => delivery.sent += 1,
            Ok(Outcome::Gone) => {
                tracing::info!("push subscription gone; dropping it");
                let _ = app.store.unsubscribe(&sub.endpoint);
            }
            Err(e) => {
                tracing::warn!("push failed: {e:#}");
                delivery.errors.push(format!("{e:#}"));
            }
        }
    }
    delivery
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::AgentStatus;

    fn item(session: SessionId, state: AgentState, seq: u64, seen: bool) -> QueueItem {
        QueueItem {
            session,
            name: format!("proj{session}"),
            cwd: "/".into(),
            status: AgentStatus {
                agent: "claude".into(),
                state,
                ask: (state == AgentState::NeedsInput).then_some(AskKind::Permission),
                summary: Some("Bash: cargo test".into()),
                seq,
                seen,
                ..AgentStatus::default()
            },
        }
    }

    const AWAY: u64 = 1_000_000; // last keypress long before `now`

    #[test]
    fn a_prompt_pushes_once_when_you_are_away() {
        let mut w = Watch::default();
        w.queue(&[], false, 0);
        w.queue(&[item(1, AgentState::NeedsInput, 5, false)], false, AWAY);
        assert!(w.due(AWAY + 1_000, 0, false).is_empty(), "within the grace");
        let notes = w.due(AWAY + GRACE_MS, 0, false);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "proj1 · Claude wants permission");
        assert_eq!(notes[0].body, "Bash: cargo test");
        assert_eq!(notes[0].urgency, Urgency::High);
        assert!(w.due(AWAY + 2 * GRACE_MS, 0, false).is_empty(), "once");
        // The same transition again is not news; the next one is.
        w.queue(&[item(1, AgentState::NeedsInput, 5, false)], false, AWAY);
        assert!(!w.has_pending());
        w.queue(&[item(1, AgentState::ReviewReady, 6, false)], false, AWAY);
        assert!(w.has_pending());
    }

    #[test]
    fn typing_holds_it_back_until_you_stop() {
        let mut w = Watch::default();
        w.queue(&[], false, 0);
        w.queue(&[item(1, AgentState::NeedsInput, 1, true)], false, AWAY);
        let now = AWAY + GRACE_MS;
        assert!(w.due(now, now - 1_000, false).is_empty(), "at the keyboard");
        assert_eq!(
            w.due(now + AWAY_MS, now - 1_000, false).len(),
            1,
            "walked off"
        );
    }

    #[test]
    fn answered_or_shown_in_the_app_is_not_pushed() {
        let mut w = Watch::default();
        w.queue(&[], false, 0);
        w.queue(&[item(1, AgentState::NeedsInput, 1, false)], false, AWAY);
        w.queue(&[], false, AWAY + 1); // answered at the desk
        assert!(w.due(AWAY + GRACE_MS, 0, false).is_empty());

        w.queue(&[item(2, AgentState::NeedsInput, 1, false)], false, AWAY);
        assert!(w.due(AWAY + GRACE_MS, 0, true).is_empty(), "app on screen");
        assert!(
            w.due(AWAY + 2 * GRACE_MS, 0, false).is_empty(),
            "already seen"
        );

        w.queue(&[item(3, AgentState::NeedsInput, 1, false)], true, AWAY);
        assert!(!w.has_pending(), "arrived while the app was open");
    }

    #[test]
    fn a_finish_you_watched_and_old_news_stay_quiet() {
        let mut w = Watch::default();
        w.queue(&[item(1, AgentState::NeedsInput, 1, false)], false, 0);
        assert!(!w.has_pending(), "waiting before the server started");
        w.queue(&[item(2, AgentState::ReviewReady, 1, true)], false, AWAY);
        assert!(!w.has_pending(), "finished while you watched");
        w.queue(&[item(3, AgentState::Interrupted, 1, false)], false, AWAY);
        assert!(!w.has_pending(), "a guess, not worth a buzz");
    }

    #[test]
    fn a_bell_says_what_the_screen_says() {
        let mut bell = item(1, AgentState::NeedsInput, 1, false);
        bell.status.ask = Some(AskKind::Bell);
        assert!(note(&bell, 1).from_screen);
        assert!(!note(&item(1, AgentState::NeedsInput, 1, false), 1).from_screen);
        assert_eq!(
            last_line("$ ./deploy.sh\nApprove the deploy? [y/N] \n\n  ──── \n"),
            Some("Approve the deploy? [y/N]".into())
        );
        assert_eq!(last_line("\n  \n"), None);
    }

    #[test]
    fn a_long_summary_is_cut_to_fit_a_push() {
        let mut n = note(&item(1, AgentState::NeedsInput, 1, false), 1);
        n.body = "é".repeat(5000);
        let bytes = n.payload();
        assert!(bytes.len() <= MAX_PAYLOAD);
        let back: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(back["body"].as_str().unwrap().ends_with('…'));
        assert_eq!(back["session"], 1);
    }
}

//! When to buzz the phone. The server watches the daemon's queue. A session that
//! starts needing you (or finishes, or fails) becomes pending. Once you're away it
//! is pushed, once, to every subscribed device. "Away" means:
//! - nobody has typed into any session for `AWAY`;
//! - no web app is on screen (an app on screen shows the inbox itself, and anything
//!   it showed counts as seen);
//! - the item has been waiting `GRACE` (a desk that answers at once never pushes).
//!
//! A supervisor's flag (ADR-0019 §5) names its reason, and one still unseen an
//! hour after its push pushes once more.

use crate::App;
use crate::auth::Subscription;
use crate::push::{MAX_PAYLOAD, Outcome, Urgency};
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use valkyrie_proto::client::{Client, DaemonInfo};
use valkyrie_proto::{
    AgentState, AskKind, ClientBuild, Decision, LaunchOffer, NoticeSound, QueueItem, ServerMsg,
    SessionId,
};

/// How long without a keypress before you count as away.
const AWAY_MS: u64 = 90_000;
/// How long an item waits before it may push.
const GRACE_MS: u64 = 20_000;
/// How long a pushed flag may go unseen before it pushes once more.
const REMIND_MS: u64 = 3_600_000;
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
    /// Decisions waiting on review, by (project, id, updated): each is news once.
    decisions_known: std::collections::HashSet<(std::path::PathBuf, u32, u64)>,
    /// New ones since the last push about them, and when the first arrived.
    decisions_new: Vec<((std::path::PathBuf, u32, u64), String)>,
    decisions_since: u64,
    decisions_started: bool,
    /// Flags pushed and not seen yet, with when they push again: once each.
    reminders: HashMap<SessionId, (QueueItem, u64)>,
    /// Launch offers seen, by id: each is news once.
    launches_known: std::collections::HashSet<u64>,
    /// New ones not pushed yet, and when each arrived.
    launches_new: Vec<(LaunchOffer, u64)>,
    launches_started: bool,
}

impl Watch {
    /// The push for a `valk notify`. Always sent, even with the app open: nothing in
    /// the app shows a notice. Its tag is its own, so notices don't replace each
    /// other; asking for you is urgent.
    pub fn notice(&self, text: &str, sound: Option<NoticeSound>, now_ms: u64) -> Note {
        Note {
            title: "Valkyrie".into(),
            body: text.into(),
            session: None,
            seq: 0,
            tag: format!("notice-{now_ms}"),
            badge: self.queue_len,
            urgency: if sound == Some(NoticeSound::Request) {
                Urgency::High
            } else {
                Urgency::Normal
            },
            from_screen: false,
        }
    }

    /// A new queue from the daemon. `visible`: a web app is on screen.
    pub fn queue(&mut self, items: &[QueueItem], visible: bool, now_ms: u64) {
        self.queue_len = items.len();
        // Gone from the queue, or moved on: nothing to push for it.
        self.pending.retain(|id, p| {
            items
                .iter()
                .any(|i| i.session == *id && i.status.seq == p.item.status.seq)
        });
        // Seen, moved on or gone: no reminder.
        self.reminders.retain(|id, (r, _)| {
            items.iter().any(|i| {
                i.session == *id
                    && i.status.seq == r.status.seq
                    && !i.status.seen
                    && i.flag.is_some()
            })
        });
        for item in items {
            let seq = item.status.seq;
            if self.handled.insert(item.session, seq) == Some(seq) {
                // The same transition, filled in since (a flag's step): push that.
                if let Some(p) = self.pending.get_mut(&item.session) {
                    p.item = item.clone();
                }
                if let Some((r, _)) = self.reminders.get_mut(&item.session) {
                    *r = item.clone();
                }
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

    /// The decisions waiting on review (ADR-0007): new ones are pending, as
    /// sessions are, so you hear about them once you're away.
    pub fn proposals(&mut self, items: &[Decision], visible: bool, now_ms: u64) {
        let keys: std::collections::HashSet<_> = items
            .iter()
            .map(|d| (d.project.clone(), d.id, d.updated))
            .collect();
        // Reviewed meanwhile: no longer news. First, so a new one arriving in the
        // same push starts its own grace.
        self.decisions_new.retain(|(key, _)| keys.contains(key));
        for d in items {
            let key = (d.project.clone(), d.id, d.updated);
            if self.decisions_known.contains(&key) || !self.decisions_started || visible {
                continue;
            }
            if self.decisions_new.is_empty() {
                self.decisions_since = now_ms;
            }
            self.decisions_new.push((key, d.title.clone()));
        }
        self.decisions_known = keys;
        self.decisions_started = true;
    }

    /// The ready items the supervisor offers to start (ADR-0019 §2): a new one is
    /// news once, after the same grace, so one you start or leave at once, or that
    /// the next look replaces, stays quiet.
    pub fn launches(&mut self, offers: &[LaunchOffer], visible: bool, now_ms: u64) {
        let ids: std::collections::HashSet<u64> = offers.iter().map(|o| o.id).collect();
        self.launches_new.retain(|(o, _)| ids.contains(&o.id));
        for o in offers {
            if self.launches_known.contains(&o.id) || !self.launches_started || visible {
                continue;
            }
            self.launches_new.push((o.clone(), now_ms));
        }
        self.launches_known = ids;
        self.launches_started = true;
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
            || !self.decisions_new.is_empty()
            || !self.reminders.is_empty()
            || !self.launches_new.is_empty()
    }

    /// What to push now. `last_input_ms`: the latest keypress into any session.
    pub fn due(&mut self, now_ms: u64, last_input_ms: u64, visible: bool) -> Vec<Note> {
        if visible {
            // On screen in the app's inbox: seen.
            self.pending.clear();
            self.decisions_new.clear();
            self.launches_new.clear();
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
        let mut notes: Vec<Note> = Vec::new();
        for p in ready.iter().filter_map(|id| self.pending.remove(id)) {
            if p.item.flag.is_some() {
                self.reminders
                    .insert(p.item.session, (p.item.clone(), now_ms + REMIND_MS));
            }
            notes.push(note(&p.item, self.queue_len));
        }
        // Flags pushed an hour ago and still unseen: once more, then not again.
        let again: Vec<SessionId> = self
            .reminders
            .iter()
            .filter(|(_, (_, at))| now_ms >= *at)
            .map(|(id, _)| *id)
            .collect();
        for id in again {
            if let Some((item, _)) = self.reminders.remove(&id) {
                let mut n = note(&item, self.queue_len);
                n.title = format!("Still waiting: {}", n.title);
                notes.push(n);
            }
        }
        notes.sort_by_key(|n| n.session);
        let (ripe, young): (Vec<_>, Vec<_>) = std::mem::take(&mut self.launches_new)
            .into_iter()
            .partition(|(_, at)| now_ms.saturating_sub(*at) >= GRACE_MS);
        self.launches_new = young;
        for (o, _) in ripe {
            notes.push(Note {
                title: format!("Ready to start: #{} {}", o.item, o.title),
                body: o.why.clone(),
                session: None,
                seq: 0,
                tag: format!("launch-{}", o.item),
                badge: self.queue_len,
                urgency: Urgency::Normal,
                from_screen: false,
            });
        }
        if !self.decisions_new.is_empty() && now_ms.saturating_sub(self.decisions_since) >= GRACE_MS
        {
            let titles: Vec<String> = std::mem::take(&mut self.decisions_new)
                .into_iter()
                .map(|(_, title)| title)
                .collect();
            notes.push(Note {
                title: match titles.len() {
                    1 => "A decision to review".into(),
                    n => format!("{n} decisions to review"),
                },
                body: titles.join(" · "),
                session: None,
                seq: 0,
                tag: "decisions".into(),
                badge: self.queue_len,
                urgency: Urgency::Normal,
                from_screen: false,
            });
        }
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
    // A supervisor's flag names its reason, and what it offers to do about it.
    if let Some(flag) = &item.flag {
        let body = match &flag.step {
            Some(step) => format!("Offers: {} ({})", step.action.label(), step.why),
            None => String::new(),
        };
        return Note {
            title: format!("{} · {}", item.name, flag.reason),
            body,
            session: Some(item.session),
            seq: s.seq,
            tag: format!("s{}", item.session),
            badge,
            urgency: Urgency::Normal,
            from_screen: false,
        };
    }
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
/// A screen's last line with something on it: what a notification says when
/// the agent gave nothing better (`Note::from_screen`).
pub fn last_line(screen: &str) -> Option<String> {
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
    let mut daemon = None;
    loop {
        if let Err(e) = watch_daemon(&app, &mut watch, &mut daemon).await {
            tracing::debug!("queue watch: {e:#}");
        }
        // Whatever takes the socket next has to say it's a development daemon again.
        app.set_dev(false);
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn watch_daemon(
    app: &Arc<App>,
    watch: &mut Watch,
    daemon: &mut Option<DaemonInfo>,
) -> Result<()> {
    let (client, mut pushes) = Client::connect(&app.socket).await?;
    let info = client.hello_as(ClientBuild::this("web")).await?;
    app.set_dev(info.dev);
    // Another protocol is only followed to the daemon's own binary: this one,
    // unreplaced, would just fail to start against it.
    let upgraded = daemon
        .as_ref()
        .is_some_and(|before| info.upgraded_from(before))
        || (info.protocol != valkyrie_proto::PROTOCOL && info.follow(None).is_some());
    *daemon = Some(info.clone());
    if upgraded {
        *app.upgraded_to.lock().unwrap() = Some(info);
        app.upgraded.notify_one();
    }
    client.watch_queue().await?;
    // Relays notices as pushes without being counted as someone who sees them. An
    // older daemon doesn't know them, and the queue still works.
    let _ = client.watch_notices(false).await;
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            msg = pushes.recv() => match msg {
                Some(ServerMsg::Queue { items }) => watch.queue(&items, app.visible(), now_ms()),
                Some(ServerMsg::Proposals { items }) => {
                    watch.proposals(&items, app.visible(), now_ms())
                }
                Some(ServerMsg::Items { launches, .. }) => {
                    watch.launches(&launches, app.visible(), now_ms())
                }
                Some(ServerMsg::Notice { text, sound }) => {
                    let note = watch.notice(&text, sound, now_ms());
                    let d = send(app, &note, None).await;
                    tracing::info!(sent = d.sent, of = d.subscribed, "pushed a notice");
                }
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
            auto_name: false,
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
            flag: None,
            report: None,
        }
    }

    #[test]
    fn a_notice_is_its_own_push_and_asking_is_urgent() {
        let mut w = Watch::default();
        w.queue(&[item(1, AgentState::NeedsInput, 1, false)], false, 0);
        let n = w.notice("deploy done", Some(NoticeSound::Request), 7);
        assert_eq!(
            (n.title.as_str(), n.body.as_str()),
            ("Valkyrie", "deploy done")
        );
        assert_eq!((n.session, n.tag.as_str(), n.badge), (None, "notice-7", 1));
        assert_eq!(n.urgency, Urgency::High);
        for sound in [Some(NoticeSound::Done), None] {
            assert_eq!(w.notice("x", sound, 8).urgency, Urgency::Normal);
        }
        assert_ne!(w.notice("x", None, 8).tag, w.notice("x", None, 9).tag);
    }

    const AWAY: u64 = 1_000_000; // last keypress long before `now`

    fn decision(id: u32, title: &str) -> Decision {
        Decision {
            id,
            project: "/r".into(),
            title: title.into(),
            body: String::new(),
            kind: Default::default(),
            status: Default::default(),
            created: 0,
            updated: 1,
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

    #[test]
    fn new_decisions_push_once_together_when_you_are_away() {
        let mut w = Watch::default();
        w.proposals(&[decision(1, "Old one")], false, 0);
        w.proposals(
            &[decision(1, "Old one"), decision(2, "Use pnpm")],
            false,
            AWAY,
        );
        w.proposals(
            &[
                decision(1, "Old one"),
                decision(2, "Use pnpm"),
                decision(3, "No pkill -f"),
            ],
            false,
            AWAY + 1,
        );
        assert!(w.has_pending());
        assert!(w.due(AWAY + 1_000, 0, false).is_empty(), "within the grace");
        let notes = w.due(AWAY + GRACE_MS, 0, false);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "2 decisions to review");
        assert_eq!(notes[0].body, "Use pnpm · No pkill -f");
        assert_eq!(notes[0].session, None);
        assert!(w.due(AWAY + 2 * GRACE_MS, 0, false).is_empty(), "once");
        // Reviewed before the push: nothing to say.
        w.proposals(&[decision(4, "Another"), decision(5, "Kept")], false, AWAY);
        w.proposals(&[decision(5, "Kept")], false, AWAY + 1);
        let notes = w.due(AWAY + GRACE_MS * 3, 0, false);
        assert_eq!(notes[0].title, "A decision to review");
        assert_eq!(notes[0].body, "Kept");
        w.proposals(&[decision(6, "Gone")], false, AWAY);
        w.proposals(&[], false, AWAY + 1);
        assert!(w.due(AWAY + GRACE_MS * 3, 0, false).is_empty());
    }

    #[test]
    fn a_ready_item_pushes_once_after_the_grace_unless_answered() {
        let offer = |id: u64, item: u32| LaunchOffer {
            id,
            project: "/r".into(),
            item,
            title: format!("item {item}"),
            agent: "claude".into(),
            model: String::new(),
            why: "next in the backlog; 1 of 3 agent sessions running".into(),
            waived: Vec::new(),
            made: 0,
        };
        let mut w = Watch::default();
        // Waiting when the server started: not news.
        w.launches(&[offer(1, 4)], false, 0);
        w.launches(&[offer(1, 4), offer(2, 5)], false, AWAY);
        assert!(w.due(AWAY + 1_000, 0, false).is_empty(), "within the grace");
        let notes = w.due(AWAY + GRACE_MS, 0, false);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Ready to start: #5 item 5");
        assert!(notes[0].body.contains("1 of 3"));
        assert!(w.due(AWAY + 2 * GRACE_MS, 0, false).is_empty(), "once");
        // Started or left before the push: nothing to say.
        w.launches(&[offer(3, 6)], false, AWAY);
        w.launches(&[], false, AWAY + 1);
        assert!(!w.has_pending());
    }

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
    fn a_flag_names_its_reason_and_reminds_once_an_hour_on() {
        use valkyrie_proto::{Flag, Step, StepAction};
        let flagged = |step: bool, seen: bool| {
            let mut i = item(1, AgentState::Blocked, 7, seen);
            i.flag = Some(Flag {
                reason: "PR #98 conflicts with main".into(),
                pr: "https://github.com/o/r/pull/98".into(),
                seq: 7,
                step: step.then(|| Step {
                    action: StepAction::MergeBase,
                    why: "the branch conflicts with its base".into(),
                    alt: None,
                }),
                working: !step,
            });
            i
        };
        let mut w = Watch::default();
        w.queue(&[], false, 0);
        w.queue(&[flagged(false, false)], false, AWAY);
        // Its step is worked out within the grace: the push says it.
        w.queue(&[flagged(true, false)], false, AWAY + 1);
        let notes = w.due(AWAY + GRACE_MS, 0, false);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "proj1 · PR #98 conflicts with main");
        assert_eq!(
            notes[0].body,
            "Offers: ask its agent to merge the base in (the branch conflicts with its base)"
        );
        assert!(w.has_pending(), "a reminder waits");
        assert!(w.due(AWAY + GRACE_MS + REMIND_MS - 1, 0, false).is_empty());
        let again = w.due(AWAY + GRACE_MS + REMIND_MS, 0, false);
        assert_eq!(again.len(), 1);
        assert!(
            again[0].title.starts_with("Still waiting: proj1 · PR #98"),
            "{}",
            again[0].title
        );
        assert!(
            w.due(AWAY + GRACE_MS + 3 * REMIND_MS, 0, false).is_empty(),
            "once"
        );
        // Seen meanwhile: no reminder.
        let mut w = Watch::default();
        w.queue(&[], false, 0);
        w.queue(&[flagged(true, false)], false, AWAY);
        assert_eq!(w.due(AWAY + GRACE_MS, 0, false).len(), 1);
        w.queue(&[flagged(true, true)], false, AWAY + GRACE_MS + 1);
        assert!(!w.has_pending());
        assert!(w.due(AWAY + GRACE_MS + REMIND_MS, 0, false).is_empty());
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

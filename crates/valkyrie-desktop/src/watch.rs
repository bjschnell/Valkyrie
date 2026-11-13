//! The shell as a client of the daemon (ADR-0020 §6): the tray's count of
//! sessions that need you, and native notifications for them. Its own
//! connection over the socket, like the TUI's, not a bridge from the page.
//!
//! When to notify is the web app's rule for a phone's push
//! (`valkyrie_web::notify`), with the window in front standing for an app on
//! screen: nothing while you're looking at the window or typing into a
//! session, and each thing once.

use anyhow::Result;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::UnboundedSender;
use valkyrie_proto::client::Client;
use valkyrie_proto::{ClientBuild, ServerMsg};
use valkyrie_web::notify::{Note, Watch, last_line};

/// How often waiting items are checked against "are you away".
const TICK: Duration = Duration::from_secs(5);
/// How long before a lost daemon is tried again.
const RETRY: Duration = Duration::from_secs(2);

/// What the tray and the notification centre are told.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The daemon answers, or doesn't. The first event is always one of these.
    Online(bool),
    /// The daemon was upgraded past this app: it speaks another protocol, and
    /// nothing more is known until the app is restarted on the new version.
    Outdated,
    /// How many sessions need you: the number on the tray icon.
    Count(usize),
    /// Show this; clicking it opens `Note::session`.
    Notify(Note),
}

/// The decisions, apart from any I/O.
#[derive(Default)]
pub struct Tray {
    watch: Watch,
    count: Option<usize>,
}

impl Tray {
    /// What a push from the daemon changes. `front`: the window is in front.
    pub fn on(&mut self, msg: &ServerMsg, front: bool, now_ms: u64) -> Vec<Event> {
        match msg {
            ServerMsg::Queue { items } => {
                self.watch.queue(items, front, now_ms);
                if self.count == Some(items.len()) {
                    return Vec::new();
                }
                self.count = Some(items.len());
                vec![Event::Count(items.len())]
            }
            ServerMsg::Proposals { items } => {
                self.watch.proposals(items, front, now_ms);
                Vec::new()
            }
            ServerMsg::Items { launches, .. } => {
                self.watch.launches(launches, front, now_ms);
                Vec::new()
            }
            // A `valk notify` is for you wherever you are looking.
            ServerMsg::Notice { text, sound } => {
                vec![Event::Notify(self.watch.notice(text, *sound, now_ms))]
            }
            _ => Vec::new(),
        }
    }

    /// Something waits that may be worth a notification once you're away.
    pub fn waiting(&self) -> bool {
        self.watch.has_pending()
    }

    /// What to show now. `last_input_ms`: the latest keypress into any session.
    pub fn due(&mut self, now_ms: u64, last_input_ms: u64, front: bool) -> Vec<Note> {
        self.watch.due(now_ms, last_input_ms, front)
    }

    /// The daemon is gone: the count is unknown until it is back.
    fn lost(&mut self) {
        self.count = None;
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Follows the daemon on `socket` for as long as `events` is listened to,
/// reconnecting when it goes away (an upgrade restarts it). `front` says whether
/// the window is in front right now.
pub async fn run(socket: &Path, front: impl Fn() -> bool, events: UnboundedSender<Event>) {
    let mut tray = Tray::default();
    // Unknown until the first try: `None` so that it is told either way.
    let mut online = None;
    let mut outdated = false;
    loop {
        let ended = follow(socket, &mut tray, &front, &events, &mut online).await;
        if events.is_closed() {
            return;
        }
        match ended {
            Err(e) if e.is::<Outdated>() => {
                // Said once; a restart of the app is the only way on.
                if !outdated {
                    outdated = true;
                    let _ = events.send(Event::Outdated);
                }
            }
            Err(e) => tracing::debug!("daemon connection: {e:#}"),
            Ok(()) => {}
        }
        tray.lost();
        if online != Some(false) {
            online = Some(false);
            let _ = events.send(Event::Online(false));
        }
        tokio::time::sleep(RETRY).await;
    }
}

/// The daemon speaks another protocol than this build.
#[derive(Debug)]
struct Outdated;

impl std::fmt::Display for Outdated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the daemon speaks another protocol than this app")
    }
}

impl std::error::Error for Outdated {}

async fn follow(
    socket: &Path,
    tray: &mut Tray,
    front: &impl Fn() -> bool,
    events: &UnboundedSender<Event>,
    online: &mut Option<bool>,
) -> Result<()> {
    let (client, mut pushes) = Client::connect(socket).await?;
    let daemon = client.hello_as(ClientBuild::this("desktop")).await?;
    // Its messages may not even decode: better no count than a wrong one that
    // comes and goes.
    if daemon.protocol != valkyrie_proto::PROTOCOL {
        return Err(Outdated.into());
    }
    client.watch_queue().await?;
    // It shows notices to someone, so `valk notify` counts it. An older daemon
    // doesn't know them, and the queue still works.
    let _ = client.watch_notices(true).await;
    *online = Some(true);
    let _ = events.send(Event::Online(true));
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            msg = pushes.recv() => {
                let Some(msg) = msg else {
                    anyhow::bail!("daemon connection closed");
                };
                for event in tray.on(&msg, front(), now_ms()) {
                    if events.send(event).is_err() {
                        return Ok(());
                    }
                }
            }
            _ = tick.tick() => {
                if events.is_closed() {
                    return Ok(());
                }
                if !tray.waiting() {
                    continue;
                }
                let last_input = client
                    .list()
                    .await?
                    .iter()
                    .map(|s| s.last_input_ms)
                    .max()
                    .unwrap_or(0);
                for mut note in tray.due(now_ms(), last_input, front()) {
                    // Nothing better to say than "bell": the screen says more.
                    if note.from_screen
                        && let Some(id) = note.session
                        && let Ok(text) = client.dump(id).await
                        && let Some(line) = last_line(&text)
                    {
                        note.body = line;
                    }
                    if events.send(Event::Notify(note)).is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::{AgentState, AgentStatus, AskKind, NoticeSound, QueueItem, SessionId};

    fn item(session: SessionId, seq: u64) -> QueueItem {
        QueueItem {
            session,
            name: format!("proj{session}"),
            auto_name: false,
            cwd: "/".into(),
            status: AgentStatus {
                agent: "claude".into(),
                state: AgentState::NeedsInput,
                ask: Some(AskKind::Permission),
                summary: Some("Bash: cargo test".into()),
                seq,
                seen: false,
                ..AgentStatus::default()
            },
            flag: None,
            report: None,
        }
    }

    fn queue(items: Vec<QueueItem>) -> ServerMsg {
        ServerMsg::Queue { items }
    }

    const MINUTE: u64 = 60_000;

    #[test]
    fn the_count_follows_the_queue_and_says_so_once() {
        let mut tray = Tray::default();
        // The first queue is a count too, even an empty one.
        assert_eq!(tray.on(&queue(vec![]), false, 0), [Event::Count(0)]);
        assert_eq!(
            tray.on(&queue(vec![item(1, 1)]), false, 0),
            [Event::Count(1)]
        );
        assert_eq!(tray.on(&queue(vec![item(1, 2)]), false, 0), []);
        assert_eq!(tray.on(&queue(vec![]), false, 0), [Event::Count(0)]);
        // After the daemon went away the next queue is news again.
        tray.lost();
        assert_eq!(tray.on(&queue(vec![]), false, 0), [Event::Count(0)]);
    }

    #[test]
    fn a_session_that_needs_you_notifies_once_you_are_away() {
        let mut tray = Tray::default();
        tray.on(&queue(vec![]), false, 0);
        tray.on(&queue(vec![item(1, 1)]), false, 10 * MINUTE);
        assert!(tray.waiting());
        // Typing into a session a moment ago: at the keyboard, not away.
        assert_eq!(tray.due(11 * MINUTE, 11 * MINUTE - 1000, false), []);
        let notes = tray.due(13 * MINUTE, 11 * MINUTE, false);
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].session, Some(1));
        // Once.
        assert_eq!(tray.due(20 * MINUTE, 11 * MINUTE, false), []);
    }

    #[test]
    fn nothing_notifies_while_the_window_is_in_front() {
        let mut tray = Tray::default();
        tray.on(&queue(vec![]), true, 0);
        tray.on(&queue(vec![item(1, 1)]), true, 10 * MINUTE);
        assert_eq!(tray.due(13 * MINUTE, 0, true), []);
        // Seen there: looking away afterwards doesn't bring it back.
        assert_eq!(tray.due(20 * MINUTE, 0, false), []);
    }

    #[test]
    fn a_notice_shows_wherever_you_are_looking() {
        let mut tray = Tray::default();
        let msg = ServerMsg::Notice {
            text: "deploy done".into(),
            sound: Some(NoticeSound::Done),
        };
        let events = tray.on(&msg, true, 5);
        let [Event::Notify(note)] = events.as_slice() else {
            panic!("{events:?}");
        };
        assert_eq!((note.body.as_str(), note.session), ("deploy done", None));
    }
}

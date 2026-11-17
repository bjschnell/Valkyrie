//! The shell as a client of the daemon (ADR-0020 §6): the tray's count of
//! sessions that need you, and native notifications for them. Its own
//! connection over the socket, like the TUI's, not a bridge from the page.
//! Each other machine with a window open is followed too, through
//! `ssh HOST valk bridge` (backlog #226): its count adds to the tray's, and its
//! notifications say which machine they're from.
//!
//! When to notify is the web app's rule for a phone's push
//! (`valkyrie_web::notify`), with the window in front standing for an app on
//! screen: nothing while you're looking at the window or typing into a
//! session, and each thing once.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::UnboundedSender;
use valkyrie_proto::client::{Client, Pushes};
use valkyrie_proto::remote::{self, Auth, Host};
use valkyrie_proto::{ClientBuild, ServerMsg};
use valkyrie_web::notify::{Note, Watch, last_line};

/// How often waiting items are checked against "are you away".
const TICK: Duration = Duration::from_secs(5);
/// How long before a lost daemon is tried again.
const RETRY: Duration = Duration::from_secs(2);
/// The same for another machine's: each try is a login there.
const RETRY_REMOTE: Duration = Duration::from_secs(15);

/// A daemon to follow.
#[derive(Debug, Clone)]
pub enum Source {
    /// This computer's, on its socket.
    Local(PathBuf),
    /// Another machine's, through `ssh HOST valk bridge`. Never asks for a
    /// password: the app has no terminal.
    Remote(Host),
}

impl Source {
    /// The machine, for what it tells: `None` for this one.
    pub fn host(&self) -> Option<&Host> {
        match self {
            Source::Local(_) => None,
            Source::Remote(host) => Some(host),
        }
    }
}

/// What a source said, and which one: `None` is this computer.
pub type Report = (Option<Host>, Event);

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
    /// Not followed any more: another machine's window closed. Never sent by
    /// `run`; whoever stops it says so.
    Gone,
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

/// Follows `source`'s daemon for as long as `events` is listened to,
/// reconnecting when it goes away (an upgrade restarts it). `front` says whether
/// its window is in front right now.
pub async fn run(source: Source, front: impl Fn() -> bool, events: UnboundedSender<Report>) {
    let from = source.host().cloned();
    let send = |event: Event| events.send((from.clone(), event)).is_ok();
    let mut tray = Tray::default();
    // Unknown until the first try: `None` so that it is told either way.
    let mut online = None;
    let mut outdated = false;
    loop {
        let ended = follow(&source, &mut tray, &front, &events, &mut online).await;
        if events.is_closed() {
            return;
        }
        match ended {
            Err(e) if e.is::<Outdated>() => {
                // Said once; a restart of the app is the only way on.
                if !outdated {
                    outdated = true;
                    send(Event::Outdated);
                }
            }
            Err(e) => tracing::debug!(host = ?from, "daemon connection: {e:#}"),
            Ok(()) => {}
        }
        tray.lost();
        if online != Some(false) {
            online = Some(false);
            send(Event::Online(false));
        }
        tokio::time::sleep(match source {
            Source::Local(_) => RETRY,
            Source::Remote(_) => RETRY_REMOTE,
        })
        .await;
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

/// A connection to `source`'s daemon, and the `ssh` that carries it, if one
/// does: it lives as long as the connection is used, and is killed with it.
async fn connect(source: &Source) -> Result<(Client, Pushes, Option<tokio::process::Child>)> {
    match source {
        Source::Local(socket) => {
            let (client, pushes) = Client::connect(socket).await?;
            Ok((client, pushes, None))
        }
        Source::Remote(host) => {
            let mut ssh = tokio::process::Command::from(remote::bridge(host, Auth::Never))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("couldn't run ssh")?;
            let (Some(up), Some(down)) = (ssh.stdin.take(), ssh.stdout.take()) else {
                anyhow::bail!("ssh has no pipes");
            };
            // Read, or a full pipe stops `ssh`; kept for whoever looks.
            if let Some(err) = ssh.stderr.take() {
                let host = host.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    let mut lines = tokio::io::BufReader::new(err).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::debug!(%host, "ssh: {line}");
                    }
                });
            }
            let (client, pushes) = Client::over(down, up, false);
            Ok((client, pushes, Some(ssh)))
        }
    }
}

async fn follow(
    source: &Source,
    tray: &mut Tray,
    front: &impl Fn() -> bool,
    events: &UnboundedSender<Report>,
    online: &mut Option<bool>,
) -> Result<()> {
    let send = |event: Event| events.send((source.host().cloned(), event)).is_ok();
    let (client, mut pushes, _ssh) = connect(source).await?;
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
    send(Event::Online(true));
    let mut tick = tokio::time::interval(TICK);
    loop {
        tokio::select! {
            msg = pushes.recv() => {
                let Some(msg) = msg else {
                    anyhow::bail!("daemon connection closed");
                };
                for event in tray.on(&msg, front(), now_ms()) {
                    if !send(event) {
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
                    if !send(Event::Notify(note)) {
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// The tray's label, from every machine followed.
#[derive(Debug, Default)]
pub struct Board {
    /// This computer's daemon: `None` until it said, or while it's gone.
    here: Option<usize>,
    online: Option<bool>,
    outdated: bool,
    /// Other machines followed, with their count while known.
    there: BTreeMap<String, There>,
}

#[derive(Debug, Default)]
struct There {
    count: Option<usize>,
    outdated: bool,
}

/// What the tray shows: its tooltip, and the title beside the icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub tooltip: String,
    pub title: Option<String>,
}

impl Board {
    /// The label after what `from` said, when it changes it; the
    /// notifications are the caller's.
    pub fn on(&mut self, from: Option<&Host>, event: &Event, name: &str) -> Option<Label> {
        let before = self.label(name);
        match (from, event) {
            (_, Event::Notify(_)) => return None,
            (None, Event::Online(up)) => {
                self.online = Some(*up);
                if !up {
                    self.here = None;
                }
            }
            (None, Event::Outdated) => self.outdated = true,
            (None, Event::Count(n)) => self.here = Some(*n),
            (None, Event::Gone) => {}
            (Some(host), Event::Gone) => {
                self.there.remove(host.as_str());
            }
            (Some(host), event) => {
                let there = self.there.entry(host.as_str().to_owned()).or_default();
                match event {
                    Event::Online(false) => there.count = None,
                    Event::Online(true) => {}
                    Event::Outdated => there.outdated = true,
                    Event::Count(n) => there.count = Some(*n),
                    Event::Notify(_) | Event::Gone => {}
                }
            }
        }
        let after = self.label(name);
        (after != before).then_some(after)
    }

    /// The label as things are.
    pub fn label(&self, name: &str) -> Label {
        if self.outdated {
            return Label {
                tooltip: format!("{name} was upgraded: quit and open this app again"),
                title: Some("!".into()),
            };
        }
        let elsewhere: Vec<String> = self
            .there
            .iter()
            .filter_map(|(host, t)| t.count.filter(|&n| n > 0).map(|n| format!("{n} on {host}")))
            .collect();
        let total =
            self.here.unwrap_or(0) + self.there.values().filter_map(|t| t.count).sum::<usize>();
        let mut tooltip = if total == 0 {
            "nothing needs you".to_owned()
        } else if elsewhere.is_empty() {
            format!("{total} need you")
        } else {
            format!("{total} need you ({})", elsewhere.join(", "))
        };
        if self.online == Some(false) {
            tooltip = if total == 0 {
                "no daemon".to_owned()
            } else {
                format!("no daemon here; {tooltip}")
            };
        }
        let behind: Vec<&str> = self
            .there
            .iter()
            .filter(|(_, t)| t.outdated)
            .map(|(host, _)| host.as_str())
            .collect();
        if !behind.is_empty() {
            tooltip = format!("{tooltip}; another version on {}", behind.join(", "));
        }
        Label {
            tooltip: format!("{name}: {tooltip}"),
            title: (total > 0).then(|| total.to_string()),
        }
    }
}

/// A notification's title, saying which machine it's from when it isn't
/// this one.
pub fn titled(from: Option<&Host>, title: &str) -> String {
    match from {
        Some(host) => format!("{title} on {host}"),
        None => title.to_owned(),
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

    fn label(tooltip: &str, title: Option<&str>) -> Label {
        Label {
            tooltip: format!("V: {tooltip}"),
            title: title.map(str::to_owned),
        }
    }

    #[test]
    fn the_tray_counts_every_machine_and_says_where() {
        let boxed = Host::parse("box").unwrap();
        let mut board = Board::default();
        let box_ = Some(&boxed);
        assert_eq!(board.on(None, &Event::Online(true), "V"), None);
        assert_eq!(
            board.on(None, &Event::Count(1), "V"),
            Some(label("1 need you", Some("1")))
        );
        // Another machine's window opens: nothing until it counts.
        assert_eq!(board.on(box_, &Event::Online(true), "V"), None);
        assert_eq!(
            board.on(box_, &Event::Count(2), "V"),
            Some(label("3 need you (2 on box)", Some("3")))
        );
        // It's out of reach for a while: its count is unknown, not zero forever.
        assert_eq!(
            board.on(box_, &Event::Online(false), "V"),
            Some(label("1 need you", Some("1")))
        );
        board.on(box_, &Event::Count(1), "V");
        // Here goes away; there still counts.
        assert_eq!(
            board.on(None, &Event::Online(false), "V"),
            Some(label("no daemon here; 1 need you (1 on box)", Some("1")))
        );
        // The window closed: its count goes with it.
        assert_eq!(
            board.on(box_, &Event::Gone, "V"),
            Some(label("no daemon", None))
        );
        // Notifications are the caller's.
        let note = ServerMsg::Notice {
            text: "x".into(),
            sound: None,
        };
        for event in Tray::default().on(&note, false, 0) {
            assert_eq!(board.on(box_, &event, "V"), None);
        }
    }

    #[test]
    fn another_version_there_is_said_without_taking_the_tray_over() {
        let boxed = Host::parse("box").unwrap();
        let mut board = Board::default();
        board.on(None, &Event::Count(0), "V");
        assert_eq!(
            board.on(Some(&boxed), &Event::Outdated, "V"),
            Some(label("nothing needs you; another version on box", None))
        );
        // This computer's is the one that asks for a restart.
        assert_eq!(
            board.on(None, &Event::Outdated, "V"),
            Some(Label {
                tooltip: "V was upgraded: quit and open this app again".into(),
                title: Some("!".into()),
            })
        );
    }

    #[test]
    fn a_notification_from_elsewhere_says_where() {
        let boxed = Host::parse("me@box").unwrap();
        assert_eq!(titled(None, "proj needs you"), "proj needs you");
        assert_eq!(
            titled(Some(&boxed), "proj needs you"),
            "proj needs you on me@box"
        );
    }
}

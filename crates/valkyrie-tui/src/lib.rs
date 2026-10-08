//! Terminal client. Talks to the daemon only through `valkyrie-proto` (ADR-0004).
//!
//! Input is read as raw stdin bytes and forwarded untouched while attached, so the
//! program sees exactly what the user's terminal sends. For that to be correct the
//! outer terminal mirrors the program's input modes (app cursor, bracketed paste…).

mod mouse;
mod ping;
mod theme;

use anyhow::Result;
use mouse::{Input, Mouse, MouseKind, Selection};
use ping::{Kind, Ping, Pinger};
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{self, Modifier, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{
    Block, BorderType, Cell, HighlightSpacing, Padding, Paragraph, Row as TableRow, Table,
    TableState, Wrap,
};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use theme::{Theme, state_icon};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use valkyrie_proto::client::{Client, Pushes};
use valkyrie_proto::{
    AgentState, AgentStatus, Color, CursorShape, Modes, QueueItem, Row, ScreenUpdate, ScrollAnchor,
    ServerMsg, SessionId, SessionInfo, Size, SpawnSpec, Style,
};

/// Ctrl-] — detach from the attached session.
pub const DETACH_KEY: u8 = 0x1d;

/// How long the TUI keeps trying to reach the daemon after the connection drops. An
/// upgrade handoff (ADR-0006) takes milliseconds; a daemon restart is not coming back.
const RECONNECT_FOR: Duration = Duration::from_secs(10);

pub async fn run(
    client: Client,
    pushes: Pushes,
    attach_to: Option<SessionId>,
    socket: PathBuf,
) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    // ratatui's own hook (installed by init) restores raw mode and the screen; chain
    // ours in front so mirrored input modes don't outlive a panic either.
    let ratatui_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = reset_terminal_modes();
        ratatui_hook(info);
    }));
    let result = App::new(client, socket)
        .run(&mut terminal, pushes, attach_to)
        .await;
    let _ = reset_terminal_modes();
    ratatui::restore();
    result
}

struct App {
    client: Client,
    socket: PathBuf,
    /// The daemon's identity (`Hello::boot`): an upgrade keeps it, a restart doesn't.
    boot: Option<u64>,
    /// The ranked attention queue, as last pushed by the daemon.
    queue: Vec<QueueItem>,
    sessions: Vec<SessionInfo>,
    /// One selection over the queue rows followed by the session rows.
    selected: usize,
    view: Option<Attached>,
    status: String,
    quit: bool,
    theme: &'static Theme,
    /// The selected session's screen text, for the preview pane.
    preview: Option<(SessionId, String)>,
    pinger: Pinger,
    /// Ping sounds on (`m` toggles).
    sound: bool,
    /// The last ping, shown in the bar for `TOAST_FOR`.
    toast: Option<(Ping, Instant)>,
    /// The start of a mouse report the last read cut off.
    partial: Vec<u8>,
}

struct Attached {
    id: SessionId,
    name: String,
    rows: Vec<Row>,
    cursor: Option<(u16, u16)>,
    shape: CursorShape,
    modes: Modes,
    title: Option<String>,
    exited: Option<Option<i32>>,
    status: Option<AgentStatus>,
    /// Screen width, for copying wrapped rows.
    cols: u16,
    /// Scrolled back into history; `None` shows the live screen.
    scroll: Option<Scrolled>,
    /// Text dragged over with the mouse; stays highlighted until the next key or click.
    selection: Option<Selection>,
    selecting: bool,
    /// A short message for the bar (`copied 42 chars`), and when it was set.
    notice: Option<(String, Instant)>,
}

/// A page of scrollback (`Reply::Scrollback`).
struct Scrolled {
    from_top: u32,
    history: u32,
    rows: Vec<Row>,
}

/// What most terminals send for Shift-PageUp.
const SHIFT_PAGE_UP: &[u8] = b"\x1b[5;2~";
/// Deletes every image (and its data) from the outer terminal, asking for no reply.
const CLEAR_IMAGES: &str = "\x1b_Ga=d,d=A,q=2\x1b\\";
/// Lines per wheel notch.
const WHEEL_LINES: i64 = 3;
/// How long a notice stays in the attached bar.
const NOTICE_FOR: Duration = Duration::from_secs(3);

impl Attached {
    fn new(id: SessionId, name: String, status: Option<AgentStatus>) -> Self {
        Self {
            id,
            name,
            rows: Vec::new(),
            cursor: None,
            shape: CursorShape::Block,
            modes: Modes::default(),
            title: None,
            exited: None,
            status,
            cols: 0,
            scroll: None,
            selection: None,
            selecting: false,
            notice: None,
        }
    }

    /// What is on screen: the scrollback page, or the live screen.
    fn shown(&self) -> &[Row] {
        match &self.scroll {
            Some(page) => &page.rows,
            None => &self.rows,
        }
    }

    fn notice(&mut self, text: String) {
        self.notice = Some((text, Instant::now()));
    }

    /// The program gets the mouse when it asked for it; otherwise Valkyrie uses it.
    fn ours(&self) -> bool {
        !self.modes.wants_mouse()
    }
}

impl App {
    fn new(client: Client, socket: PathBuf) -> Self {
        Self {
            client,
            socket,
            boot: None,
            queue: Vec::new(),
            sessions: Vec::new(),
            selected: 0,
            view: None,
            status: String::new(),
            quit: false,
            theme: Theme::load(),
            preview: None,
            pinger: Pinger::default(),
            sound: ping::load_enabled(),
            toast: None,
            partial: Vec::new(),
        }
    }

    async fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
        mut pushes: Pushes,
        attach_to: Option<SessionId>,
    ) -> Result<()> {
        let mut stdin = stdin_bytes();
        let mut winch = signal(SignalKind::window_change())?;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        // Only redraws, for the working spinner; runs while one is on screen.
        let mut anim = tokio::time::interval(SPIN_EVERY);
        anim.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        self.boot = self.client.hello_info().await.ok().map(|info| info.boot);
        self.report_cell(None);
        if let Err(e) = self.client.watch_queue().await {
            self.status = format!("queue unavailable: {e:#}");
        }
        self.refresh().await;
        if let Some(id) = attach_to {
            self.attach(id).await;
        }

        while !self.quit {
            self.draw(terminal)?;
            tokio::select! {
                bytes = stdin.recv() => match bytes {
                    Some(bytes) => self.on_input(bytes).await,
                    None => break,
                },
                msg = pushes.recv() => {
                    let Some(msg) = msg else {
                        pushes = self.reconnect(terminal).await?;
                        continue;
                    };
                    let mut queue_changed = self.on_push(msg)?;
                    // Coalesce bursts into one redraw.
                    while let Ok(msg) = pushes.try_recv() {
                        queue_changed |= self.on_push(msg)?;
                    }
                    if queue_changed {
                        self.refresh().await;
                    }
                }
                _ = winch.recv() => {
                    // A font size change resizes the window too. First, so the PTY
                    // resize below carries the new pixel size in one SIGWINCH.
                    self.report_cell(self.view.as_ref().map(|v| v.id));
                    if let Some(view) = &self.view
                        && let Ok(size) = session_size()
                    {
                        let _ = self.client.resize(view.id, size);
                    }
                    terminal.autoresize()?;
                }
                _ = tick.tick() => {
                    self.refresh().await;
                    self.update_preview().await;
                }
                _ = anim.tick(), if self.animating() => {}
                _ = sleep_until(self.pinger.next_due()), if self.pinger.next_due().is_some() => {
                    self.fire_pings();
                }
            }
        }
        Ok(())
    }

    /// The daemon connection dropped, usually for an upgrade handoff: reconnect and
    /// put the same view back.
    async fn reconnect(&mut self, terminal: &mut DefaultTerminal) -> Result<Pushes> {
        self.status = "daemon connection lost; reconnecting…".into();
        self.draw(terminal)?;
        let deadline = std::time::Instant::now() + RECONNECT_FOR;
        while std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let Ok((client, pushes)) = Client::connect(&self.socket).await else {
                continue;
            };
            let info = match client.hello_info().await {
                Ok(info) if info.protocol == valkyrie_proto::PROTOCOL => info,
                Ok(info) => anyhow::bail!(
                    "the daemon now speaks protocol {}, this valk {}; run the new valk",
                    info.protocol,
                    valkyrie_proto::PROTOCOL
                ),
                Err(_) => continue,
            };
            self.client = client;
            // Session ids start over in a restarted daemon: re-attaching by id could
            // land in an unrelated program.
            let same = self.boot == Some(info.boot);
            if !same {
                self.pinger.reset();
            }
            self.boot = Some(info.boot);
            self.status = if same {
                "reconnected to the daemon".into()
            } else {
                self.view = None;
                let _ = reset_terminal_modes();
                "the daemon restarted; restorable sessions came back with new ids".into()
            };
            if let Err(e) = self.client.watch_queue().await {
                self.status = format!("queue unavailable: {e:#}");
            }
            self.report_cell(None);
            self.refresh().await;
            if let Some(id) = self.view.as_ref().map(|v| v.id) {
                self.attach(id).await;
            }
            return Ok(pushes);
        }
        anyhow::bail!("lost the daemon connection")
    }

    /// Something on the home screen is spinning.
    fn animating(&self) -> bool {
        self.view.is_none()
            && self
                .sessions
                .iter()
                .any(|s| s.exited.is_none() && s.status.state == AgentState::Working)
    }

    /// Fetches the selected session's screen for the preview pane.
    async fn update_preview(&mut self) {
        if self.view.is_some() {
            return;
        }
        let (Some(id), _) = self.selection() else {
            self.preview = None;
            return;
        };
        self.preview = self.client.dump(id).await.ok().map(|text| (id, text));
    }

    async fn refresh(&mut self) {
        match self.client.list().await {
            Ok(sessions) => {
                let anchor = self.anchor();
                self.sessions = sessions;
                self.restore(anchor);
                if let Some(id) = self.view.as_ref().map(|v| v.id) {
                    let status = self.sessions_status(id);
                    if let Some(view) = &mut self.view {
                        view.status = status;
                    }
                }
            }
            Err(e) => self.status = format!("list failed: {e:#}"),
        }
    }

    async fn attach(&mut self, id: SessionId) {
        // A TUI running inside a session would feed its own output back to itself.
        if std::env::var("VALK_SESSION").is_ok_and(|own| own == id.to_string()) {
            self.status = format!("session {id} is this TUI");
            return;
        }
        let name = self
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| id.to_string());
        let status = self.sessions_status(id);
        let size = match session_size() {
            Ok(size) => size,
            Err(e) => {
                self.status = format!("{e:#}");
                return;
            }
        };
        // Set the view before the request so the snapshot pushed right after the reply lands in it.
        self.view = Some(Attached::new(id, name, status));
        // The snapshot only rewrites modes that differ from the defaults; the mouse
        // capture has to start now.
        let _ = write_mouse(Modes::default(), true);
        self.report_cell(Some(id));
        if let Err(e) = self.client.attach(id, size).await {
            self.view = None;
            let _ = reset_terminal_modes();
            self.status = format!("attach {id} failed: {e:#}");
        }
    }

    async fn detach(&mut self) {
        self.view = None;
        let _ = reset_terminal_modes();
        let _ = self.client.detach().await;
        self.refresh().await;
    }

    async fn on_input(&mut self, mut bytes: Vec<u8>) {
        if !self.partial.is_empty() {
            let mut joined = std::mem::take(&mut self.partial);
            joined.extend(bytes);
            bytes = joined;
        }
        if let Some(view) = &self.view {
            // A program that asked for the mouse gets the reports untouched.
            let inputs = if view.ours() {
                // A report cut off by the end of this read finishes in the next one.
                if let Some(cut) = mouse::unfinished(&bytes) {
                    self.partial = bytes.split_off(cut);
                }
                mouse::split(&bytes)
            } else {
                vec![Input::Bytes(bytes)]
            };
            for input in inputs {
                match (&self.view, input) {
                    // Detached mid-read: the rest belongs to the home screen.
                    (None, Input::Bytes(rest)) => {
                        for key in home_keys(&rest) {
                            Box::pin(self.on_home_key(key)).await;
                        }
                    }
                    (None, Input::Mouse(_)) => {}
                    (Some(_), Input::Mouse(m)) => self.on_mouse(m).await,
                    // A finished program: any key returns home, unless you are
                    // reading its scrollback.
                    (Some(view), Input::Bytes(_))
                        if view.exited.is_some() && view.scroll.is_none() =>
                    {
                        self.detach().await;
                    }
                    (Some(_), Input::Bytes(bytes)) => self.on_keys(bytes).await,
                }
            }
            return;
        }
        for key in home_keys(&bytes) {
            self.on_home_key(key).await;
        }
    }

    /// Keys while attached: scrollback keys when scrolled back, else the program's.
    async fn on_keys(&mut self, bytes: Vec<u8>) {
        let Some(view) = &mut self.view else { return };
        view.selection = None;
        let page = view.rows.len().max(2) as i64 - 1;
        let mut rest: &[u8] = &bytes;
        if view.scroll.is_none() && !view.modes.alt_screen && rest.starts_with(SHIFT_PAGE_UP) {
            // Shift-PageUp scrolls back, as in most terminals. A full-screen program
            // has no scrollback and gets the key.
            rest = &rest[SHIFT_PAGE_UP.len()..];
            self.scroll_by(-page).await;
        }
        if self.view.as_ref().is_some_and(|v| v.scroll.is_some()) {
            rest = self.scroll_keys(rest, page).await;
        }
        if rest.is_empty() {
            return;
        }
        let Some(view) = &self.view else { return };
        let id = view.id;
        match rest.iter().position(|&b| b == DETACH_KEY) {
            Some(i) => {
                if i > 0 {
                    let _ = self.client.input(id, rest[..i].to_vec());
                }
                self.detach().await;
                // Keys typed right after ^] in the same read belong to the home screen.
                for key in home_keys(&rest[i + 1..]) {
                    Box::pin(self.on_home_key(key)).await;
                }
            }
            None => {
                let _ = self.client.input(id, rest.to_vec());
            }
        }
    }

    /// Takes the scrollback keys from the front of `keys` and returns the rest, which
    /// goes to the program: the first other key ends scroll mode. Scroll keys after
    /// the page reached the live screen are dropped, not typed (a held `j`).
    async fn scroll_keys<'a>(&mut self, mut keys: &'a [u8], page: i64) -> &'a [u8] {
        let mut live = false;
        while !keys.is_empty() {
            let key = next_key(keys);
            // A lone Esc is a key; Esc with more after it is Alt plus that key.
            let whole = key.len() == keys.len();
            let action = match key {
                b"\x1b[A" | b"\x1bOA" | b"k" => Some(Some(-1)),
                b"\x1b[B" | b"\x1bOB" | b"j" => Some(Some(1)),
                b"\x1b[5~" | b"\x1b[5;2~" | b"b" | b"\x02" | b"\x15" => Some(Some(-page)),
                b"\x1b[6~" | b"\x1b[6;2~" | b" " | b"\x06" | b"\x04" => Some(Some(page)),
                b"g" | b"\x1b[H" | b"\x1b[1~" | b"\x1bOH" => Some(Some(i64::MIN / 2)),
                b"q" | b"G" | b"\x1b[F" | b"\x1b[4~" | b"\x1bOF" => Some(None),
                b"\x1b" if whole => Some(None),
                _ => None,
            };
            let Some(delta) = action else { break };
            keys = &keys[key.len()..];
            if live {
                continue;
            }
            match delta {
                Some(delta) => self.scroll_by(delta).await,
                None => {
                    if let Some(view) = &mut self.view {
                        view.scroll = None;
                    }
                }
            }
            live = self.view.as_ref().is_none_or(|v| v.scroll.is_none());
        }
        if let Some(view) = &mut self.view
            && !keys.is_empty()
        {
            view.scroll = None;
        }
        keys
    }

    async fn on_mouse(&mut self, m: Mouse) {
        let Some(view) = &mut self.view else { return };
        let bottom = view.rows.len().saturating_sub(1) as u16;
        let at = (m.x, m.y.min(bottom));
        match m.kind {
            MouseKind::WheelUp | MouseKind::WheelDown if view.modes.alt_screen => {
                // Full-screen programs have no scrollback; like other terminals, the
                // wheel becomes arrow keys (mode 1007).
                if view.modes.alt_scroll {
                    let key: &[u8] = match (m.kind == MouseKind::WheelUp, view.modes.app_cursor) {
                        (true, true) => b"\x1bOA",
                        (true, false) => b"\x1b[A",
                        (false, true) => b"\x1bOB",
                        (false, false) => b"\x1b[B",
                    };
                    let _ = self.client.input(view.id, key.repeat(WHEEL_LINES as usize));
                }
            }
            MouseKind::WheelUp => self.scroll_by(-WHEEL_LINES).await,
            MouseKind::WheelDown => self.scroll_by(WHEEL_LINES).await,
            MouseKind::Press => {
                view.selection = Some(Selection {
                    anchor: at,
                    head: at,
                });
                view.selecting = true;
            }
            MouseKind::Drag => {
                if view.selecting
                    && let Some(sel) = &mut view.selection
                {
                    sel.head = at;
                }
            }
            MouseKind::Release => {
                if !std::mem::replace(&mut view.selecting, false) {
                    return;
                }
                let Some(sel) = view.selection.filter(|s| !s.is_empty()) else {
                    view.selection = None;
                    return;
                };
                let text = sel.text(view.shown(), view.cols);
                if text.is_empty() {
                    return;
                }
                let n = text.chars().count();
                match mouse::copy_to_clipboard(&text) {
                    Ok(()) => view.notice(format!("copied {n} chars")),
                    Err(e) => view.notice(format!("copy failed: {e}")),
                }
            }
            MouseKind::Other => {}
        }
    }

    /// Scrolls `delta` lines (negative is up, into history). Scrolling down past the
    /// live screen returns to it.
    async fn scroll_by(&mut self, delta: i64) {
        let Some(view) = &mut self.view else { return };
        let anchor = match &view.scroll {
            None if delta >= 0 => return,
            None => ScrollAnchor::Up((-delta).min(u32::MAX as i64) as u32),
            Some(page) => {
                let top = page.from_top as i64 + delta;
                if top >= page.history as i64 {
                    view.scroll = None;
                    view.selection = None;
                    return;
                }
                ScrollAnchor::FromTop(top.max(0) as u32)
            }
        };
        // The text under a selection moves.
        view.selection = None;
        let id = view.id;
        self.fetch_scroll(id, anchor).await;
    }

    async fn fetch_scroll(&mut self, id: SessionId, anchor: ScrollAnchor) {
        let result = self.client.scrollback(id, anchor).await;
        let Some(view) = self.view.as_mut().filter(|v| v.id == id) else {
            return;
        };
        match result {
            Ok((from_top, history, _)) if from_top >= history => {
                if view.scroll.is_none() {
                    view.notice("no scrollback".into());
                }
                view.scroll = None;
            }
            Ok((from_top, history, rows)) => {
                if view.scroll.is_none() {
                    view.selection = None;
                }
                view.scroll = Some(Scrolled {
                    from_top,
                    history,
                    rows,
                });
            }
            Err(e) => view.notice(format!("scrollback failed: {e:#}")),
        }
    }

    fn rows(&self) -> usize {
        self.queue.len() + self.sessions.len()
    }

    /// Which section the cursor is in and on which session, so it can follow that
    /// session when rows above it come or go (else `x` would kill the wrong one).
    fn anchor(&self) -> (bool, Option<SessionId>) {
        (self.selected < self.queue.len(), self.selection().0)
    }

    fn restore(&mut self, (in_queue, id): (bool, Option<SessionId>)) {
        let found = if in_queue {
            self.queue.iter().position(|q| Some(q.session) == id)
        } else {
            self.sessions
                .iter()
                .position(|s| Some(s.id) == id)
                .map(|i| self.queue.len() + i)
        };
        if let Some(i) = found {
            self.selected = i;
        }
        self.clamp_selection();
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.rows().saturating_sub(1));
    }

    /// The session under the cursor, and the queue item if the cursor is in the queue.
    fn selection(&self) -> (Option<SessionId>, Option<&QueueItem>) {
        match self.queue.get(self.selected) {
            Some(item) => (Some(item.session), Some(item)),
            None => (
                self.sessions
                    .get(self.selected - self.queue.len())
                    .map(|s| s.id),
                None,
            ),
        }
    }

    async fn on_home_key(&mut self, key: HomeKey) {
        let (selected, item) = self.selection();
        let item = item.map(|i| (i.session, i.status.seq));
        match key {
            HomeKey::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.update_preview().await;
            }
            HomeKey::Down => {
                self.selected += 1;
                self.clamp_selection();
                self.update_preview().await;
            }
            HomeKey::Sound => {
                self.sound = !self.sound;
                ping::save_enabled(self.sound);
                self.status = format!("sound: {}", if self.sound { "on" } else { "off" });
                if std::env::var_os("VALK_SOUND").is_some() {
                    self.status += " (VALK_SOUND decides at the next start)";
                }
            }
            HomeKey::Theme => {
                self.theme = self.theme.next();
                self.theme.save();
                self.status = format!("theme: {}", self.theme.name);
            }
            HomeKey::Top => {
                if let Some(top) = self.queue.first() {
                    let id = top.session;
                    self.attach(id).await;
                }
            }
            HomeKey::Seen => {
                if let Some((id, seq)) = item {
                    self.mark_seen(&[(id, seq)]).await;
                }
            }
            HomeKey::SeenAll => {
                let all: Vec<_> = self
                    .queue
                    .iter()
                    .map(|i| (i.session, i.status.seq))
                    .collect();
                self.mark_seen(&all).await;
            }
            HomeKey::Quit => self.quit = true,
            HomeKey::Refresh => self.refresh().await,
            HomeKey::Enter => {
                if let Some(id) = selected {
                    self.attach(id).await;
                }
            }
            HomeKey::Kill => {
                if let Some(id) = selected {
                    if let Err(e) = self.client.kill(id).await {
                        self.status = format!("kill failed: {e:#}");
                    }
                    self.refresh().await;
                }
            }
            HomeKey::NewShell => {
                let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
                let size = session_size().unwrap_or(Size { cols: 80, rows: 24 });
                let spec = SpawnSpec {
                    command: vec![shell],
                    cwd: None,
                    name: None,
                    size,
                    env: valkyrie_proto::login_env(),
                };
                match self.client.spawn(spec).await {
                    Ok(info) => {
                        self.refresh().await;
                        self.attach(info.id).await;
                    }
                    Err(e) => self.status = format!("spawn failed: {e:#}"),
                }
            }
        }
    }

    async fn mark_seen(&mut self, items: &[(SessionId, u64)]) {
        for &(id, seq) in items {
            if let Err(e) = self.client.mark_seen(id, seq).await {
                self.status = format!("mark seen failed: {e:#}");
            }
        }
    }

    /// Returns whether the queue changed.
    fn on_push(&mut self, msg: ServerMsg) -> Result<bool> {
        if let ServerMsg::Queue { items } = msg {
            let viewing = self.view.as_ref().map(|v| v.id);
            self.pinger.on_queue(&items, viewing, Instant::now());
            let anchor = self.anchor();
            self.queue = items;
            self.restore(anchor);
            return Ok(true);
        }
        let Some(view) = &mut self.view else {
            return Ok(false);
        };
        match msg {
            ServerMsg::Screen { session, update } if session == view.id => {
                let before = (view.modes, view.shape);
                view.apply(update);
                if before != (view.modes, view.shape) {
                    write_modes(view.modes, Some(view.shape))?;
                }
                // Rewriting the mouse modes mid-drag can lose the drag, so only on change.
                if mouse::modes(before.0) != mouse::modes(view.modes) {
                    write_mouse(view.modes, true)?;
                }
            }
            ServerMsg::Graphics {
                session,
                x,
                y,
                data,
            } if session == view.id => {
                // At the cell the program's cursor was on, then the cursor back where
                // ratatui left it. Unicode placeholders (yazi, `kitten icat`) are
                // ordinary cells on the screen; this only delivers the image data.
                let mut out = std::io::stdout();
                write!(out, "\x1b7\x1b[{};{}H{data}\x1b8", y + 1, x + 1)?;
                out.flush()?;
            }
            ServerMsg::Clipboard { session, text } if session == view.id => {
                let n = text.chars().count();
                match mouse::copy_to_clipboard(&text) {
                    Ok(()) => view.notice(format!("{} copied {n} chars", view.name)),
                    Err(e) => view.notice(format!("copy failed: {e}")),
                }
            }
            ServerMsg::Exited { session, code } if session == view.id => {
                view.exited = Some(code);
                // A program that died with the mouse on: take it back, so a wheel or
                // a motion report scrolls its last screen instead of leaving it.
                if view.modes.wants_mouse() {
                    view.modes.mouse_click = false;
                    view.modes.mouse_drag = false;
                    view.modes.mouse_motion = false;
                    write_mouse(view.modes, true)?;
                }
            }
            _ => {}
        }
        Ok(false)
    }

    /// The attached session's state for its status bar: from the queue when it is
    /// queued (fresh), else from the last session list.
    fn sessions_status(&self, id: SessionId) -> Option<AgentStatus> {
        self.queue
            .iter()
            .find(|q| q.session == id)
            .map(|q| q.status.clone())
            .or_else(|| {
                self.sessions
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| s.status.clone())
            })
    }

    fn draw(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        terminal.draw(|frame| {
            let [body, bar] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
            match &self.view {
                Some(view) => {
                    render_rows(view.shown(), body, frame.buffer_mut());
                    if let Some(sel) = &view.selection {
                        highlight(sel, body, frame.buffer_mut());
                    }
                    if let Some((x, y)) = view.cursor.filter(|_| view.scroll.is_none())
                        && x < body.width
                        && y < body.height
                    {
                        frame.set_cursor_position(Position::new(body.x + x, body.y + y));
                    }
                    let mut line = attached_bar(view, self.theme);
                    if let Some(toast) = self.toast() {
                        line.spans.extend(toast_spans(toast, self.theme));
                    }
                    frame.render_widget(
                        Paragraph::new(line).style(style::Style::new().bg(self.theme.panel)),
                        bar,
                    );
                }
                None => self.draw_home(frame, body, bar),
            }
        })?;
        Ok(())
    }
}

/// How long a ping's toast stays in the bar.
const TOAST_FOR: Duration = Duration::from_secs(6);

impl App {
    /// Tells the daemon this terminal's cell size in pixels, for image programs (as
    /// the default for new sessions, and for `session`). Terminals that do not report
    /// pixels (some over SSH) leave the daemon's guess.
    fn report_cell(&self, session: Option<SessionId>) {
        if let Some((width, height)) = cell_pixels() {
            let _ = self.client.cell_pixels(session, width, height);
        }
    }

    fn fire_pings(&mut self) {
        let viewing = self.view.as_ref().map(|v| v.id);
        let Some(ping) = self.pinger.due(viewing, Instant::now()) else {
            return;
        };
        if ping.sound && self.sound {
            ping::play(ping.kind);
        }
        // A fresh request stays up over a later finish.
        if self.toast().is_some_and(|t| t.kind == Kind::Request) && ping.kind == Kind::Done {
            return;
        }
        self.toast = Some((ping, Instant::now()));
    }

    fn toast(&self) -> Option<&Ping> {
        self.toast
            .as_ref()
            .filter(|(_, at)| at.elapsed() < TOAST_FOR)
            .map(|(ping, _)| ping)
    }
}

async fn sleep_until(at: Option<Instant>) {
    if let Some(at) = at {
        tokio::time::sleep_until(at.into()).await;
    }
}

/// ` ● api needs input ` in the ping's color.
fn toast_spans(ping: &Ping, t: &Theme) -> Vec<ratatui::text::Span<'static>> {
    let (icon, color) = match ping.kind {
        Kind::Request => ("●", t.needs),
        Kind::Done => ("✓", t.done),
    };
    vec![
        format!(" {icon} {} ", ping.text).fg(t.bg).bg(color).bold(),
        " ".into(),
    ]
}

/// How often the working spinner advances.
const SPIN_EVERY: Duration = Duration::from_millis(100);
/// Wide enough for the lists plus a preview of the selected session.
const PREVIEW_MIN_WIDTH: u16 = 130;

impl App {
    fn draw_home(&self, frame: &mut ratatui::Frame, body: Rect, bar: Rect) {
        let t = self.theme;
        let now = now_ms();
        let spin = (now / SPIN_EVERY.as_millis() as u64) as usize;
        frame.render_widget(
            Block::new().style(style::Style::new().bg(t.bg)),
            frame.area(),
        );

        let [header, main] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(body);
        frame.render_widget(
            Paragraph::new(header_line(&self.queue, &self.sessions, t)),
            header,
        );

        let (lists, preview) = if main.width >= PREVIEW_MIN_WIDTH {
            let [lists, preview] =
                Layout::horizontal([Constraint::Percentage(62), Constraint::Fill(1)]).areas(main);
            (lists, Some(preview))
        } else {
            (main, None)
        };
        let queue_height = (self.queue.len().max(2) as u16 + 2)
            .min(lists.height / 2)
            .max(4);
        let [queue_area, sessions_area] =
            Layout::vertical([Constraint::Length(queue_height), Constraint::Fill(1)]).areas(lists);

        let in_queue = self.selected < self.queue.len();
        let queue_title = Line::from(vec![
            " ● ".fg(t.needs).bold(),
            "needs you ".fg(t.fg).bold(),
            format!("{} ", self.queue.len()).fg(t.muted),
        ]);
        let queue_block = panel(queue_title, t, in_queue || self.sessions.is_empty());
        if self.queue.is_empty() {
            let inner = queue_block.inner(queue_area);
            frame.render_widget(queue_block, queue_area);
            let calm = Line::from(vec![
                "✓ ".fg(t.done).bold(),
                "all clear".fg(t.fg).bold(),
                "  ·  nothing needs you".fg(t.muted),
            ])
            .centered();
            let [_, mid, _] = Layout::vertical([
                Constraint::Fill(1),
                Constraint::Length(1),
                Constraint::Fill(1),
            ])
            .areas(inner);
            frame.render_widget(Paragraph::new(calm), mid);
        } else {
            let rows = self.queue.iter().map(|q| queue_row(q, now, spin, t));
            let mut state = TableState::default().with_selected(in_queue.then_some(self.selected));
            frame.render_stateful_widget(table(rows, t).block(queue_block), queue_area, &mut state);
        }

        let sessions_title = Line::from(vec![
            " ◆ ".fg(t.accent).bold(),
            "sessions ".fg(t.fg).bold(),
            format!("{} ", self.sessions.len()).fg(t.muted),
        ]);
        let rows = self.sessions.iter().map(|s| session_row(s, now, spin, t));
        let mut state = TableState::default()
            .with_selected((!in_queue).then(|| self.selected - self.queue.len()));
        frame.render_stateful_widget(
            table(rows, t).block(panel(sessions_title, t, !in_queue)),
            sessions_area,
            &mut state,
        );

        if let Some(area) = preview {
            self.draw_preview(frame, area);
        }

        let mut footer = footer_line(&self.status, self.sound, t);
        if let Some(toast) = self.toast() {
            footer.spans.splice(0..0, toast_spans(toast, t));
        }
        frame.render_widget(
            Paragraph::new(footer).style(style::Style::new().bg(t.panel)),
            bar,
        );
    }

    /// The selected session: its full summary, then the bottom of its screen.
    fn draw_preview(&self, frame: &mut ratatui::Frame, area: Rect) {
        let t = self.theme;
        let (selected, _) = self.selection();
        let session = selected.and_then(|id| self.sessions.iter().find(|s| s.id == id));
        let Some(session) = session else {
            frame.render_widget(panel(Line::from(" preview ".fg(t.muted)), t, false), area);
            return;
        };
        let title = Line::from(vec![
            " ▸ ".fg(t.accent).bold(),
            session.name.clone().fg(t.fg).bold(),
            format!(" #{} ", session.id).fg(t.muted),
        ]);
        let block = panel(title, t, false);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let status = &session.status;
        let color = t.state(status.state);
        let mut head = vec![Line::from(vec![
            format!("{} {}", state_icon(status.state, 0), status.state.label())
                .fg(color)
                .bold(),
            format!("  {}  ·  {}", status.agent, short_path(&session.cwd)).fg(t.muted),
        ])];
        if let Some(summary) = &status.summary {
            head.push(Line::from(summary.clone().fg(t.fg)));
        }
        head.push(Line::from("─".repeat(inner.width as usize).fg(t.border)));
        let head_height = head.len() as u16 + u16::from(status.summary.is_some()); // room to wrap
        let [top, screen] =
            Layout::vertical([Constraint::Length(head_height), Constraint::Fill(1)]).areas(inner);
        frame.render_widget(Paragraph::new(head).wrap(Wrap { trim: true }), top);

        let text = match &self.preview {
            Some((id, text)) if *id == session.id => text.as_str(),
            _ => "",
        };
        let lines: Vec<&str> = text.lines().collect();
        let end = lines
            .iter()
            .rposition(|l| !l.trim().is_empty())
            .map_or(0, |i| i + 1);
        let start = end.saturating_sub(screen.height as usize);
        let body: Vec<Line> = lines[start..end]
            .iter()
            .map(|l| Line::from(l.to_string().fg(t.muted)))
            .collect();
        frame.render_widget(Paragraph::new(body), screen);
    }
}

/// A rounded panel; the focused one gets the accent border.
fn panel(title: Line<'static>, t: &Theme, focused: bool) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style::Style::new().fg(if focused { t.accent } else { t.border }))
        .style(style::Style::new().bg(t.panel).fg(t.fg))
        .title(title)
        .padding(Padding::horizontal(1))
}

fn table<'a>(rows: impl IntoIterator<Item = TableRow<'a>>, t: &Theme) -> Table<'a> {
    Table::new(
        rows,
        [
            Constraint::Length(15),
            Constraint::Length(16),
            Constraint::Length(5),
            Constraint::Fill(3),
            Constraint::Fill(1),
        ],
    )
    .column_spacing(1)
    .row_highlight_style(style::Style::new().bg(t.selection))
    .highlight_symbol(Line::from("▌".fg(t.accent)))
    .highlight_spacing(HighlightSpacing::Always)
}

fn badge(state: AgentState, label: String, spin: usize, t: &Theme) -> Cell<'static> {
    let color = t.state(state);
    Cell::from(Line::from(vec![
        format!("{} ", state_icon(state, spin)).fg(color),
        label.fg(color).bold(),
    ]))
}

fn name_cell(id: SessionId, name: &str, t: &Theme) -> Cell<'static> {
    Cell::from(Line::from(vec![
        name.to_string().fg(t.fg).bold(),
        format!(" #{id}").fg(t.muted),
    ]))
}

fn queue_row(q: &QueueItem, now: u64, spin: usize, t: &Theme) -> TableRow<'static> {
    let s = &q.status;
    TableRow::new([
        badge(s.state, s.state.label().into(), spin, t),
        name_cell(q.session, &q.name, t),
        Cell::from(age(s.since_ms, now).fg(t.muted)),
        Cell::from(s.summary.clone().unwrap_or_default().fg(t.fg)),
        Cell::from(short_path(&q.cwd).fg(t.muted)),
    ])
}

fn session_row(s: &SessionInfo, now: u64, spin: usize, t: &Theme) -> TableRow<'static> {
    let (state, label) = match s.exited {
        None => (s.status.state, s.status.state.label().to_string()),
        Some(Some(code)) => (AgentState::Exited, format!("exited {code}")),
        Some(None) => (AgentState::Exited, "exited".into()),
    };
    let detail = match &s.status.summary {
        Some(summary) => summary.clone().fg(t.fg),
        None => s
            .title
            .clone()
            .unwrap_or_else(|| s.command.join(" "))
            .fg(t.muted),
    };
    let mut detail = vec![detail];
    if s.status.agent != "generic" && !s.status.hooked {
        detail.push("  no hooks yet".fg(t.interrupted).italic());
    }
    if s.clients > 0 {
        detail.insert(0, "◉ ".fg(t.accent2));
    }
    TableRow::new([
        badge(state, label, spin, t),
        name_cell(s.id, &s.name, t),
        Cell::from(age(s.status.since_ms, now).fg(t.muted)),
        Cell::from(Line::from(detail)),
        Cell::from(short_path(&s.cwd).fg(t.muted)),
    ])
}

/// ` ◆ VALKYRIE  agent control` on the left, colored counts on the right.
fn header_line(queue: &[QueueItem], sessions: &[SessionInfo], t: &Theme) -> Line<'static> {
    let mut spans = vec![
        " ◆ VALKYRIE ".fg(t.bg).bg(t.accent).bold(),
        " agent control ".fg(t.muted),
    ];
    for (state, n) in counts(queue, sessions) {
        let color = t.state(state);
        spans.push(" ".into());
        spans.push(format!(" {n} {} ", state.label()).fg(t.bg).bg(color).bold());
    }
    if spans.len() == 2 {
        spans.push(" ✓ all quiet ".fg(t.done));
    }
    Line::from(spans)
}

/// The status message, then key hints as chips.
fn footer_line(status: &str, sound: bool, t: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    if !status.is_empty() {
        spans.push(format!(" {status} ").fg(t.accent2).bold());
        spans.push("│".fg(t.border));
    }
    for (key, what) in [
        ("↩", "attach"),
        ("⇥", "top"),
        ("s/S", "seen"),
        ("n", "new"),
        ("x", "kill"),
        ("t", "theme"),
        ("m", if sound { "sound" } else { "muted" }),
        ("q", "quit"),
    ] {
        spans.push(" ".into());
        spans.push(format!(" {key} ").fg(t.bg).bg(t.accent).bold());
        spans.push(format!(" {what}").fg(t.muted));
    }
    Line::from(spans).style(style::Style::new().bg(t.panel))
}

/// The bar under an attached session.
fn attached_bar(view: &Attached, t: &Theme) -> Line<'static> {
    let mut spans = vec![
        " ◆ VALKYRIE ".fg(t.bg).bg(t.accent).bold(),
        format!(" {} ", view.name).fg(t.fg).bold(),
        format!("#{} ", view.id).fg(t.muted),
    ];
    match (view.exited, &view.status) {
        (Some(code), _) => {
            let code = code.map(|c| format!(" {c}")).unwrap_or_default();
            spans.push(
                format!(" × exited{code} · any key returns ")
                    .fg(t.blocked)
                    .bold(),
            );
        }
        (None, Some(s)) => {
            let color = t.state(s.state);
            spans.push(
                format!(" {} {} ", state_icon(s.state, 0), s.state.label())
                    .fg(color)
                    .bold(),
            );
        }
        (None, None) => {}
    }
    if let Some(page) = &view.scroll {
        let up = page.history - page.from_top;
        spans.push(
            format!(" ↑ {up}/{} lines back ", page.history)
                .fg(t.bg)
                .bg(t.accent2)
                .bold(),
        );
        spans.push(" q live ".fg(t.muted));
    } else if let Some(title) = &view.title {
        spans.push(format!(" {title} ").fg(t.muted));
    }
    if let Some((notice, at)) = &view.notice
        && at.elapsed() < NOTICE_FOR
    {
        spans.push(format!(" {notice} ").fg(t.done).bold());
    }
    spans.push(" ".into());
    spans.push(" ^] ".fg(t.bg).bg(t.accent).bold());
    spans.push(" detach ".fg(t.muted));
    Line::from(spans).style(style::Style::new().bg(t.panel))
}

/// `~/repos/x` for paths under `$HOME`.
fn short_path(path: &std::path::Path) -> String {
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) if path.starts_with(&home) && home.as_os_str().len() > 1 => {
            format!("~/{}", path.strip_prefix(&home).unwrap().display())
                .trim_end_matches('/')
                .to_string()
        }
        _ => path.display().to_string(),
    }
}

/// How many sessions are in each state worth a glance, most urgent first.
fn counts(queue: &[QueueItem], sessions: &[SessionInfo]) -> Vec<(AgentState, usize)> {
    let mut out = Vec::new();
    for state in [
        AgentState::NeedsInput,
        AgentState::Blocked,
        AgentState::ReviewReady,
        AgentState::Interrupted,
        AgentState::Stale,
    ] {
        let n = queue.iter().filter(|q| q.status.state == state).count();
        if n > 0 {
            out.push((state, n));
        }
    }
    let working = sessions
        .iter()
        .filter(|s| s.exited.is_none() && s.status.state == AgentState::Working)
        .count();
    if working > 0 {
        out.push((AgentState::Working, working));
    }
    out
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// `42s`, `5m`, `3h`, `2d`.
fn age(since_ms: u64, now_ms: u64) -> String {
    let s = now_ms.saturating_sub(since_ms) / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

impl Attached {
    fn apply(&mut self, update: ScreenUpdate) {
        let height = update.size.rows as usize;
        self.cols = update.size.cols;
        if update.full || self.rows.len() != height {
            self.rows = (0..height as u16)
                .map(|y| Row {
                    y,
                    spans: Vec::new(),
                    wrapped: false,
                })
                .collect();
        }
        for row in update.rows {
            let y = row.y as usize;
            if y < self.rows.len() {
                self.rows[y] = row;
            }
        }
        self.cursor = update
            .cursor
            .visible
            .then_some((update.cursor.x, update.cursor.y));
        self.shape = update.cursor.shape;
        self.modes = update.modes;
        self.title = update.title;
    }
}

fn render_rows(rows: &[Row], area: Rect, buf: &mut ratatui::buffer::Buffer) {
    for row in rows {
        if row.y >= area.height {
            continue;
        }
        for span in &row.spans {
            if span.x >= area.width {
                continue;
            }
            let max = (area.width - span.x) as usize;
            buf.set_stringn(
                area.x + span.x,
                area.y + row.y,
                &span.text,
                max,
                to_style(span.style),
            );
        }
    }
}

/// Shows a mouse selection as reversed cells.
fn highlight(sel: &Selection, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    for y in 0..area.height {
        for x in 0..area.width {
            if sel.contains(x, y) {
                let cell = &mut buf[(area.x + x, area.y + y)];
                cell.modifier.toggle(Modifier::REVERSED);
            }
        }
    }
}

fn to_style(s: Style) -> style::Style {
    let mut out = style::Style::new().fg(to_color(s.fg)).bg(to_color(s.bg));
    for (flag, m) in [
        (Style::BOLD, Modifier::BOLD),
        (Style::ITALIC, Modifier::ITALIC),
        (Style::UNDERLINE, Modifier::UNDERLINED),
        (Style::INVERSE, Modifier::REVERSED),
        (Style::DIM, Modifier::DIM),
        (Style::HIDDEN, Modifier::HIDDEN),
        (Style::STRIKEOUT, Modifier::CROSSED_OUT),
    ] {
        if s.flags & flag != 0 {
            out = out.add_modifier(m);
        }
    }
    out
}

fn to_color(c: Color) -> style::Color {
    match c {
        Color::Default => style::Color::Reset,
        Color::Indexed(i) => style::Color::Indexed(i),
        Color::Rgb(r, g, b) => style::Color::Rgb(r, g, b),
    }
}

/// This terminal's cell size in pixels, if it reports its pixel size (Ghostty does,
/// and SSH forwards it).
pub fn cell_pixels() -> Option<(u16, u16)> {
    let ws = ratatui::crossterm::terminal::window_size().ok()?;
    (ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0)
        .then(|| (ws.width / ws.columns, ws.height / ws.rows))
}

/// The session gets the whole terminal minus the status bar.
pub fn session_size() -> Result<Size> {
    let (cols, rows) = ratatui::crossterm::terminal::size()?;
    Ok(Size {
        cols,
        rows: rows.saturating_sub(1).max(1),
    })
}

/// Mirrors the program's input modes.
fn write_modes(m: Modes, shape: Option<CursorShape>) -> Result<()> {
    let flag = |on: bool| if on { 'h' } else { 'l' };
    let mut out = std::io::stdout();
    write!(
        out,
        "\x1b[?1{}\x1b[?2004{}\x1b[?1004{}{}",
        flag(m.app_cursor),
        flag(m.bracketed_paste),
        flag(m.focus_events),
        if m.app_keypad { "\x1b=" } else { "\x1b>" },
    )?;
    if let Some(shape) = shape {
        let style = match shape {
            CursorShape::Block => SetCursorStyle::SteadyBlock,
            CursorShape::Underline => SetCursorStyle::SteadyUnderScore,
            CursorShape::Beam => SetCursorStyle::SteadyBar,
        };
        execute!(out, style)?;
    }
    out.flush()?;
    Ok(())
}

/// The mouse: the program's if it asked for it, else Valkyrie's (scrollback,
/// selection) while `attached`; on the home screen, nobody's.
fn write_mouse(m: Modes, attached: bool) -> Result<()> {
    let mut out = std::io::stdout();
    write!(out, "{}", mouse::CAPTURE_OFF)?;
    if attached && m.wants_mouse() {
        for (on, mode) in [
            (m.mouse_click, 1000),
            (m.mouse_drag, 1002),
            (m.mouse_motion, 1003),
            (m.mouse_sgr, 1006),
            (m.mouse_utf8, 1005),
        ] {
            if on {
                write!(out, "\x1b[?{mode}h")?;
            }
        }
    } else if attached {
        write!(out, "{}", mouse::CAPTURE_ON)?;
    }
    out.flush()?;
    Ok(())
}

/// Back to what a plain shell expects: no mirrored input modes, the user's cursor shape.
fn reset_terminal_modes() -> Result<()> {
    write_modes(Modes::default(), None)?;
    write_mouse(Modes::default(), false)?;
    // The session's images must not stay over the home screen (or the shell).
    write!(std::io::stdout(), "{CLEAR_IMAGES}")?;
    execute!(std::io::stdout(), SetCursorStyle::DefaultUserShape)?;
    Ok(())
}

/// The first key in `bytes`: a CSI or SS3 sequence, a lone Esc, or one byte.
fn next_key(bytes: &[u8]) -> &[u8] {
    match bytes {
        [0x1b, b'[', rest @ ..] => {
            let end = rest
                .iter()
                .position(|b| (0x40..=0x7e).contains(b))
                .map_or(bytes.len(), |i| i + 3);
            &bytes[..end]
        }
        [0x1b, b'O', _, ..] => &bytes[..3],
        [] => bytes,
        _ => &bytes[..1],
    }
}

/// Blocking stdin reader on its own thread; raw mode means each read is what the
/// terminal sent, so escape sequences arrive whole in practice.
fn stdin_bytes() -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel(64);
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stdin.read(&mut buf) {
            if n == 0 || tx.blocking_send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    rx
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HomeKey {
    Up,
    Down,
    Enter,
    /// Attach to the top of the queue.
    Top,
    Seen,
    SeenAll,
    NewShell,
    Kill,
    Refresh,
    /// Cycle the color theme.
    Theme,
    /// Ping sounds on/off.
    Sound,
    Quit,
}

fn home_keys(bytes: &[u8]) -> Vec<HomeKey> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &bytes[i..];
        // The terminal's late answer to an image command (`ESC _ G ... ESC \`).
        if rest.starts_with(b"\x1b_") {
            i += rest
                .windows(2)
                .position(|w| w == b"\x1b\\")
                .map_or(rest.len(), |end| end + 2);
            continue;
        }
        if rest.len() >= 2 && (rest.starts_with(b"\x1b[") || rest.starts_with(b"\x1bO")) {
            // Whole sequences, so a stray mouse report (`ESC[<0;5;3m`) is not read as
            // keys; an X10 report (`ESC[M` + 3 raw bytes) has no final byte at all.
            let key = next_key(rest);
            match key {
                b"\x1b[A" | b"\x1bOA" => keys.push(HomeKey::Up),
                b"\x1b[B" | b"\x1bOB" => keys.push(HomeKey::Down),
                _ => {}
            }
            i += if key == b"\x1b[M" { 6 } else { key.len() };
            continue;
        }
        match rest[0] {
            b'k' => keys.push(HomeKey::Up),
            b'j' => keys.push(HomeKey::Down),
            b'\r' | b'\n' => keys.push(HomeKey::Enter),
            b'\t' => keys.push(HomeKey::Top),
            b's' => keys.push(HomeKey::Seen),
            b'S' => keys.push(HomeKey::SeenAll),
            b'n' => keys.push(HomeKey::NewShell),
            b'x' => keys.push(HomeKey::Kill),
            b'r' => keys.push(HomeKey::Refresh),
            b't' => keys.push(HomeKey::Theme),
            b'm' => keys.push(HomeKey::Sound),
            b'q' | 0x03 => keys.push(HomeKey::Quit),
            _ => {}
        }
        i += 1;
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::{Cursor, Span};

    #[test]
    fn parses_home_keys() {
        assert_eq!(
            home_keys(b"j\x1b[Ak\x1bOB\r\tsSq"),
            vec![
                HomeKey::Down,
                HomeKey::Up,
                HomeKey::Up,
                HomeKey::Down,
                HomeKey::Enter,
                HomeKey::Top,
                HomeKey::Seen,
                HomeKey::SeenAll,
                HomeKey::Quit
            ]
        );
        // Mouse reports arriving after a detach are not keys (`m` would toggle sound,
        // an X10 report's raw bytes could be `q` or `x`).
        assert_eq!(home_keys(b"\x1b[<0;5;3m\x1b[Mqxj"), []);
        assert_eq!(home_keys(b"\x1b[<0;5;3mj"), [HomeKey::Down]);
        // Nor is the terminal's answer to an image command.
        assert_eq!(home_keys(b"\x1b_Gi=1;EINVAL:q\x1b\\j"), [HomeKey::Down]);
    }

    fn update(full: bool, rows: Vec<Row>) -> ScreenUpdate {
        ScreenUpdate {
            full,
            size: Size { cols: 10, rows: 3 },
            rows,
            cursor: Cursor {
                x: 1,
                y: 2,
                visible: true,
                shape: CursorShape::Beam,
            },
            modes: Modes {
                bracketed_paste: true,
                ..Modes::default()
            },
            title: Some("t".into()),
        }
    }

    fn row(y: u16, text: &str) -> Row {
        Row {
            y,
            spans: vec![Span {
                x: 0,
                text: text.into(),
                style: Style::default(),
            }],
            wrapped: false,
        }
    }

    fn info(id: SessionId) -> SessionInfo {
        SessionInfo {
            id,
            name: String::new(),
            command: vec![],
            cwd: "/".into(),
            pid: None,
            created_unix: 0,
            title: None,
            clients: 0,
            exited: None,
            status: AgentStatus::default(),
        }
    }

    fn queued(id: SessionId) -> QueueItem {
        QueueItem {
            session: id,
            name: String::new(),
            cwd: "/".into(),
            status: AgentStatus::default(),
        }
    }

    #[tokio::test]
    async fn cursor_follows_its_session_when_the_queue_changes() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-{}", std::process::id()));
        // A client is needed to build an App; this one is never used.
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.sessions = vec![info(1), info(2), info(3)];
        app.selected = 2; // session 3
        app.on_push(ServerMsg::Queue {
            items: vec![queued(1)],
        })
        .unwrap();
        assert_eq!(app.selection().0, Some(3));
        assert_eq!(app.selected, 3);
        app.selected = 0; // queue row for session 1
        app.on_push(ServerMsg::Queue {
            items: vec![queued(2), queued(1)],
        })
        .unwrap();
        assert_eq!(app.selection(), (Some(1), app.queue.get(1)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The home screen with a realistic mix of sessions, in each theme. With
    /// `VALK_SNAPSHOT_DIR` set, also writes each render's cells (symbol, fg, bg,
    /// bold) as JSON there, for eyeballing a theme as an image.
    #[tokio::test]
    async fn home_screen_renders_queue_sessions_and_preview() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        let now = now_ms();
        let home = std::env::var("HOME").unwrap_or_default();
        let session = |id, name: &str, state, summary: Option<&str>, ago: u64, cwd: &str| {
            let mut s = info(id);
            s.name = name.into();
            s.command = vec![name.into()];
            s.cwd = format!("{home}/repos/{cwd}").into();
            s.status = AgentStatus {
                agent: "claude".into(),
                state,
                summary: summary.map(Into::into),
                since_ms: now - ago * 1000,
                hooked: true,
                ..AgentStatus::default()
            };
            s
        };
        app.sessions = vec![
            session(
                1,
                "api-auth",
                AgentState::NeedsInput,
                Some("Permission: Bash cargo test -p auth"),
                42,
                "api",
            ),
            session(2, "web-ui", AgentState::Working, None, 380, "web"),
            session(
                3,
                "migrations",
                AgentState::ReviewReady,
                Some("Done: backfill script added · uncommitted: 3 files +120 -8"),
                900,
                "api",
            ),
            session(
                4,
                "infra",
                AgentState::Blocked,
                Some("exited with code 101"),
                3600,
                "infra",
            ),
            session(5, "docs", AgentState::Idle, None, 7200, "docs"),
        ];
        app.sessions[1].clients = 1;
        app.queue = [0, 3, 2]
            .iter()
            .map(|&i| {
                let s = &app.sessions[i];
                QueueItem {
                    session: s.id,
                    name: s.name.clone(),
                    cwd: s.cwd.clone(),
                    status: s.status.clone(),
                }
            })
            .collect();
        app.preview = Some((
            1,
            "● Running the auth tests\n  ⎿  $ cargo test -p auth\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. No".into(),
        ));
        for theme in theme::THEMES {
            app.theme = theme;
            let mut terminal = Terminal::new(TestBackend::new(160, 22)).unwrap();
            terminal
                .draw(|frame| {
                    let [body, bar] =
                        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                            .areas(frame.area());
                    app.draw_home(frame, body, bar);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let text: String = buffer.content().iter().map(|c| c.symbol()).collect();
            for want in [
                "VALKYRIE",
                "1 needs input",
                "cargo test -p auth",
                "backfill",
                "web-ui",
                "Do you want to proceed?",
                "theme",
            ] {
                assert!(text.contains(want), "{} theme lacks {want:?}", theme.name);
            }
            if let Some(out) = std::env::var_os("VALK_SNAPSHOT_DIR") {
                let color = |c: style::Color| match c {
                    style::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
                    _ => String::new(),
                };
                let rows: Vec<Vec<serde_json::Value>> = (0..buffer.area.height)
                    .map(|y| {
                        (0..buffer.area.width)
                            .map(|x| {
                                let c = &buffer[(x, y)];
                                serde_json::json!([
                                    c.symbol(),
                                    color(c.fg),
                                    color(c.bg),
                                    c.modifier.contains(Modifier::BOLD)
                                ])
                            })
                            .collect()
                    })
                    .collect();
                let path = PathBuf::from(out).join(format!("{}.json", theme.name));
                std::fs::write(path, serde_json::to_string(&rows).unwrap()).unwrap();
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn applies_full_then_partial_updates() {
        let mut view = Attached::new(1, "x".into(), None);
        view.apply(update(true, vec![row(0, "a"), row(1, "b"), row(2, "c")]));
        view.apply(update(false, vec![row(1, "B"), row(9, "ignored")]));
        let texts: Vec<&str> = view.rows.iter().map(|r| r.spans[0].text.as_str()).collect();
        assert_eq!(texts, ["a", "B", "c"]);
        assert_eq!(view.cursor, Some((1, 2)));
        assert_eq!(view.shape, CursorShape::Beam);
        assert!(view.modes.bracketed_paste);
    }

    #[test]
    fn renders_spans_into_buffer() {
        let mut view = Attached::new(1, "x".into(), None);
        view.apply(update(
            true,
            vec![Row {
                y: 1,
                spans: vec![Span {
                    x: 2,
                    text: "日x".into(),
                    style: Style {
                        flags: Style::BOLD,
                        ..Style::default()
                    },
                }],
                wrapped: false,
            }],
        ));
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_rows(&view.rows, area, &mut buf);
        assert_eq!(buf[(2, 1)].symbol(), "日");
        assert_eq!(buf[(4, 1)].symbol(), "x");
        assert!(buf[(4, 1)].modifier.contains(Modifier::BOLD));
    }
}

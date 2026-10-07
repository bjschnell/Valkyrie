//! Terminal client. Talks to the daemon only through `overseer-proto` (ADR-0004).
//!
//! Input is read as raw stdin bytes and forwarded untouched while attached, so the
//! program sees exactly what the user's terminal sends. For that to be correct the
//! outer terminal mirrors the program's input modes (app cursor, bracketed paste…).

use anyhow::Result;
use overseer_proto::client::{Client, Pushes};
use overseer_proto::{
    AgentState, AgentStatus, Color, CursorShape, Modes, QueueItem, Row, ScreenUpdate, ServerMsg,
    SessionId, SessionInfo, Size, SpawnSpec, Style,
};
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{self, Modifier, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use std::io::{Read, Write};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

/// Ctrl-] — detach from the attached session.
pub const DETACH_KEY: u8 = 0x1d;

pub async fn run(client: Client, pushes: Pushes, attach_to: Option<SessionId>) -> Result<()> {
    let mut terminal = ratatui::try_init()?;
    // ratatui's own hook (installed by init) restores raw mode and the screen; chain
    // ours in front so mirrored input modes don't outlive a panic either.
    let ratatui_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = reset_terminal_modes();
        ratatui_hook(info);
    }));
    let result = App::new(client).run(&mut terminal, pushes, attach_to).await;
    let _ = reset_terminal_modes();
    ratatui::restore();
    result
}

struct App {
    client: Client,
    /// The ranked attention queue, as last pushed by the daemon.
    queue: Vec<QueueItem>,
    sessions: Vec<SessionInfo>,
    /// One selection over the queue rows followed by the session rows.
    selected: usize,
    view: Option<Attached>,
    status: String,
    quit: bool,
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
}

impl App {
    fn new(client: Client) -> Self {
        Self {
            client,
            queue: Vec::new(),
            sessions: Vec::new(),
            selected: 0,
            view: None,
            status: String::new(),
            quit: false,
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
                        self.status = "daemon connection lost".into();
                        break;
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
                    if let Some(view) = &self.view
                        && let Ok(size) = session_size()
                    {
                        let _ = self.client.resize(view.id, size);
                    }
                    terminal.autoresize()?;
                }
                _ = tick.tick() => self.refresh().await,
            }
        }
        Ok(())
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
        if std::env::var("OVERSEER_SESSION").is_ok_and(|own| own == id.to_string()) {
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
        self.view = Some(Attached {
            id,
            name,
            rows: Vec::new(),
            cursor: None,
            shape: CursorShape::Block,
            modes: Modes::default(),
            title: None,
            exited: None,
            status,
        });
        if let Err(e) = self.client.attach(id, size).await {
            self.view = None;
            self.status = format!("attach {id} failed: {e:#}");
        }
    }

    async fn detach(&mut self) {
        self.view = None;
        let _ = reset_terminal_modes();
        let _ = self.client.detach().await;
        self.refresh().await;
    }

    async fn on_input(&mut self, bytes: Vec<u8>) {
        if let Some(view) = &self.view {
            if view.exited.is_some() {
                return self.detach().await;
            }
            let id = view.id;
            match bytes.iter().position(|&b| b == DETACH_KEY) {
                Some(i) => {
                    if i > 0 {
                        let _ = self.client.input(id, bytes[..i].to_vec());
                    }
                    self.detach().await;
                    // Keys typed right after ^] in the same read belong to the home screen.
                    for key in home_keys(&bytes[i + 1..]) {
                        Box::pin(self.on_home_key(key)).await;
                    }
                }
                None => {
                    let _ = self.client.input(id, bytes);
                }
            }
            return;
        }
        for key in home_keys(&bytes) {
            self.on_home_key(key).await;
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
            HomeKey::Up => self.selected = self.selected.saturating_sub(1),
            HomeKey::Down => {
                self.selected += 1;
                self.clamp_selection();
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
                    env: overseer_proto::login_env(),
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
            }
            ServerMsg::Exited { session, code } if session == view.id => view.exited = Some(code),
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
                    render_screen(view, body, frame.buffer_mut());
                    if let Some((x, y)) = view.cursor
                        && x < body.width
                        && y < body.height
                    {
                        frame.set_cursor_position(Position::new(body.x + x, body.y + y));
                    }
                    let state = match (view.exited, &view.status) {
                        (Some(Some(code)), _) => format!(" [exited {code}] any key returns"),
                        (Some(None), _) => " [exited] any key returns".into(),
                        (None, Some(s)) => format!(" [{}]", s.state.label()),
                        (None, None) => String::new(),
                    };
                    let title = view
                        .title
                        .as_deref()
                        .map(|t| format!(" · {t}"))
                        .unwrap_or_default();
                    let text = format!(
                        " overseer · {} {}{title}{state}  ·  ^] detach ",
                        view.id, view.name
                    );
                    frame.render_widget(Paragraph::new(text).reversed(), bar);
                }
                None => self.draw_home(frame, body, bar),
            }
        })?;
        Ok(())
    }
}

impl App {
    fn draw_home(&self, frame: &mut ratatui::Frame, body: Rect, bar: Rect) {
        let now = now_ms();
        let queue_height = (self.queue.len().max(1) as u16 + 2)
            .min(body.height / 2)
            .max(3);
        let [queue_area, sessions_area] =
            Layout::vertical([Constraint::Length(queue_height), Constraint::Fill(1)]).areas(body);

        let in_queue = self.selected < self.queue.len();
        let queue_items: Vec<ListItem> = if self.queue.is_empty() {
            vec![ListItem::new(Line::from(" nothing needs you").dim())]
        } else {
            self.queue.iter().map(|q| queue_line(q, now)).collect()
        };
        let mut state = ListState::default()
            .with_selected((in_queue && !self.queue.is_empty()).then_some(self.selected));
        frame.render_stateful_widget(
            List::new(queue_items)
                .block(ratatui::widgets::Block::bordered().title(" overseer · needs you "))
                .highlight_style(Modifier::REVERSED),
            queue_area,
            &mut state,
        );

        let session_items: Vec<ListItem> =
            self.sessions.iter().map(|s| session_line(s, now)).collect();
        let mut state = ListState::default()
            .with_selected((!in_queue).then(|| self.selected - self.queue.len()));
        frame.render_stateful_widget(
            List::new(session_items)
                .block(ratatui::widgets::Block::bordered().title(" sessions "))
                .highlight_style(Modifier::REVERSED),
            sessions_area,
            &mut state,
        );

        let help = "enter attach · tab top · s/S seen · n new · x kill · q quit";
        let mut parts = vec![counts(&self.queue, &self.sessions)];
        if !self.status.is_empty() {
            parts.push(self.status.clone());
        }
        parts.push(help.into());
        frame.render_widget(
            Paragraph::new(format!(" {} ", parts.join(" · "))).reversed(),
            bar,
        );
    }
}

/// `2 needs input · 1 done · 3 working`
fn counts(queue: &[QueueItem], sessions: &[SessionInfo]) -> String {
    let mut parts = Vec::new();
    for state in [
        AgentState::NeedsInput,
        AgentState::Blocked,
        AgentState::ReviewReady,
        AgentState::Interrupted,
        AgentState::Stale,
    ] {
        let n = queue.iter().filter(|q| q.status.state == state).count();
        if n > 0 {
            parts.push(format!("{n} {}", state.label()));
        }
    }
    let working = sessions
        .iter()
        .filter(|s| s.exited.is_none() && s.status.state == AgentState::Working)
        .count();
    if working > 0 {
        parts.push(format!("{working} working"));
    }
    if parts.is_empty() {
        "all quiet".into()
    } else {
        parts.join(" · ")
    }
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

fn state_color(state: AgentState) -> style::Color {
    match state {
        AgentState::NeedsInput => style::Color::Yellow,
        AgentState::Blocked => style::Color::Red,
        AgentState::ReviewReady => style::Color::Green,
        AgentState::Interrupted | AgentState::Stale => style::Color::Magenta,
        AgentState::Working => style::Color::Cyan,
        AgentState::Idle | AgentState::Exited => style::Color::DarkGray,
    }
}

fn queue_line(q: &QueueItem, now: u64) -> ListItem<'static> {
    let s = &q.status;
    let summary = s.summary.clone().unwrap_or_default();
    ListItem::new(Line::from(vec![
        format!(" {:<12} ", s.state.label())
            .fg(state_color(s.state))
            .bold(),
        format!(
            "{:>3} {:<12} {:>4}  ",
            q.session,
            q.name,
            age(s.since_ms, now)
        )
        .into(),
        summary.into(),
        format!("  ({})", q.cwd.display()).dim(),
    ]))
}

impl Attached {
    fn apply(&mut self, update: ScreenUpdate) {
        let height = update.size.rows as usize;
        if update.full || self.rows.len() != height {
            self.rows = (0..height as u16)
                .map(|y| Row {
                    y,
                    spans: Vec::new(),
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

fn render_screen(view: &Attached, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    for row in &view.rows {
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

fn session_line(s: &SessionInfo, now: u64) -> ListItem<'static> {
    let state = match s.exited {
        None => s.status.state.label().to_string(),
        Some(Some(code)) => format!("exited {code}"),
        Some(None) => "exited".to_string(),
    };
    let color = match s.exited {
        None => state_color(s.status.state),
        Some(_) => style::Color::DarkGray,
    };
    let hooks = if s.status.agent == "generic" || s.status.hooked {
        ""
    } else {
        " (no hooks yet)"
    };
    let detail = s.title.clone().unwrap_or_else(|| s.command.join(" "));
    ListItem::new(Line::from(vec![
        format!(" {:<12} ", state).fg(color),
        format!(
            "{:>3} {:<12} {:>4}  {}c  {detail}{hooks}  ",
            s.id,
            s.name,
            age(s.status.since_ms, now),
            s.clients
        )
        .into(),
        format!("({})", s.cwd.display()).dim(),
    ]))
}

/// The session gets the whole terminal minus the status bar.
pub fn session_size() -> Result<Size> {
    let (cols, rows) = ratatui::crossterm::terminal::size()?;
    Ok(Size {
        cols,
        rows: rows.saturating_sub(1).max(1),
    })
}

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

/// Back to what a plain shell expects: no mirrored input modes, the user's cursor shape.
fn reset_terminal_modes() -> Result<()> {
    write_modes(Modes::default(), None)?;
    execute!(std::io::stdout(), SetCursorStyle::DefaultUserShape)?;
    Ok(())
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
    Quit,
}

fn home_keys(bytes: &[u8]) -> Vec<HomeKey> {
    let mut keys = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &bytes[i..];
        if rest.len() >= 3 && (rest.starts_with(b"\x1b[") || rest.starts_with(b"\x1bO")) {
            match rest[2] {
                b'A' => keys.push(HomeKey::Up),
                b'B' => keys.push(HomeKey::Down),
                _ => {}
            }
            i += 3;
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
    use overseer_proto::{Cursor, Span};

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
        let dir = std::env::temp_dir().join(format!("overseer-tui-{}", std::process::id()));
        // A client is needed to build an App; this one is never used.
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        overseer_proto::ensure_private_dir(&dir).unwrap();
        let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client);
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

    #[test]
    fn applies_full_then_partial_updates() {
        let mut view = Attached {
            id: 1,
            name: "x".into(),
            rows: Vec::new(),
            cursor: None,
            shape: CursorShape::Block,
            modes: Modes::default(),
            title: None,
            exited: None,
            status: None,
        };
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
        let mut view = Attached {
            id: 1,
            name: "x".into(),
            rows: Vec::new(),
            cursor: None,
            shape: CursorShape::Block,
            modes: Modes::default(),
            title: None,
            exited: None,
            status: None,
        };
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
            }],
        ));
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_screen(&view, area, &mut buf);
        assert_eq!(buf[(2, 1)].symbol(), "日");
        assert_eq!(buf[(4, 1)].symbol(), "x");
        assert!(buf[(4, 1)].modifier.contains(Modifier::BOLD));
    }
}

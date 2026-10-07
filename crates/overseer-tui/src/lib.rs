//! Terminal client. Talks to the daemon only through `overseer-proto` (ADR-0004).
//!
//! Input is read as raw stdin bytes and forwarded untouched while attached, so the
//! program sees exactly what the user's terminal sends. For that to be correct the
//! outer terminal mirrors the program's input modes (app cursor, bracketed paste…).

mod theme;

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
use ratatui::widgets::{
    Block, BorderType, Cell, HighlightSpacing, Padding, Paragraph, Row as TableRow, Table,
    TableState, Wrap,
};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use theme::{Theme, state_icon};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

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
                Ok(info) if info.protocol == overseer_proto::PROTOCOL => info,
                Ok(info) => anyhow::bail!(
                    "the daemon now speaks protocol {}, this overseer {}; run the new overseer",
                    info.protocol,
                    overseer_proto::PROTOCOL
                ),
                Err(_) => continue,
            };
            self.client = client;
            // Session ids start over in a restarted daemon: re-attaching by id could
            // land in an unrelated program.
            let same = self.boot == Some(info.boot);
            self.boot = Some(info.boot);
            self.status = if same {
                "reconnected to the daemon".into()
            } else {
                self.view = None;
                let _ = reset_terminal_modes();
                "the daemon restarted; sessions from before are gone".into()
            };
            if let Err(e) = self.client.watch_queue().await {
                self.status = format!("queue unavailable: {e:#}");
            }
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
            HomeKey::Up => {
                self.selected = self.selected.saturating_sub(1);
                self.update_preview().await;
            }
            HomeKey::Down => {
                self.selected += 1;
                self.clamp_selection();
                self.update_preview().await;
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
                    frame.render_widget(
                        Paragraph::new(attached_bar(view, self.theme))
                            .style(style::Style::new().bg(self.theme.panel)),
                        bar,
                    );
                }
                None => self.draw_home(frame, body, bar),
            }
        })?;
        Ok(())
    }
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

        frame.render_widget(
            Paragraph::new(footer_line(&self.status, t)).style(style::Style::new().bg(t.panel)),
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

/// ` ◆ OVERSEER  agent control` on the left, colored counts on the right.
fn header_line(queue: &[QueueItem], sessions: &[SessionInfo], t: &Theme) -> Line<'static> {
    let mut spans = vec![
        " ◆ OVERSEER ".fg(t.bg).bg(t.accent).bold(),
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
fn footer_line(status: &str, t: &Theme) -> Line<'static> {
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
        " ◆ OVERSEER ".fg(t.bg).bg(t.accent).bold(),
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
    if let Some(title) = &view.title {
        spans.push(format!(" {title} ").fg(t.muted));
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
    /// Cycle the color theme.
    Theme,
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
            b't' => keys.push(HomeKey::Theme),
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
    /// `OVERSEER_SNAPSHOT_DIR` set, also writes each render's cells (symbol, fg, bg,
    /// bold) as JSON there, for eyeballing a theme as an image.
    #[tokio::test]
    async fn home_screen_renders_queue_sessions_and_preview() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("overseer-tui-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        overseer_proto::ensure_private_dir(&dir).unwrap();
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
                "OVERSEER",
                "1 needs input",
                "cargo test -p auth",
                "backfill",
                "web-ui",
                "Do you want to proceed?",
                "theme",
            ] {
                assert!(text.contains(want), "{} theme lacks {want:?}", theme.name);
            }
            if let Some(out) = std::env::var_os("OVERSEER_SNAPSHOT_DIR") {
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

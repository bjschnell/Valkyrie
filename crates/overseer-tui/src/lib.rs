//! Terminal client. Talks to the daemon only through `overseer-proto` (ADR-0004).
//!
//! Input is read as raw stdin bytes and forwarded untouched while attached, so the
//! program sees exactly what the user's terminal sends. For that to be correct the
//! outer terminal mirrors the program's input modes (app cursor, bracketed paste…).

use anyhow::Result;
use overseer_proto::client::{Client, Pushes};
use overseer_proto::{
    Color, CursorShape, Modes, Row, ScreenUpdate, ServerMsg, SessionId, SessionInfo, Size,
    SpawnSpec, Style,
};
use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{self, Modifier, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use std::io::{Read, Write};
use std::time::Duration;
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
    sessions: Vec<SessionInfo>,
    list: ListState,
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
}

impl App {
    fn new(client: Client) -> Self {
        Self {
            client,
            sessions: Vec::new(),
            list: ListState::default().with_selected(Some(0)),
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
                    self.on_push(msg)?;
                    // Coalesce bursts into one redraw.
                    while let Ok(msg) = pushes.try_recv() {
                        self.on_push(msg)?;
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
                _ = tick.tick(), if self.view.is_none() => self.refresh().await,
            }
        }
        Ok(())
    }

    async fn refresh(&mut self) {
        match self.client.list().await {
            Ok(sessions) => {
                self.sessions = sessions;
                let max = self.sessions.len().saturating_sub(1);
                if self.list.selected().is_some_and(|i| i > max) {
                    self.list.select(Some(max));
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

    async fn on_home_key(&mut self, key: HomeKey) {
        let selected = self
            .list
            .selected()
            .and_then(|i| self.sessions.get(i))
            .map(|s| s.id);
        match key {
            HomeKey::Up => self.list.select_previous(),
            HomeKey::Down => self.list.select_next(),
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

    fn on_push(&mut self, msg: ServerMsg) -> Result<()> {
        let Some(view) = &mut self.view else {
            return Ok(());
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
        Ok(())
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
                    let state = match view.exited {
                        Some(Some(code)) => format!(" [exited {code}] any key returns"),
                        Some(None) => " [exited] any key returns".into(),
                        None => String::new(),
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
                None => {
                    let items: Vec<ListItem> = self.sessions.iter().map(session_line).collect();
                    let list = List::new(items)
                        .block(ratatui::widgets::Block::bordered().title(" overseer · sessions "))
                        .highlight_style(Modifier::REVERSED);
                    frame.render_stateful_widget(list, body, &mut self.list);
                    let help = " enter attach · n new shell · x kill · r refresh · q quit ";
                    let text = if self.status.is_empty() {
                        help.to_string()
                    } else {
                        format!(" {} ·{help}", self.status)
                    };
                    frame.render_widget(Paragraph::new(text).reversed(), bar);
                }
            }
        })?;
        Ok(())
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

fn session_line(s: &SessionInfo) -> ListItem<'static> {
    let state = match s.exited {
        None => "running".to_string(),
        Some(Some(code)) => format!("exited {code}"),
        Some(None) => "exited".to_string(),
    };
    let detail = s.title.clone().unwrap_or_else(|| s.command.join(" "));
    ListItem::new(Line::from(format!(
        " {:>3}  {:<14} {:<10} {}c  {}  ({})",
        s.id,
        s.name,
        state,
        s.clients,
        detail,
        s.cwd.display()
    )))
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
            home_keys(b"j\x1b[Ak\x1bOB\rq"),
            vec![
                HomeKey::Down,
                HomeKey::Up,
                HomeKey::Up,
                HomeKey::Down,
                HomeKey::Enter,
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

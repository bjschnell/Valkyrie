//! Terminal client. Talks to the daemon only through `valkyrie-proto` (ADR-0004).
//!
//! Input is read as raw stdin bytes and forwarded untouched while attached, so the
//! program sees exactly what the user's terminal sends. For that to be correct the
//! outer terminal mirrors the program's input modes (app cursor, bracketed paste…).

mod mouse;
mod panes;
mod ping;
mod quiet;
mod settings;
mod theme;

use anyhow::Result;
use mouse::{Input, Mouse, MouseKind, Selection};
use ping::{Kind, Ping, Pinger};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{self, Modifier, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, HighlightSpacing, Padding, Paragraph, Row as TableRow, Table,
    TableState, Wrap,
};
use settings::{Field, Settings, TabSide, TabStyle};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use theme::{Theme, state_icon};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use valkyrie_proto::client::{Client, Pushes};
use valkyrie_proto::{
    AgentState, AgentStatus, Color, CursorShape, Modes, Pane, QueueItem, Row, ScreenUpdate,
    ScrollAnchor, ServerMsg, SessionId, SessionInfo, Side, Size, SpawnSpec, Style,
};

/// Ctrl-] — detach from the attached session.
pub const DETACH_KEY: u8 = 0x1d;
/// Ctrl-\ — give the tab strip the keyboard: switch, jump, rename, start sessions.
pub const TABS_KEY: u8 = 0x1c;

/// How long the TUI keeps trying to reach the daemon after the connection drops. An
/// upgrade handoff (ADR-0006) takes milliseconds; a daemon restart is not coming back.
const RECONNECT_FOR: Duration = Duration::from_secs(10);

/// The terminal, through a backend that stays silent while nothing changes.
type Term = Terminal<quiet::Quiet<CrosstermBackend<std::io::Stdout>>>;

pub async fn run(
    client: Client,
    pushes: Pushes,
    attach_to: Option<SessionId>,
    socket: PathBuf,
) -> Result<()> {
    // Raw mode, the alternate screen and ratatui's panic hook; then the same
    // terminal behind `Quiet`.
    drop(ratatui::try_init()?);
    let mut terminal = Terminal::new(quiet::Quiet::new(CrosstermBackend::new(std::io::stdout())))
        .inspect_err(|_| ratatui::restore())?;
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
    /// Theme, tabs and sound; `,` opens the panel that changes them.
    settings: Settings,
    /// Where `settings` are kept.
    settings_file: PathBuf,
    /// The settings panel is open, its cursor on this field.
    settings_cursor: Option<usize>,
    /// The selected session's screen text, for the preview pane.
    preview: Option<(SessionId, String)>,
    pinger: Pinger,
    /// The last ping, shown in the bar for `TOAST_FOR`.
    toast: Option<(Ping, Instant)>,
    /// The start of a mouse report the last read cut off.
    partial: Vec<u8>,
    /// The tab strip has the keyboard, its cursor on this tab (one past the sessions
    /// is "+").
    tab_cursor: Option<usize>,
    /// The session being renamed, and the name typed so far.
    renaming: Option<(SessionId, String)>,
    /// A tab's menu, opened with a right click or `x` in tab mode.
    menu: Option<Menu>,
    /// The tab being dragged to a new place.
    tab_drag: Option<SessionId>,
    /// The attached tab's other panes, when it is split; `view` is the focused one.
    others: Vec<Attached>,
    /// The attached tab's split layout (`None`: one pane).
    layout: Option<Pane>,
    /// Every split tab's layout, from the last session list.
    layouts: Vec<Pane>,
    /// The divider being dragged, and the ratio it was last dragged to.
    divider_drag: Option<(valkyrie_proto::layout::Divider, u16)>,
    /// The pane a button went down in: its drag and release go there too.
    mouse_owner: Option<SessionId>,
    /// The mouse capture written to the terminal: whether plain motion is on.
    captured: Option<bool>,
    /// Tests draw at this size (terminal minus the bar) instead of the terminal's.
    fixed_size: Option<Size>,
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
    /// The size last asked of the daemon for this pane.
    size: Size,
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
            size: Size { cols: 0, rows: 0 },
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
            settings: Settings::load(),
            settings_file: settings::path(),
            settings_cursor: None,
            preview: None,
            pinger: Pinger::default(),
            toast: None,
            partial: Vec::new(),
            tab_cursor: None,
            renaming: None,
            menu: None,
            tab_drag: None,
            others: Vec::new(),
            layout: None,
            layouts: Vec::new(),
            divider_drag: None,
            mouse_owner: None,
            captured: None,
            fixed_size: None,
        }
    }

    async fn run(
        &mut self,
        terminal: &mut Term,
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
                    let running: Vec<SessionId> = self
                        .view
                        .iter()
                        .chain(&self.others)
                        .filter(|v| v.exited.is_none())
                        .map(|v| v.id)
                        .collect();
                    let mut queue_changed = self.on_push(msg)?;
                    // Coalesce bursts into one redraw.
                    while let Ok(msg) = pushes.try_recv() {
                        queue_changed |= self.on_push(msg)?;
                    }
                    let ended = running.into_iter().find(|&id| {
                        self.view.iter().chain(&self.others).any(|v| v.id == id && v.exited.is_some())
                    });
                    if let Some(id) = ended {
                        self.on_ended(id).await;
                    } else if queue_changed {
                        self.refresh().await;
                    }
                    if pushes.take_lagged() {
                        self.resync().await;
                    }
                }
                _ = winch.recv() => {
                    // A font size change resizes the window too. First, so the PTY
                    // resize below carries the new pixel size in one SIGWINCH.
                    for id in self.pane_ids() {
                        self.report_cell(Some(id));
                    }
                    if self.view.is_none() {
                        self.report_cell(None);
                    }
                    self.relayout();
                    terminal.backend_mut().forget();
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
    async fn reconnect(&mut self, terminal: &mut Term) -> Result<Pushes> {
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
                self.others.clear();
                self.layout = None;
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

    /// Something on the home screen or the tab strip is spinning.
    fn animating(&self) -> bool {
        self.sessions
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
        match self.client.list_tabs().await {
            Ok((sessions, layouts)) => {
                let anchor = self.anchor();
                self.sessions = sessions;
                self.layouts = layouts;
                self.restore(anchor);
                for id in self.pane_ids() {
                    let status = self.sessions_status(id);
                    // A shell's name follows the agent in its foreground.
                    let name = self
                        .sessions
                        .iter()
                        .find(|s| s.id == id)
                        .map(|s| s.name.clone());
                    if let Some(pane) = self.pane_mut(id) {
                        pane.status = status;
                        if let Some(name) = name {
                            pane.name = name;
                        }
                    }
                }
                self.sync_panes().await;
            }
            Err(e) => self.status = format!("list failed: {e:#}"),
        }
    }

    /// Screen pushes were dropped while this TUI couldn't keep up (its terminal
    /// stalled): attach again for a fresh snapshot, keeping the view as it is.
    async fn resync(&mut self) {
        // An exited session too: its last output may be among the dropped pushes.
        let Some(id) = self.view.as_ref().map(|v| v.id) else {
            return;
        };
        // A dropped image delete would leave its image up; the replay after the
        // snapshot puts back the ones still shown.
        let mut out = std::io::stdout();
        let _ = write!(out, "{CLEAR_IMAGES}").and_then(|()| out.flush());
        let attached = match self.session_size() {
            Ok(size) => self.client.attach(id, size).await,
            Err(e) => Err(e),
        };
        if let Err(e) = attached {
            self.status = format!("resync failed: {e:#}");
        }
    }

    /// Attaches to `id`'s tab, every pane of it, with `id` focused.
    async fn attach(&mut self, id: SessionId) {
        self.attach_tab(id).await;
    }

    async fn detach(&mut self) {
        self.view = None;
        self.others.clear();
        self.layout = None;
        self.mouse_owner = None;
        self.divider_drag = None;
        self.tab_cursor = None;
        self.menu = None;
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
        if self.settings_cursor.is_some() {
            return self.settings_keys(&bytes);
        }
        if self.view.is_some() {
            // Every report is Valkyrie's first: it finds the pane under the pointer,
            // and passes reports on to a program that asked for the mouse.
            // A report cut off by the end of this read finishes in the next one.
            if let Some(cut) = mouse::unfinished(&bytes) {
                self.partial = bytes.split_off(cut);
            }
            let inputs = mouse::split(&bytes);
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
                    // A finished program: any key returns home (or to the next pane),
                    // unless you are reading its scrollback.
                    (Some(view), Input::Bytes(keys))
                        if view.exited.is_some()
                            && view.scroll.is_none()
                            && self.tab_cursor.is_none()
                            && self.renaming.is_none()
                            && self.menu.is_none()
                            && !keys.contains(&TABS_KEY) =>
                    {
                        if self.others.is_empty() {
                            self.detach().await;
                        } else {
                            self.cycle_focus(1);
                        }
                    }
                    (Some(_), Input::Bytes(bytes)) => self.on_keys(bytes).await,
                }
            }
            return;
        }
        if self.renaming.is_some() {
            return self.rename_keys(&bytes).await;
        }
        for key in home_keys(&bytes) {
            self.on_home_key(key).await;
        }
    }

    /// Keys while attached: scrollback keys when scrolled back, else the program's.
    async fn on_keys(&mut self, bytes: Vec<u8>) {
        if self.renaming.is_some() {
            return self.rename_keys(&bytes).await;
        }
        if self.menu.is_some() {
            return self.menu_keys(&bytes).await;
        }
        if self.tab_cursor.is_some() {
            return self.tab_keys(&bytes).await;
        }
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
        match rest.iter().position(|&b| b == DETACH_KEY || b == TABS_KEY) {
            Some(i) => {
                if i > 0 {
                    let _ = self.client.input(id, rest[..i].to_vec());
                }
                if rest[i] == TABS_KEY {
                    self.tab_cursor = Some(self.attached_index().unwrap_or(0));
                    // Keys typed right after ^\ in the same read belong to the strip.
                    return Box::pin(self.tab_keys(&rest[i + 1..])).await;
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

    /// The attached tab's place among the tabs.
    fn attached_index(&self) -> Option<usize> {
        self.tab_of(self.view.as_ref()?.id)
    }

    /// The session tab `i` shows (see `face`).
    fn tab_session(&self, i: usize) -> Option<&SessionInfo> {
        let tabs = self.tabs();
        let tab = tabs.get(i)?;
        Some(&self.sessions[self.face(tab)])
    }

    /// Keys while the tab strip has the keyboard. Keys after the one that hands the
    /// keyboard back go to the session.
    async fn tab_keys(&mut self, mut keys: &[u8]) {
        while let Some(cursor) = self.tab_cursor
            && !keys.is_empty()
        {
            let key = next_key(keys);
            // A lone Esc is a key; Esc with more after it is Alt plus that key.
            let whole = key.len() == keys.len();
            keys = &keys[key.len()..];
            let last = self.tabs().len();
            match key {
                // Across a strip on top, down a column; either works for either.
                b"h" | b"k" | b"\x1b[D" | b"\x1bOD" | b"\x1b[A" | b"\x1bOA" => {
                    self.tab_cursor = Some(cursor.saturating_sub(1))
                }
                b"l" | b"j" | b"\x1b[C" | b"\x1bOC" | b"\x1b[B" | b"\x1bOB" => {
                    self.tab_cursor = Some((cursor + 1).min(last))
                }
                b"," => {
                    self.tab_cursor = None;
                    self.settings_cursor = Some(0);
                    return self.settings_keys(keys);
                }
                b"\r" | b"\n" => {
                    self.tab_cursor = None;
                    match self.tab_session(cursor).map(|s| s.id) {
                        Some(id) => self.switch_to(id).await,
                        None => self.new_shell(self.attached_cwd()).await,
                    }
                }
                b"0" => {
                    self.tab_cursor = None;
                    self.detach().await;
                    return;
                }
                b"H" | b"L" | b"K" | b"J" => {
                    if let Some(id) = self.tab_session(cursor).map(|s| s.id) {
                        let to = if matches!(key, b"H" | b"K") {
                            cursor.saturating_sub(1)
                        } else {
                            cursor + 1
                        };
                        self.move_tab(id, to).await;
                    }
                }
                [d @ b'1'..=b'9'] => {
                    let i = (d - b'1') as usize;
                    if let Some(id) = self.tab_session(i).map(|s| s.id) {
                        self.tab_cursor = None;
                        self.switch_to(id).await;
                    }
                }
                b"\t" => {
                    self.tab_cursor = None;
                    self.next_needing().await;
                }
                b"n" => {
                    self.tab_cursor = None;
                    self.new_shell(self.attached_cwd()).await;
                }
                b"r" => {
                    if let Some(s) = self.tab_session(cursor) {
                        let renaming = Some((s.id, s.name.clone()));
                        self.tab_cursor = None;
                        self.renaming = renaming;
                        // What follows `r` in the same read is the new name.
                        return Box::pin(self.rename_keys(keys)).await;
                    }
                }
                b"x" => {
                    if let Some(id) = self.tab_session(cursor).map(|s| s.id) {
                        self.tab_cursor = None;
                        self.open_menu(id, MENU_CLOSE);
                        return Box::pin(self.menu_keys(keys)).await;
                    }
                }
                // Splits of the focused pane, vim's way round: `s` below, `v` beside.
                b"s" | b"v" | b"S" | b"V" => {
                    self.tab_cursor = None;
                    let side = match key {
                        b"s" => Side::Down,
                        b"v" => Side::Right,
                        b"S" => Side::Up,
                        _ => Side::Left,
                    };
                    self.split(side).await;
                }
                b"o" | b"O" => {
                    self.tab_cursor = None;
                    self.cycle_focus(if key == b"o" { 1 } else { -1 });
                }
                b"p" => {
                    self.tab_cursor = None;
                    self.open_focused_pane_menu();
                    return Box::pin(self.menu_keys(keys)).await;
                }
                [DETACH_KEY] => {
                    self.tab_cursor = None;
                    self.detach().await;
                    return;
                }
                [TABS_KEY] | b"q" => self.tab_cursor = None,
                b"\x1b" if whole => self.tab_cursor = None,
                _ => {}
            }
        }
        if !keys.is_empty() && self.view.is_some() {
            Box::pin(self.on_keys(keys.to_vec())).await;
        }
    }

    /// Attaches to the next session that needs you, in queue order after this one.
    async fn next_needing(&mut self) {
        let here = self.view.as_ref().map(|v| v.id);
        let n = self.queue.len();
        let start = self
            .queue
            .iter()
            .position(|q| Some(q.session) == here)
            .map_or(0, |i| i + 1);
        let next = (0..n)
            .map(|k| self.queue[(start + k) % n].session)
            .find(|&id| Some(id) != here);
        match next {
            Some(id) => self.switch_to(id).await,
            None => {
                if let Some(view) = &mut self.view {
                    view.notice("nothing else needs you".into());
                }
            }
        }
    }

    /// Keys while a name is being typed: Enter saves (blank: back to the directory's
    /// name), Esc cancels, Backspace and Ctrl-U erase.
    async fn rename_keys(&mut self, mut keys: &[u8]) {
        let mut typed = Vec::new();
        while let Some((id, name)) = &mut self.renaming
            && !keys.is_empty()
        {
            let key = next_key(keys);
            let whole = key.len() == keys.len();
            keys = &keys[key.len()..];
            if key.len() == 1 && key[0] >= 0x20 && key[0] != 0x7f {
                typed.push(key[0]);
                continue;
            }
            name.push_str(&String::from_utf8_lossy(&std::mem::take(&mut typed)));
            match key {
                b"\r" | b"\n" => {
                    let (id, name) = (*id, name.trim().to_string());
                    self.renaming = None;
                    let name = (!name.is_empty()).then_some(name);
                    if let Err(e) = self.client.rename(id, name).await {
                        self.say(format!("rename failed: {e:#}"));
                    }
                    self.refresh().await;
                }
                [0x7f] | [0x08] => {
                    name.pop();
                }
                [0x15] => name.clear(),
                b"\x1b" if whole => self.renaming = None,
                [TABS_KEY] | [0x03] => self.renaming = None,
                _ => {}
            }
        }
        if let Some((_, name)) = &mut self.renaming {
            name.push_str(&String::from_utf8_lossy(&typed));
            // A name fits a tab.
            while name.chars().count() > NAME_MAX {
                name.pop();
            }
        }
    }

    /// A message in the bar: the attached session's, else the home screen's.
    fn say(&mut self, text: String) {
        match &mut self.view {
            Some(view) => view.notice(text),
            None => self.status = text,
        }
    }

    /// Attaches to `id` from another session, unless it is the one already attached.
    async fn switch_to(&mut self, id: SessionId) {
        if self.others.iter().any(|p| p.id == id) {
            self.focus(id);
        } else if self.view.as_ref().is_none_or(|v| v.id != id) {
            self.attach(id).await;
        }
    }

    /// The attached session's directory, for a new shell started beside it.
    fn attached_cwd(&self) -> Option<PathBuf> {
        let id = self.view.as_ref()?.id;
        Some(self.sessions.iter().find(|s| s.id == id)?.cwd.clone())
    }

    /// Starts `$SHELL` (in `cwd`, else the daemon's default) and attaches to it.
    async fn new_shell(&mut self, cwd: Option<PathBuf>) {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let size = self.session_size().unwrap_or(Size { cols: 80, rows: 24 });
        let spec = SpawnSpec {
            command: vec![shell],
            cwd,
            name: None,
            size,
            env: valkyrie_proto::login_env(),
        };
        match self.client.spawn(spec).await {
            Ok(info) => {
                self.refresh().await;
                self.attach(info.id).await;
            }
            Err(e) => self.say(format!("spawn failed: {e:#}")),
        }
    }

    /// A click on the tab strip: switch to that session or start a new shell; a
    /// right click renames.
    async fn tab_click(&mut self, tabs: Rect, m: Mouse) {
        // While dragging, only along the strip matters: the pointer may stray off it.
        let (x, y) = match (self.tab_drag.is_some(), self.settings.tab_side) {
            (false, _) => (m.x, m.y),
            (true, TabSide::Top) => (m.x, tabs.y),
            (true, _) => (tabs.x + tabs.width / 2, m.y),
        };
        let hit = self.tab_hit(tabs, x, y);
        if m.kind == MouseKind::Press {
            self.tab_drag = match hit {
                Some(TabHit::Session(id)) => Some(id),
                _ => None,
            };
        }
        match (m.kind, hit) {
            (MouseKind::Drag, Some(TabHit::Session(over))) => {
                if let Some(id) = self.tab_drag
                    && let Some(to) = self.tab_of(over)
                {
                    self.move_tab(id, to).await;
                }
            }
            (MouseKind::Release, _) => self.tab_drag = None,
            (MouseKind::Press, Some(TabHit::Home)) => {
                self.tab_cursor = None;
                self.detach().await;
            }
            (MouseKind::Press, Some(TabHit::Session(id))) => {
                self.tab_cursor = None;
                // A click on the attached tab keeps the pane you were in.
                if !self.pane_ids().contains(&id) {
                    self.switch_to(id).await;
                }
            }
            (MouseKind::Press, Some(TabHit::New)) => {
                self.tab_cursor = None;
                self.new_shell(self.attached_cwd()).await;
            }
            (MouseKind::RightPress, Some(TabHit::Session(id))) => {
                self.tab_cursor = None;
                self.open_menu(id, MENU_RENAME);
            }
            _ => {}
        }
    }

    /// Moves a tab to place `to`: here at once, so a drag follows the pointer, and
    /// in the daemon, which keeps the order for every client.
    async fn move_tab(&mut self, id: SessionId, to: usize) {
        let mut tabs = self.tabs();
        let Some(from) = self.tab_of(id) else {
            return;
        };
        let to = to.min(tabs.len() - 1);
        if from == to {
            return;
        }
        let tab = tabs.remove(from);
        tabs.insert(to, tab);
        let mut old: Vec<Option<SessionInfo>> = std::mem::take(&mut self.sessions)
            .into_iter()
            .map(Some)
            .collect();
        self.sessions = tabs
            .iter()
            .flatten()
            .filter_map(|&i| old[i].take())
            .collect();
        if self.tab_cursor.is_some() {
            self.tab_cursor = Some(to);
        }
        if let Err(e) = self.client.move_session(id, to).await {
            self.say(format!("move failed: {e:#}"));
        }
    }

    /// Opens a tab's menu under the tab (beside it, for a column), the cursor on
    /// `item`.
    fn open_menu(&mut self, id: SessionId, item: usize) {
        let tabs =
            self.tabs_area()
                .unwrap_or(Rect::new(0, 0, MENU_WIDTH, self.settings.tabs.rows()));
        let index = self.tab_of(id);
        let tab = self
            .tab_layout(tabs)
            .tabs
            .iter()
            .find(|(i, _)| Some(*i) == index)
            .map_or(tabs, |&(_, r)| r);
        let (x, y) = match self.settings.tab_side {
            TabSide::Top => (
                tab.x.min(tabs.right().saturating_sub(MENU_WIDTH)),
                tabs.bottom(),
            ),
            TabSide::Left => (tabs.right(), tab.y),
            TabSide::Right => (tabs.x.saturating_sub(MENU_WIDTH), tab.y),
        };
        let mut menu = Menu {
            kind: MenuKind::Tab,
            session: id,
            x,
            y,
            cursor: item,
            confirm: false,
        };
        // Beside a column, kept above the bar.
        if self.settings.tab_side != TabSide::Top {
            menu.y = y.min(tabs.bottom().saturating_sub(menu.rect().height));
        }
        self.menu = Some(menu);
    }

    /// Keys while a tab's menu is open: move, pick (`r`/`x` pick directly), Esc.
    async fn menu_keys(&mut self, mut keys: &[u8]) {
        while let Some(menu) = &mut self.menu
            && !keys.is_empty()
        {
            let key = next_key(keys);
            let whole = key.len() == keys.len();
            keys = &keys[key.len()..];
            match key {
                b"k" | b"\x1b[A" | b"\x1bOA" => menu.cursor = menu.cursor.saturating_sub(1),
                b"j" | b"\x1b[B" | b"\x1bOB" => {
                    menu.cursor = (menu.cursor + 1).min(menu.items().len() - 1)
                }
                b"\r" | b"\n" => {
                    let item = menu.cursor;
                    self.pick(item).await;
                }
                b"r" if menu.kind == MenuKind::Tab => self.pick(MENU_RENAME).await,
                b"x" | b"c" => {
                    let close = menu.items().len() - 1;
                    self.pick(close).await
                }
                [TABS_KEY] | b"q" => self.menu = None,
                b"\x1b" if whole => self.menu = None,
                _ => {}
            }
        }
    }

    /// A click while a menu is open: on an item picks it; anywhere else closes the
    /// menu (a right click on another tab opens that tab's).
    async fn menu_click(&mut self, m: Mouse) {
        let Some(menu) = &self.menu else { return };
        let rect = menu.rect();
        let at = Position::new(m.x, m.y);
        match m.kind {
            MouseKind::Press if rect.contains(at) => {
                let row = m.y.saturating_sub(rect.y + 1) as usize;
                if row < menu.items().len() && m.y > rect.y {
                    self.pick(row).await;
                }
            }
            MouseKind::Press => self.menu = None,
            MouseKind::RightPress if !rect.contains(at) => {
                self.menu = None;
                if let Some(tabs) = self.tabs_area()
                    && tabs.contains(Position::new(m.x, m.y))
                {
                    Box::pin(self.tab_click(tabs, m)).await;
                }
            }
            _ => {}
        }
    }

    /// Does what a menu item says. Closing a session that runs an agent asks first.
    async fn pick(&mut self, item: usize) {
        let Some(menu) = &mut self.menu else { return };
        let id = menu.session;
        let close = item + 1 == menu.items().len();
        if menu.kind == MenuKind::Pane && !close {
            return self.pick_pane(id, item).await;
        }
        if menu.kind == MenuKind::Tab && item == MENU_RENAME {
            self.menu = None;
            let name = self
                .sessions
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.name.clone());
            self.renaming = Some((id, name.unwrap_or_default()));
            return;
        }
        // Closing a tab closes every pane in it.
        let ids = match menu.kind {
            MenuKind::Tab => self.tab_ids(id),
            MenuKind::Pane => vec![id],
        };
        let agent = self
            .sessions
            .iter()
            .filter(|s| ids.contains(&s.id))
            .any(|s| s.exited.is_none() && is_agent(s));
        let Some(menu) = &mut self.menu else { return };
        if agent && !menu.confirm {
            menu.confirm = true;
            menu.cursor = item;
            return;
        }
        let kind = menu.kind;
        self.menu = None;
        match kind {
            MenuKind::Pane => self.close_session(id).await,
            MenuKind::Tab => self.close_tab(id).await,
        }
    }

    /// The sessions in `id`'s tab.
    fn tab_ids(&self, id: SessionId) -> Vec<SessionId> {
        match self.layouts.iter().find(|p| p.contains(id)) {
            Some(p) => p.sessions(),
            None => vec![id],
        }
    }

    /// Kills every pane of `id`'s tab. Closing the attached tab moves to the tab
    /// beside it, or home when it was the last.
    async fn close_tab(&mut self, id: SessionId) {
        let ids = self.tab_ids(id);
        if ids.len() == 1 {
            return self.close_session(id).await;
        }
        let attached = self.pane_ids().contains(&id);
        let next = self.beside(id);
        for &pane in &ids {
            if let Err(e) = self.client.kill(pane).await {
                return self.say(format!("close failed: {e:#}"));
            }
        }
        if attached {
            match next {
                Some(next) => self.attach(next).await,
                None => self.detach().await,
            }
        }
        self.refresh().await;
    }

    /// Kills a session. Closing the attached one moves to the tab beside it, or
    /// home when it was the last.
    async fn close_session(&mut self, id: SessionId) {
        // A pane of a split tab: the tab stays, and a neighbor takes the room.
        let attached = self.pane_ids().contains(&id) && self.others.is_empty();
        let next = self.beside(id);
        if let Err(e) = self.client.kill(id).await {
            return self.say(format!("close failed: {e:#}"));
        }
        if attached {
            self.leave(id, next).await;
        }
        self.refresh().await;
    }

    /// The attached session ended. One that leaves the tabs (a shell, anything that
    /// exited 0) goes as if closed; a failure stays on screen until a key.
    async fn on_ended(&mut self, id: SessionId) {
        // One pane of several: refreshing takes it out of the layout.
        let alone = self.others.is_empty();
        let next = self.beside(id);
        self.refresh().await;
        if alone && !self.sessions.iter().any(|s| s.id == id) {
            self.leave(id, next).await;
        }
    }

    /// The tab to the right of `id`'s, else the one to its left.
    fn beside(&self, id: SessionId) -> Option<SessionId> {
        let i = self.tab_of(id)?;
        let right = self.tab_session(i + 1);
        let left = i.checked_sub(1).and_then(|l| self.tab_session(l));
        right.or(left).map(|s| s.id)
    }

    /// Moves off the attached `id`: to `next`, or home.
    async fn leave(&mut self, id: SessionId, next: Option<SessionId>) {
        if let Some(next) = next {
            self.attach(next).await;
        }
        // No tab beside it, or that one is this TUI's own session.
        if self.view.as_ref().is_some_and(|v| v.id == id) {
            self.detach().await;
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

    /// Steps a setting, keeps it, and lays the screen out again if the tabs moved.
    fn change(&mut self, field: Field, by: isize) {
        let before = self.settings;
        self.settings.cycle(field, by);
        self.settings.save_field(field, &self.settings_file);
        if (before.tabs, before.tab_side) != (self.settings.tabs, self.settings.tab_side) {
            self.relayout();
        }
        self.status = format!(
            "{}: {}",
            field.label().to_lowercase(),
            self.settings.value(field)
        );
    }

    /// Keys while the settings panel is open: ↑/↓ pick a setting, ←/→ change it,
    /// Esc (or `,` or `q`) closes. Anything else, mouse reports too, is ignored.
    fn settings_keys(&mut self, mut keys: &[u8]) {
        while let Some(cursor) = self.settings_cursor
            && !keys.is_empty()
        {
            let key = next_key(keys);
            let whole = key.len() == keys.len();
            keys = &keys[key.len()..];
            // Not keys: a legacy mouse report's three bytes, and pasted text.
            if key == b"\x1b[M" {
                keys = &keys[keys.len().min(3)..];
                continue;
            }
            if key == b"\x1b[200~" {
                let end = keys.windows(6).position(|w| w == b"\x1b[201~");
                keys = &keys[end.map_or(keys.len(), |i| i + 6)..];
                continue;
            }
            let last = Field::ALL.len() - 1;
            match key {
                b"k" | b"\x1b[A" | b"\x1bOA" => {
                    self.settings_cursor = Some(cursor.saturating_sub(1))
                }
                b"j" | b"\x1b[B" | b"\x1bOB" => self.settings_cursor = Some((cursor + 1).min(last)),
                b"h" | b"\x1b[D" | b"\x1bOD" => self.change(Field::ALL[cursor], -1),
                b"l" | b"\x1b[C" | b"\x1bOC" | b"\r" | b" " => self.change(Field::ALL[cursor], 1),
                b"\x1b" if whole => self.settings_cursor = None,
                b"," | b"q" | [0x03] => self.settings_cursor = None,
                _ => {}
            }
        }
    }

    async fn on_mouse(&mut self, m: Mouse) {
        if self.menu.is_some() {
            return self.menu_click(m).await;
        }
        let tabs = self.tabs_area();
        let Some(view) = &self.view else { return };
        if let Some(tabs) = tabs {
            // A drag that strays over the strip still selects (or moves a divider).
            let dragging =
                self.tab_drag.is_some() && matches!(m.kind, MouseKind::Drag | MouseKind::Release);
            let busy = view.selecting || self.mouse_owner.is_some() || self.divider_drag.is_some();
            if (tabs.contains(Position::new(m.x, m.y)) || dragging) && !busy {
                return self.tab_click(tabs, m).await;
            }
        }
        // Below the strip: a pane's, a divider's, or the pane menu's.
        let Some(m) = self.pane_mouse(m).await else {
            return;
        };
        let Some(view) = &mut self.view else { return };
        let bottom = view.rows.len().saturating_sub(1) as u16;
        let right = view.cols.saturating_sub(1);
        let at = (m.x.min(right), m.y.min(bottom));
        match m.kind {
            MouseKind::WheelUp | MouseKind::WheelDown => {
                let id = view.id;
                self.wheel(id, m.kind).await;
            }
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
            MouseKind::RightPress | MouseKind::Other => {}
        }
    }

    /// The wheel over pane `id`, whose program doesn't want the mouse: its
    /// scrollback, or arrow keys for a full-screen program.
    async fn wheel(&mut self, id: SessionId, kind: MouseKind) {
        let Some(pane) = self.pane_mut(id) else {
            return;
        };
        let up = kind == MouseKind::WheelUp;
        if pane.modes.alt_screen {
            // Full-screen programs have no scrollback; like other terminals, the
            // wheel becomes arrow keys (mode 1007).
            if pane.modes.alt_scroll {
                let key: &[u8] = match (up, pane.modes.app_cursor) {
                    (true, true) => b"\x1bOA",
                    (true, false) => b"\x1b[A",
                    (false, true) => b"\x1bOB",
                    (false, false) => b"\x1b[B",
                };
                let _ = self.client.input(id, key.repeat(WHEEL_LINES as usize));
            }
            return;
        }
        let delta = if up { -WHEEL_LINES } else { WHEEL_LINES };
        self.scroll_pane(id, delta).await;
    }

    /// Scrolls `delta` lines (negative is up, into history). Scrolling down past the
    /// live screen returns to it.
    async fn scroll_by(&mut self, delta: i64) {
        if let Some(id) = self.view.as_ref().map(|v| v.id) {
            self.scroll_pane(id, delta).await;
        }
    }

    /// `scroll_by` for any pane on screen.
    async fn scroll_pane(&mut self, id: SessionId, delta: i64) {
        let Some(view) = self.pane_mut(id) else {
            return;
        };
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
        let Some(view) = self.pane_mut(id) else {
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
            HomeKey::Sound => self.change(Field::Sound, 1),
            HomeKey::Theme => self.change(Field::Theme, 1),
            HomeKey::Settings => self.settings_cursor = Some(0),
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
            HomeKey::Rename => {
                if let Some(id) = selected {
                    let name = self
                        .sessions
                        .iter()
                        .find(|s| s.id == id)
                        .map(|s| s.name.clone());
                    self.renaming = Some((id, name.unwrap_or_default()));
                }
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
            HomeKey::NewShell => self.new_shell(None).await,
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
            let viewing = self.pane_ids();
            self.pinger.on_queue(&items, &viewing, Instant::now());
            let anchor = self.anchor();
            self.queue = items;
            self.restore(anchor);
            return Ok(true);
        }
        let session = match &msg {
            ServerMsg::Screen { session, .. }
            | ServerMsg::Graphics { session, .. }
            | ServerMsg::Clipboard { session, .. }
            | ServerMsg::Exited { session, .. } => *session,
            _ => return Ok(false),
        };
        // Each pane sits at its place below the tab strip.
        let origin = matches!(msg, ServerMsg::Graphics { .. })
            .then(|| self.main_area())
            .flatten()
            .map(|main| self.pane_layout(main).0)
            .and_then(|panes| panes.into_iter().find(|p| p.0 == session))
            .map_or((0, 0), |(_, r)| (r.x, r.y));
        let focused = self.view.as_ref().is_some_and(|v| v.id == session);
        let Some(view) = self.pane_mut(session) else {
            return Ok(false);
        };
        let mut mouse_changed = false;
        match msg {
            ServerMsg::Screen { update, .. } => {
                let before = (view.modes, view.shape);
                view.apply(update);
                if focused && before != (view.modes, view.shape) {
                    write_modes(view.modes, Some(view.shape))?;
                }
                mouse_changed = mouse::modes(before.0) != mouse::modes(view.modes);
            }
            ServerMsg::Graphics { x, y, data, .. } => {
                // At the cell the program's cursor was on, then the cursor back where
                // ratatui left it. Unicode placeholders (yazi, `kitten icat`) are
                // ordinary cells on the screen; this only delivers the image data.
                let mut out = std::io::stdout();
                let (col, row) = (x + 1 + origin.0, y + 1 + origin.1);
                write!(out, "\x1b7\x1b[{row};{col}H{data}\x1b8")?;
                out.flush()?;
            }
            ServerMsg::Clipboard { text, .. } => {
                let n = text.chars().count();
                let name = view.name.clone();
                let text = match mouse::copy_to_clipboard(&text) {
                    Ok(()) => format!("{name} copied {n} chars"),
                    Err(e) => format!("copy failed: {e}"),
                };
                self.say(text);
            }
            ServerMsg::Exited { code, .. } => {
                view.exited = Some(code);
                // A program that died with the mouse on: take it back, so a wheel or
                // a click scrolls or selects its last screen instead.
                mouse_changed = view.modes.wants_mouse();
                view.modes.mouse_click = false;
                view.modes.mouse_drag = false;
                view.modes.mouse_motion = false;
            }
            _ => {}
        }
        // Rewriting the mouse modes mid-drag can lose the drag, so only on change.
        if mouse_changed {
            self.update_capture();
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

    fn draw(&mut self, terminal: &mut Term) -> Result<()> {
        terminal.draw(|frame| {
            let [body, bar] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
            match &self.view {
                Some(view) => {
                    let (main, tabs) = self.split_body(body);
                    let cursor = self.tab_cursor.is_none()
                        && self.renaming.is_none()
                        && self.menu.is_none()
                        && self.settings_cursor.is_none();
                    self.draw_panes(frame, main, cursor);
                    if let Some(tabs) = tabs {
                        self.draw_tabs(frame, tabs);
                    }
                    if let Some(menu) = &self.menu {
                        self.draw_menu(frame, menu);
                    }
                    let mode = if self.renaming.is_some() {
                        BarMode::Renaming
                    } else if self.menu.is_some() {
                        BarMode::Menu
                    } else if self.tab_cursor.is_some() {
                        BarMode::Tabs
                    } else {
                        BarMode::Session
                    };
                    let mut line = attached_bar(view, mode, self.settings.theme);
                    if let Some(toast) = self.toast() {
                        line.spans.extend(toast_spans(toast, self.settings.theme));
                    }
                    frame.render_widget(
                        Paragraph::new(line)
                            .style(style::Style::new().bg(self.settings.theme.panel)),
                        bar,
                    );
                }
                None => self.draw_home(frame, body, bar),
            }
            if let Some(cursor) = self.settings_cursor {
                self.draw_settings(frame, cursor);
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
        let viewing = self.pane_ids();
        let Some(ping) = self.pinger.due(&viewing, Instant::now()) else {
            return;
        };
        if ping.sound && self.settings.sound {
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
        let t = self.settings.theme;
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

        let mut footer = match &self.renaming {
            Some((id, typed)) => rename_line(*id, typed, t),
            None => footer_line(&self.status, self.settings.sound, t),
        };
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
        let t = self.settings.theme;
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

/// Shorter terminals leave the session every row.
const TABS_MIN_ROWS: u16 = 12;
/// The column of tabs beside the session: the widest tab, its rule and a margin.
const SIDE_WIDTH: u16 = TAB_MAX + 2;
/// Narrower terminals leave the session every column.
const SIDE_MIN_COLS: u16 = 70;
/// A tab's width bounds, padding included.
const TAB_MIN: u16 = 10;
const TAB_MAX: u16 = 24;
/// The ` + new` card at the end of the strip.
const PLUS_WIDTH: u16 = 7;
/// The home card at the start, ` ⌂ home` over ` ● 3`, and a gap.
const HOME_WIDTH: u16 = 9;
/// Longest name the rename prompt takes.
const NAME_MAX: usize = 40;

/// A tab's menu (Rename, Close), or a pane's (the splits, Close pane).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Menu {
    kind: MenuKind,
    session: SessionId,
    /// Its top-left corner, under the tab.
    x: u16,
    y: u16,
    cursor: usize,
    /// Close was picked once on a session running an agent; the next pick closes.
    confirm: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MenuKind {
    /// Opened on a tab: the whole tab.
    Tab,
    /// Opened on a pane, with a right click on it.
    Pane,
}

const MENU_RENAME: usize = 0;
const MENU_CLOSE: usize = 1;
const MENU_WIDTH: u16 = 20;

impl Menu {
    /// The items, Close always last.
    fn items(&self) -> &'static [&'static str] {
        match self.kind {
            MenuKind::Tab => &["Rename", "Close"],
            MenuKind::Pane => &[
                "Split right",
                "Split down",
                "Split left",
                "Split up",
                "Close pane",
            ],
        }
    }

    /// The items inside a border.
    fn rect(&self) -> Rect {
        Rect::new(self.x, self.y, MENU_WIDTH, self.items().len() as u16 + 2)
    }
}

/// What a click on the tab strip landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TabHit {
    Home,
    Session(SessionId),
    New,
}

/// Where each visible tab is drawn, and where the marks for tabs left off either
/// end go (before: left or above; after: right or below).
struct TabLayout {
    home: Rect,
    tabs: Vec<(usize, Rect)>,
    plus: Rect,
    more_before: Option<Rect>,
    more_after: Option<Rect>,
}

impl App {
    /// The attached session's area and, when shown, the tab strip's, within `body`.
    fn split_body(&self, body: Rect) -> (Rect, Option<Rect>) {
        let side = |at| Layout::horizontal(at).areas::<2>(body);
        match self.settings.tab_side {
            TabSide::Top if body.height >= TABS_MIN_ROWS => {
                let rows = self.settings.tabs.rows();
                let [tabs, main] =
                    Layout::vertical([Constraint::Length(rows), Constraint::Fill(1)]).areas(body);
                (main, Some(tabs))
            }
            TabSide::Left if body.width >= SIDE_MIN_COLS && body.height >= TABS_MIN_ROWS => {
                let [tabs, main] = side([Constraint::Length(SIDE_WIDTH), Constraint::Fill(1)]);
                (main, Some(tabs))
            }
            TabSide::Right if body.width >= SIDE_MIN_COLS && body.height >= TABS_MIN_ROWS => {
                let [main, tabs] = side([Constraint::Fill(1), Constraint::Length(SIDE_WIDTH)]);
                (main, Some(tabs))
            }
            _ => (body, None),
        }
    }

    /// The attached session's area and, when shown, the tab strip's, on screen now.
    fn body_areas(&self) -> Option<(Rect, Option<Rect>)> {
        let full = self.screen().ok()?;
        Some(self.split_body(Rect::new(0, 0, full.cols, full.rows)))
    }

    /// Where the tab strip is on screen now, if shown.
    fn tabs_area(&self) -> Option<Rect> {
        self.body_areas()?.1
    }

    /// The terminal minus the bar and, when shown, the tab strip.
    fn session_size(&self) -> Result<Size> {
        let full = self.screen()?;
        let (main, _) = self.split_body(Rect::new(0, 0, full.cols, full.rows));
        Ok(Size {
            cols: main.width.max(1),
            rows: main.height.max(1),
        })
    }

    /// The terminal minus the bar.
    fn screen(&self) -> Result<Size> {
        self.fixed_size.map_or_else(session_size, Ok)
    }

    /// Tells the attached session its size, after the window changed.
    fn relayout(&mut self) {
        self.resize_panes();
    }

    /// Tabs keep the order sessions were made in, so each stays where you left it.
    /// When they don't all fit, the strip slides to keep the cursor (else the
    /// attached tab) in view.
    fn tab_layout(&self, strip: Rect) -> TabLayout {
        if self.settings.tab_side != TabSide::Top {
            return self.column_layout(strip);
        }
        // Home first, where the queue lives; the tabs after it.
        let home = Rect::new(strip.x, strip.y, HOME_WIDTH.min(strip.width), strip.height);
        let area = Rect::new(
            home.right(),
            strip.y,
            strip.width.saturating_sub(home.width),
            strip.height,
        );
        let widths: Vec<u16> = self
            .tabs()
            .iter()
            .map(|tab| tab_width(&self.sessions[self.face(tab)], tab.len()))
            .collect();
        let focus = self
            .tab_cursor
            .filter(|&i| i < widths.len())
            .or_else(|| self.attached_index())
            .unwrap_or(0);
        // One column between tabs; the arrows take one each when shown.
        let fits = |start: usize, end: usize| {
            let arrows = u16::from(start > 0) + u16::from(end < widths.len());
            let used: u16 = widths[start..end].iter().map(|w| w + 1).sum();
            used + arrows + PLUS_WIDTH <= area.width
        };
        let (start, end) = window(widths.len(), focus, fits);
        let mut x = area.x + u16::from(start > 0);
        let mut tabs = Vec::new();
        for (i, &w) in widths.iter().enumerate().take(end).skip(start) {
            let w = w.min(area.right().saturating_sub(x + PLUS_WIDTH));
            tabs.push((i, Rect::new(x, area.y, w, area.height)));
            x += w + 1;
        }
        let more_right = end < widths.len();
        let plus_x = (x + u16::from(more_right)).min(area.right().saturating_sub(PLUS_WIDTH));
        TabLayout {
            home,
            tabs,
            plus: Rect::new(plus_x, area.y, PLUS_WIDTH, area.height),
            more_before: (start > 0).then(|| Rect::new(home.right(), area.y, 1, 1)),
            more_after: more_right.then(|| Rect::new(plus_x.saturating_sub(1), area.y, 1, 1)),
        }
    }

    /// Tabs in a column beside the session: home on top, then a tab of two rows per
    /// session with a row between, then "+". The column's edge toward the session
    /// is left for underlined tabs' rule.
    fn column_layout(&self, strip: Rect) -> TabLayout {
        let x = match self.settings.tab_side {
            TabSide::Right => strip.x + 1,
            _ => strip.x,
        };
        let width = strip.width.saturating_sub(1);
        let row = |y: u16, height: u16| Rect::new(x, y, width, height).intersection(strip);
        let home = row(strip.y, 2);
        let first = home.bottom() + 1;
        let n = self.tabs().len();
        let focus = self
            .tab_cursor
            .filter(|&i| i < n)
            .or_else(|| self.attached_index())
            .unwrap_or(0);
        // Each tab and the row after it; the marks and "+" a row each.
        let room = strip.bottom().saturating_sub(first);
        let fits = |start: usize, end: usize| {
            let marks = u16::from(start > 0) + u16::from(end < n);
            (end - start) as u16 * 3 + marks < room
        };
        let (start, end) = window(n, focus, fits);
        let mut y = first;
        let more_before = (start > 0).then(|| {
            y += 1;
            row(first, 1)
        });
        let mut tabs = Vec::new();
        for i in start..end {
            tabs.push((i, row(y, 2)));
            y += 3;
        }
        let more_after = (end < n).then(|| {
            y += 1;
            row(y - 1, 1)
        });
        TabLayout {
            home,
            tabs,
            plus: row(y.min(strip.bottom().saturating_sub(1)), 1),
            more_before,
            more_after,
        }
    }

    fn tab_hit(&self, area: Rect, x: u16, y: u16) -> Option<TabHit> {
        let layout = self.tab_layout(area);
        let at = Position::new(x, y);
        if layout.home.contains(at) {
            return Some(TabHit::Home);
        }
        if layout.plus.contains(at) {
            return Some(TabHit::New);
        }
        let (i, _) = layout.tabs.iter().find(|(_, r)| r.contains(at))?;
        Some(TabHit::Session(self.tab_session(*i)?.id))
    }

    /// One tab per session: its name, then what runs in it and its state.
    fn draw_tabs(&self, frame: &mut ratatui::Frame, area: Rect) {
        match self.settings.tabs {
            TabStyle::Cards => self.draw_cards(frame, area),
            TabStyle::Underline | TabStyle::Folder => self.draw_underlined(frame, area),
        }
    }

    /// Marks for tabs that did not fit: before the first shown and after the last.
    fn draw_more(&self, frame: &mut ratatui::Frame, layout: &TabLayout) {
        let (before, after) = match self.settings.tab_side {
            TabSide::Top => ("‹", "›"),
            _ => ("  ▲ more", "  ▼ more"),
        };
        for (rect, mark) in [(layout.more_before, before), (layout.more_after, after)] {
            if let Some(rect) = rect {
                frame.render_widget(Paragraph::new(mark.fg(self.settings.theme.muted)), rect);
            }
        }
    }

    /// Down a column, a card reaches half a row into the gap under it, so cards
    /// are set apart by a thin line of the strip rather than a whole row.
    fn card_edge(&self, frame: &mut ratatui::Frame, area: Rect, card: Rect, color: style::Color) {
        let y = card.bottom();
        if self.settings.tab_side == TabSide::Top || y >= area.bottom() {
            return;
        }
        let buf = frame.buffer_mut();
        for x in card.left()..card.right() {
            buf[(x, y)]
                .set_symbol("▀")
                .set_fg(color)
                .set_bg(self.settings.theme.strip);
        }
    }

    /// Tabs as cards on a darker strip, the attached one marked with a bar.
    fn draw_cards(&self, frame: &mut ratatui::Frame, area: Rect) {
        let t = self.settings.theme;
        let spin = (now_ms() / SPIN_EVERY.as_millis() as u64) as usize;
        let buf = frame.buffer_mut();
        buf.set_style(area, style::Style::new().bg(t.strip));
        let layout = self.tab_layout(area);
        // Every clickable thing is a card on the darker strip; the cursor's is lit.
        let lit = style::Style::new().bg(t.accent).fg(t.bg);
        let attached = self.attached_index();
        let tabs = self.tabs();
        for &(i, rect) in &layout.tabs {
            let tab = &tabs[i];
            let s = &self.sessions[self.face(tab)];
            let here = Some(i) == attached;
            let cursor = self.tab_cursor == Some(i);
            let (state, label) = match s.exited {
                None => (s.status.state, s.status.state.label()),
                Some(_) => (AgentState::Exited, "exited"),
            };
            let color = t.state(state);
            let queued = self.queue.iter().any(|q| q.session == s.id);
            // A split tab says how many panes it has.
            let panes = if tab.len() > 1 {
                format!("⊞{} ", tab.len())
            } else {
                String::new()
            };
            let bg = if here { t.selection } else { t.card };
            let width = rect.width as usize;
            let mark = if here { "▌" } else { " " };
            let number = if i < 9 {
                format!("{} ", i + 1)
            } else {
                String::new()
            };
            let top = match &self.renaming {
                Some((id, typed)) if *id == s.id => Line::from(vec![
                    mark.fg(t.accent).bold(),
                    "✎ ".fg(t.accent),
                    fit(typed, width.saturating_sub(4)).fg(t.fg),
                    "▏".fg(t.accent),
                ]),
                _ => {
                    let name_color = if queued { color } else { t.fg };
                    let mut name =
                        fit(&s.name, width.saturating_sub(1 + number.width())).fg(name_color);
                    if here || queued {
                        name = name.bold();
                    }
                    Line::from(vec![mark.fg(t.accent).bold(), number.fg(t.muted), name])
                }
            };
            let mut bottom = vec![mark.fg(t.accent).bold()];
            bottom.extend(detail_spans(
                &format!("{panes}{}", program(s)),
                label,
                state_icon(state, spin),
                color,
                width - 1,
                t,
            ));
            let bottom = Line::from(bottom);
            frame.render_widget(
                Paragraph::new(vec![top, bottom]).style(style::Style::new().bg(bg)),
                rect,
            );
            if cursor {
                frame.buffer_mut().set_style(rect, lit);
            }
            self.card_edge(frame, area, rect, if cursor { t.accent } else { bg });
        }
        // Home: the way back to the whole picture, with how many need you.
        let needs = if self.queue.is_empty() {
            Line::default()
        } else {
            Line::from(format!(" ● {}", self.queue.len()).fg(t.needs).bold())
        };
        // On top the home card ends a column early, like the gap after a tab.
        let home = match self.settings.tab_side {
            TabSide::Top => Rect {
                width: layout.home.width.saturating_sub(1),
                ..layout.home
            },
            _ => layout.home,
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![" ⌂ ".fg(t.accent).bold(), "home".fg(t.fg).bold()]),
                needs,
            ])
            .style(style::Style::new().bg(t.card)),
            home,
        );
        self.card_edge(frame, area, home, t.card);
        self.draw_more(frame, &layout);
        let plus = Paragraph::new(Line::from(vec![" + ".fg(t.accent).bold(), "new".fg(t.fg)]))
            .style(style::Style::new().bg(t.card));
        frame.render_widget(plus, layout.plus);
        if self.tab_cursor == Some(tabs.len()) {
            frame.buffer_mut().set_style(layout.plus, lit);
        }
    }

    /// Flat tabs on the page over a thin rule; the attached tab is bright and
    /// underlined in the accent, the rest muted. The tab cursor lifts its tab and
    /// underlines it in the second accent. Folder tabs on top mark the attached tab
    /// with a bar over it instead, and break the rule under it.
    fn draw_underlined(&self, frame: &mut ratatui::Frame, area: Rect) {
        let t = self.settings.theme;
        let spin = (now_ms() / SPIN_EVERY.as_millis() as u64) as usize;
        let layout = self.tab_layout(area);
        // The session's own background, so the tabs sit on the same page.
        let page = style::Style::new().bg(style::Color::Reset);
        // The rule runs along the strip's edge toward the session: under a strip on
        // top, beside a column. A tab is underlined where it meets the rule.
        let folder = self.settings.tabs == TabStyle::Folder;
        let mut bar = None;
        let (rule, text) = match self.settings.tab_side {
            TabSide::Top if folder => {
                let [top, text, rule] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Fill(1),
                    Constraint::Length(1),
                ])
                .areas(area);
                bar = Some(top);
                (rule, text)
            }
            TabSide::Top => {
                let [text, rule] =
                    Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
                (rule, text)
            }
            TabSide::Left => {
                let [text, rule] =
                    Layout::horizontal([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
                (rule, text)
            }
            TabSide::Right => {
                let [rule, text] =
                    Layout::horizontal([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
                (rule, text)
            }
        };
        let (thin, thick) = if rule.height == 1 {
            ("─", "━")
        } else {
            ("│", "┃")
        };
        let buf = frame.buffer_mut();
        buf.set_style(area, page);
        for at in rule.positions() {
            buf[at].set_symbol(thin).set_fg(t.border);
        }
        let underline = |frame: &mut ratatui::Frame, rect: Rect, color| {
            let buf = frame.buffer_mut();
            if let Some(bar) = bar {
                // A bar along the tab's top, and the rule open under it, cornered.
                for x in rect.left()..rect.right() {
                    buf[(x, bar.y)].set_symbol("▂").set_fg(color);
                    buf[(x, rule.y)].set_symbol(" ");
                }
                if rect.x > area.x {
                    buf[(rect.x - 1, rule.y)].set_symbol("┘").set_fg(t.border);
                }
                if rect.right() < area.right() {
                    buf[(rect.right(), rule.y)].set_symbol("└").set_fg(t.border);
                }
                return;
            }
            for at in rule.positions() {
                let along = if rule.height == 1 {
                    (rect.left()..rect.right()).contains(&at.x)
                } else {
                    (rect.top()..rect.bottom()).contains(&at.y)
                };
                if along {
                    buf[at].set_symbol(thick).set_fg(color);
                }
            }
        };
        let lift = |frame: &mut ratatui::Frame, rect: Rect| {
            let rect = rect.intersection(text);
            frame
                .buffer_mut()
                .set_style(rect, style::Style::new().bg(t.selection));
        };
        let attached = self.attached_index();
        let tabs = self.tabs();
        for &(i, rect) in &layout.tabs {
            let tab = &tabs[i];
            let s = &self.sessions[self.face(tab)];
            let here = Some(i) == attached;
            // A split tab says how many panes it has.
            let panes = if tab.len() > 1 {
                format!("⊞{} ", tab.len())
            } else {
                String::new()
            };
            let (state, label) = match s.exited {
                None => (s.status.state, s.status.state.label()),
                Some(_) => (AgentState::Exited, "exited"),
            };
            let color = t.state(state);
            let queued = self.queue.iter().any(|q| q.session == s.id);
            let width = rect.width as usize;
            let number = if i < 9 {
                format!("{} ", i + 1)
            } else {
                String::new()
            };
            let top = match &self.renaming {
                Some((id, typed)) if *id == s.id => Line::from(vec![
                    " ✎ ".fg(t.accent),
                    fit(typed, width.saturating_sub(4)).fg(t.fg),
                    "▏".fg(t.accent),
                ]),
                _ => {
                    // Bright when attached, the state's color when it needs you.
                    let name = fit(&s.name, width.saturating_sub(1 + number.width()));
                    let name = match (here, queued) {
                        (_, true) => name.fg(color).bold(),
                        (true, false) => name.fg(t.fg).bold(),
                        (false, false) => name.fg(t.muted),
                    };
                    let number = if here {
                        number.fg(t.accent)
                    } else {
                        number.fg(t.border)
                    };
                    Line::from(vec![" ".into(), number, name])
                }
            };
            let mut bottom = vec![Span::raw(" ")];
            bottom.extend(detail_spans(
                &format!("{panes}{}", program(s)),
                label,
                state_icon(state, spin),
                color,
                width - 1,
                t,
            ));
            frame.render_widget(
                Paragraph::new(vec![top, Line::from(bottom)]),
                rect.intersection(text),
            );
            if self.tab_cursor == Some(i) {
                lift(frame, rect);
                underline(frame, rect, t.accent2);
            } else if here {
                underline(frame, rect, t.accent);
            }
        }
        // A faint rule between tabs meets the edge's, so each tab reads as its own:
        // a column down the gap after each tab on top, a row across it in a column.
        if rule.height == 1 {
            let buf = frame.buffer_mut();
            let gaps = std::iter::once(layout.home.right().saturating_sub(1))
                .chain(layout.tabs.iter().map(|&(_, r)| r.right()));
            for x in gaps {
                let mark = layout.more_after.is_some_and(|m| m.x == x);
                if x >= layout.plus.x || mark {
                    continue;
                }
                for y in text.top()..text.bottom() {
                    buf[(x, y)].set_symbol("│").set_fg(t.border);
                }
                // Where an open tab's corner already meets it, the corner stays.
                let at = &mut buf[(x, rule.y)];
                if !matches!(at.symbol(), "┘" | "└") {
                    at.set_symbol("┴").set_fg(t.border);
                }
            }
        }
        if rule.width == 1 {
            let join = if rule.x > text.x { "┤" } else { "├" };
            let buf = frame.buffer_mut();
            let below = std::iter::once(layout.home).chain(layout.tabs.iter().map(|&(_, r)| r));
            for r in below {
                let y = r.bottom();
                if y >= area.bottom() {
                    continue;
                }
                for x in r.left()..r.right() {
                    buf[(x, y)].set_symbol("─").set_fg(t.border);
                }
                buf[(rule.x, y)].set_symbol(join).set_fg(t.border);
            }
        }
        // Home: the way back to the whole picture, with how many need you.
        let needs = if self.queue.is_empty() {
            Line::default()
        } else {
            Line::from(format!(" ● {}", self.queue.len()).fg(t.needs).bold())
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(vec![" ⌂ ".fg(t.accent), "home".fg(t.muted)]),
                needs,
            ]),
            layout.home.intersection(text),
        );
        self.draw_more(frame, &layout);
        frame.render_widget(
            Paragraph::new(Line::from(vec![" + ".fg(t.accent), "new".fg(t.muted)])),
            layout.plus.intersection(text),
        );
        if self.tab_cursor == Some(tabs.len()) {
            lift(frame, layout.plus);
            underline(frame, layout.plus, t.accent2);
        }
    }
}

impl App {
    fn draw_menu(&self, frame: &mut ratatui::Frame, menu: &Menu) {
        let t = self.settings.theme;
        let rect = menu.rect().intersection(frame.area());
        let program = self
            .sessions
            .iter()
            .find(|s| s.id == menu.session)
            .map(program)
            .unwrap_or_default();
        let labels = menu.items();
        let last = labels.len() - 1;
        let close = if menu.confirm {
            format!("Close {program}? ↩")
        } else {
            labels[last].into()
        };
        let items = labels[..last]
            .iter()
            .map(|l| (l.to_string(), t.fg))
            .chain([(close, t.blocked)])
            .enumerate()
            .map(|(i, (label, color))| {
                let line = Line::from(format!(" {label}").fg(color));
                if i == menu.cursor {
                    line.style(style::Style::new().bg(t.selection)).bold()
                } else {
                    line
                }
            })
            .collect::<Vec<_>>();
        frame.render_widget(ratatui::widgets::Clear, rect);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style::Style::new().fg(t.accent))
            .style(style::Style::new().bg(t.panel).fg(t.fg));
        frame.render_widget(Paragraph::new(items).block(block), rect);
    }

    /// The settings panel, in the middle of the screen: one row per setting with
    /// its value between arrows, and where the settings are kept.
    fn draw_settings(&self, frame: &mut ratatui::Frame, cursor: usize) {
        let t = self.settings.theme;
        let file = &self.settings_file;
        let file = match std::env::var_os("HOME") {
            Some(home) => match file.strip_prefix(&home) {
                Ok(rest) => format!("~/{}", rest.display()),
                Err(_) => file.display().to_string(),
            },
            None => file.display().to_string(),
        };
        let width = (file.width() as u16 + 6).max(40);
        let height = Field::ALL.len() as u16 + 5;
        let area = frame.area();
        let rect = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        )
        .intersection(area);
        let mut lines = vec![Line::default()];
        for (i, &field) in Field::ALL.iter().enumerate() {
            let here = i == cursor;
            let arrow = |a: &'static str| if here { a.fg(t.accent) } else { "  ".into() };
            let value = self.settings.value(field);
            let mut line = Line::from(vec![
                format!("  {:<12}", field.label()).fg(if here { t.fg } else { t.muted }),
                arrow("‹ "),
                if here {
                    value.fg(t.accent).bold()
                } else {
                    value.fg(t.fg)
                },
                arrow(" ›"),
            ]);
            // Across the panel, so the cursor's row is lit edge to edge.
            let pad = (width as usize).saturating_sub(2 + line.width());
            line.spans.push(" ".repeat(pad).into());
            lines.push(if here {
                line.style(style::Style::new().bg(t.selection))
            } else {
                line
            });
        }
        lines.push(Line::default());
        lines.push(Line::from(format!("  {file}").fg(t.muted)));
        let hints = Line::from(vec![
            " ↑↓ ".fg(t.accent).bold(),
            "choose ".fg(t.muted),
            " ←→ ".fg(t.accent).bold(),
            "change ".fg(t.muted),
            " esc ".fg(t.accent).bold(),
            "done ".fg(t.muted),
        ]);
        frame.render_widget(ratatui::widgets::Clear, rect);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style::Style::new().fg(t.accent))
            .title(" Settings ".fg(t.fg).bold())
            .title_bottom(hints.centered())
            .style(style::Style::new().bg(t.panel).fg(t.fg));
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

/// Whether an agent (not a shell or another program) runs in the session.
fn is_agent(s: &SessionInfo) -> bool {
    !matches!(s.status.agent.as_str(), "generic" | "unknown" | "")
}

/// The run of `n` tabs to show: from the first, sliding just far enough that
/// `focus` is in it, as many as `fits(start, end)` allows (at least one).
fn window(n: usize, focus: usize, fits: impl Fn(usize, usize) -> bool) -> (usize, usize) {
    if n == 0 {
        return (0, 0);
    }
    let mut start = 0;
    loop {
        let mut end = start;
        while end < n && fits(start, end + 1) {
            end += 1;
        }
        if focus < end || start >= focus {
            return (start, end.max(start + 1).min(n));
        }
        start += 1;
    }
}

/// A tab wide enough for its name and its program line, within bounds.
fn tab_width(s: &SessionInfo, panes: usize) -> u16 {
    // Mark and number before the name; mark, " · ", icon and state after the program,
    // and `⊞2 ` before it on a split tab.
    let name = 3 + s.name.width();
    let split = if panes > 1 {
        2 + panes.to_string().len()
    } else {
        0
    };
    let detail = 1 + split + program(s).width() + 5 + s.status.state.label().width();
    (name.max(detail) as u16 + 1).clamp(TAB_MIN, TAB_MAX)
}

/// A tab's second line, `claude · ⠋ working` in `width`: the program muted, the
/// icon and state in the state's color.
fn detail_spans(
    program: &str,
    label: &str,
    icon: &str,
    color: style::Color,
    width: usize,
    t: &Theme,
) -> Vec<Span<'static>> {
    let detail = fit(&format!("{program} · {icon} {label}"), width);
    let split = program.len().min(detail.len());
    vec![
        detail[..split].to_string().fg(t.muted),
        detail[split..].to_string().fg(color),
    ]
}

/// What runs in a session: its agent, else its program (`fish`).
fn program(s: &SessionInfo) -> String {
    match s.status.agent.as_str() {
        _ if !is_agent(s) => s
            .command
            .first()
            .and_then(|p| std::path::Path::new(p).file_name())
            .map_or_else(|| "?".into(), |n| n.to_string_lossy().into_owned()),
        agent => agent.to_string(),
    }
}

/// `text` cut to `max` columns, with `…` when cut.
fn fit(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        if out.width() + c.width().unwrap_or(0) + 1 > max {
            break;
        }
        out.push(c);
    }
    if max > 0 {
        out.push('…');
    }
    out
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
    // What runs there, then what it last said; never the shell's window title.
    let mut detail = vec![program(s).fg(t.muted)];
    if let Some(summary) = &s.status.summary {
        detail.push("  ".into());
        detail.push(summary.clone().fg(t.fg));
    }
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

/// The home screen's rename prompt.
fn rename_line(id: SessionId, typed: &str, t: &Theme) -> Line<'static> {
    let mut spans = vec![
        format!(" rename #{id} ").fg(t.bg).bg(t.accent).bold(),
        format!(" {typed}").fg(t.fg),
        "▏".fg(t.accent),
    ];
    for (key, what) in [("↩", "save"), ("esc", "cancel"), ("blank", "folder name")] {
        spans.push(" ".into());
        spans.push(format!(" {key} ").fg(t.bg).bg(t.accent).bold());
        spans.push(format!(" {what}").fg(t.muted));
    }
    Line::from(spans).style(style::Style::new().bg(t.panel))
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
        ("R", "rename"),
        ("t", "theme"),
        (",", "settings"),
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
/// What the attached bar's key hints are for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarMode {
    Session,
    Tabs,
    Menu,
    Renaming,
}

fn attached_bar(view: &Attached, mode: BarMode, t: &Theme) -> Line<'static> {
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
    }
    if let Some((notice, at)) = &view.notice
        && at.elapsed() < NOTICE_FOR
    {
        spans.push(format!(" {notice} ").fg(t.done).bold());
    }
    let keys: &[(&str, &str)] = match mode {
        BarMode::Session => &[("^\\", "tabs"), ("^]", "home")],
        BarMode::Tabs => &[
            ("h/l", "move"),
            ("H/L", "reorder"),
            ("↩", "switch"),
            ("1-9", "jump"),
            ("0", "home"),
            ("⇥", "next ●"),
            ("n", "new"),
            ("s/v", "split"),
            ("o", "pane"),
            ("r", "rename"),
            ("x", "close"),
            (",", "settings"),
            ("esc", "back"),
        ],
        BarMode::Menu => &[("↑/↓", "move"), ("↩", "pick"), ("esc", "close menu")],
        BarMode::Renaming => &[("↩", "save"), ("esc", "cancel"), ("blank", "folder name")],
    };
    for (key, what) in keys {
        spans.push(" ".into());
        spans.push(format!(" {key} ").fg(t.bg).bg(t.accent).bold());
        spans.push(format!(" {what}").fg(t.muted));
    }
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
        // Not seconds: a label that changes every second keeps the TUI writing.
        0..60 => "<1m".into(),
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

/// Draws a session's rows in `area`: with `theme`, its default and ANSI colors are
/// the theme's (blank cells too), else the outer terminal's.
fn render_rows(rows: &[Row], area: Rect, buf: &mut ratatui::buffer::Buffer, theme: Option<&Theme>) {
    if let Some(t) = theme {
        buf.set_style(area, style::Style::new().fg(t.fg).bg(t.bg));
    }
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
                to_style(span.style, theme),
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

fn to_style(s: Style, theme: Option<&Theme>) -> style::Style {
    let mut out = style::Style::new()
        .fg(to_color(s.fg, theme.map(|t| t.fg), theme))
        .bg(to_color(s.bg, theme.map(|t| t.bg), theme));
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

/// `default` stands in for the program's default color, and `theme` supplies the
/// 16 ANSI colors; without them the outer terminal's are used.
fn to_color(c: Color, default: Option<style::Color>, theme: Option<&Theme>) -> style::Color {
    match c {
        Color::Default => default.unwrap_or(style::Color::Reset),
        Color::Indexed(i) => match theme {
            Some(t) if i < 16 => t.ansi[i as usize],
            _ => style::Color::Indexed(i),
        },
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

/// The mouse off: on the home screen it is nobody's. While attached it is always
/// Valkyrie's first (`App::update_capture`).
fn release_mouse() -> Result<()> {
    let mut out = std::io::stdout();
    write!(out, "{}", mouse::CAPTURE_OFF)?;
    out.flush()?;
    Ok(())
}

/// Back to what a plain shell expects: no mirrored input modes, the user's cursor shape.
fn reset_terminal_modes() -> Result<()> {
    write_modes(Modes::default(), None)?;
    release_mouse()?;
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
    /// Open the settings panel.
    Settings,
    /// Ping sounds on/off.
    Sound,
    Rename,
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
            b',' => keys.push(HomeKey::Settings),
            b'm' => keys.push(HomeKey::Sound),
            b'R' => keys.push(HomeKey::Rename),
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

    /// With `VALK_SNAPSHOT_DIR` set, writes a render's cells (symbol, fg, bg, bold)
    /// as `<name>.json` there, for eyeballing a theme as an image.
    fn snapshot(buffer: &ratatui::buffer::Buffer, name: &str) {
        let Some(out) = std::env::var_os("VALK_SNAPSHOT_DIR") else {
            return;
        };
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
        let path = PathBuf::from(out).join(format!("{name}.json"));
        std::fs::write(path, serde_json::to_string(&rows).unwrap()).unwrap();
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
            last_input_ms: 0,
            chat: None,
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
        // Not whatever this machine's settings.toml says.
        app.settings = Settings::default();
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
        // Not whatever this machine's settings.toml says.
        app.settings = Settings::default();
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
            app.settings.theme = theme;
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
            snapshot(buffer, theme.name);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An attached session under the tab strip: the session gets the rows left
    /// over, tabs show name then program and state, clicks land on the tabs drawn,
    /// and the keys move, jump, and rename.
    #[tokio::test]
    async fn tab_strip_lists_sessions_above_the_attached_one() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-tabs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        // Not whatever this machine's settings.toml says.
        app.settings = Settings::default();
        app.sessions = (1..=3)
            .map(|id| {
                let mut s = info(id);
                s.name = format!("proj{id}");
                s.command = vec!["/usr/bin/fish".into()];
                s
            })
            .collect();
        app.sessions[1].status.agent = "claude".into();
        app.sessions[1].status.state = AgentState::Working;
        app.sessions[2].status.agent = "codex".into();
        app.sessions[2].status.state = AgentState::NeedsInput;
        app.queue = vec![queued(3)];
        let mut view = Attached::new(2, "proj2".into(), None);
        view.apply(update(true, vec![row(0, "inside session 2")]));
        view.title = Some("xdx@Thor:~/repos/proj2".into());
        app.view = Some(view);
        app.settings.tabs = TabStyle::Cards;

        let body = Rect::new(0, 0, 120, 23);
        let (main, tabs) = app.split_body(body);
        let tabs = tabs.expect("tall enough for the strip");
        assert_eq!((tabs.y, tabs.height), (0, 2));
        assert_eq!((main.y, main.width), (2, 120));
        assert!(
            app.split_body(Rect::new(0, 0, 120, 8)).1.is_none(),
            "too short"
        );

        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        let draw = |app: &App, terminal: &mut Terminal<TestBackend>| {
            terminal
                .draw(|frame| {
                    let [body, bar] =
                        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                            .areas(frame.area());
                    let (main, tabs) = app.split_body(body);
                    let view = app.view.as_ref().unwrap();
                    render_rows(view.shown(), main, frame.buffer_mut(), None);
                    app.draw_tabs(frame, tabs.unwrap());
                    frame.render_widget(
                        Paragraph::new(attached_bar(view, BarMode::Session, app.settings.theme)),
                        bar,
                    );
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect()
                })
                .collect::<Vec<String>>()
        };
        for theme in theme::THEMES {
            app.settings.theme = theme;
            draw(&app, &mut terminal);
            snapshot(terminal.backend().buffer(), &format!("tabs-{}", theme.name));
            app.tab_cursor = Some(0);
            draw(&app, &mut terminal);
            snapshot(
                terminal.backend().buffer(),
                &format!("tabs-cursor-{}", theme.name),
            );
            app.tab_cursor = None;
        }
        app.settings.theme = theme::THEMES[0];
        let lines = draw(&app, &mut terminal);
        assert!(
            lines[0].contains("1 proj1") && lines[0].contains("2 proj2"),
            "{}",
            lines[0]
        );
        assert!(lines[1].contains("fish · ○ idle"), "{}", lines[1]);
        assert!(lines[1].contains("claude · ") && lines[1].contains("working"));
        assert!(lines[1].contains("codex · ● needs input"));
        assert!(lines[0].trim_end().ends_with("+ new"), "{}", lines[0]);
        assert!(
            lines[2].starts_with("inside session 2"),
            "the session sits below"
        );
        // The attached tab is marked; the window title is gone from the bar.
        let layout = app.tab_layout(tabs);
        let attached = layout.tabs[1].1;
        assert_eq!(lines[0].chars().nth(attached.x as usize), Some('▌'));
        assert!(!lines[23].contains("xdx@Thor"), "{}", lines[23]);

        let (first, third) = (layout.tabs[0].1, layout.tabs[2].1);
        assert_eq!(app.tab_hit(tabs, first.x + 2, 1), Some(TabHit::Session(1)));
        assert_eq!(
            app.tab_hit(tabs, third.right() - 1, 0),
            Some(TabHit::Session(3))
        );
        assert_eq!(app.tab_hit(tabs, layout.plus.x + 1, 0), Some(TabHit::New));
        // Home comes first, with how many need you under it.
        assert_eq!(app.tab_hit(tabs, 1, 1), Some(TabHit::Home));
        assert!(lines[0].starts_with(" ⌂ home"), "{}", lines[0]);
        assert!(lines[1].starts_with(" ● 1"), "{}", lines[1]);
        assert!(first.x >= HOME_WIDTH);
        assert_eq!(app.tab_hit(tabs, 119, 0), None, "past the +");

        // ^\ puts the cursor on the attached tab; ←/→ stop at "+" and the first.
        app.on_keys(vec![TABS_KEY]).await;
        assert_eq!(app.tab_cursor, Some(1));
        app.on_keys(b"llll".to_vec()).await;
        assert_eq!(app.tab_cursor, Some(3));
        app.on_keys(b"\x1b[Dhhhh".to_vec()).await;
        assert_eq!(app.tab_cursor, Some(0));
        app.on_keys(b"\x1b".to_vec()).await;
        assert_eq!(app.tab_cursor, None);
        // Jumping to the tab already attached just hands the keyboard back.
        app.on_keys(vec![TABS_KEY, b'2']).await;
        assert_eq!(app.tab_cursor, None);
        assert_eq!(app.view.as_ref().map(|v| v.id), Some(2));

        // r renames the tab under the cursor; typing edits, Esc cancels.
        app.on_keys(vec![TABS_KEY, b'r']).await;
        assert_eq!(app.renaming, Some((2, "proj2".into())));
        app.on_keys(b"\x7f\x7f\x7f\x7f\x7fapi \xc3\xa9".to_vec())
            .await;
        assert_eq!(app.renaming, Some((2, "api é".into())));
        let lines = draw(&app, &mut terminal);
        assert!(lines[0].contains("✎ api é▏"), "{}", lines[0]);
        app.on_keys(b"\x15".to_vec()).await;
        assert_eq!(app.renaming, Some((2, String::new())));
        app.on_keys(b"\x1b".to_vec()).await;
        assert_eq!(app.renaming, None);
        // A right click on a tab starts the same prompt.
        let right = Mouse::at(MouseKind::RightPress, third.x + 1, 0);
        // A right click opens the tab's menu; Rename is first.
        app.tab_click(tabs, right).await;
        let menu = app.menu.expect("menu open");
        assert_eq!((menu.session, menu.cursor), (3, MENU_RENAME));
        let lines = draw_menu_lines(&app);
        assert!(lines.iter().any(|l| l.contains(" Rename")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains(" Close")));
        // Clicking Rename starts the prompt.
        let rect = menu.rect();
        app.on_mouse(Mouse::at(MouseKind::Press, rect.x + 3, rect.y + 1))
            .await;
        assert_eq!(app.menu, None);
        assert_eq!(app.renaming, Some((3, "proj3".into())));
        app.on_keys(b"\x1b".to_vec()).await;
        // Close on an agent's tab asks first; Esc keeps the session.
        app.on_keys(vec![TABS_KEY, b'l', b'x']).await;
        let menu = app.menu.expect("x opens the menu on Close");
        assert_eq!(
            (menu.session, menu.cursor, menu.confirm),
            (3, MENU_CLOSE, false)
        );
        app.on_keys(b"\r".to_vec()).await;
        assert!(app.menu.expect("still open").confirm);
        assert!(
            draw_menu_lines(&app)
                .iter()
                .any(|l| l.contains("Close codex?"))
        );
        app.on_keys(b"\x1b".to_vec()).await;
        assert_eq!(app.menu, None);
        // A click outside closes the menu without doing anything.
        app.tab_click(tabs, right).await;
        app.on_mouse(Mouse::at(MouseKind::Press, 100, 15)).await;
        assert_eq!((app.menu, app.renaming.is_none()), (None, true));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn draw_menu_lines(app: &App) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal
            .draw(|frame| app.draw_menu(frame, app.menu.as_ref().unwrap()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    /// Many sessions: the strip slides to keep the cursor's tab in view.
    /// Underlined tabs: names, then programs, then a rule that is thick under the
    /// attached tab; the session starts below the rule.
    #[tokio::test]
    async fn underlined_tabs_rule_off_the_session() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-under-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        // Not whatever this machine's settings.toml says.
        app.settings = Settings::default();
        app.settings.tabs = TabStyle::Underline;
        app.sessions = (1..=3)
            .map(|id| {
                let mut s = info(id);
                s.name = format!("proj{id}");
                s.command = vec!["/usr/bin/fish".into()];
                s
            })
            .collect();
        app.sessions[2].status.agent = "codex".into();
        app.sessions[2].status.state = AgentState::NeedsInput;
        app.queue = vec![queued(3)];
        let mut view = Attached::new(2, "proj2".into(), None);
        view.apply(update(true, vec![row(0, "inside session 2")]));
        app.view = Some(view);

        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        let mut draw = |app: &App| {
            terminal
                .draw(|frame| {
                    let (main, tabs) = app.split_body(frame.area());
                    render_rows(
                        app.view.as_ref().unwrap().shown(),
                        main,
                        frame.buffer_mut(),
                        None,
                    );
                    app.draw_tabs(frame, tabs.unwrap());
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            snapshot(&buffer, "tabs-underline");
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect()
                })
                .collect::<Vec<String>>()
        };
        let lines = draw(&app);
        assert!(lines[0].starts_with(" ⌂ home"), "{}", lines[0]);
        assert!(lines[0].contains("2 proj2"), "{}", lines[0]);
        assert!(lines[1].contains("codex · ● needs input"), "{}", lines[1]);
        assert!(lines[3].starts_with("inside session 2"), "{}", lines[3]);
        let layout = app.tab_layout(app.split_body(Rect::new(0, 0, 100, 20)).1.unwrap());
        let attached = layout.tabs[1].1;
        let rule: Vec<char> = lines[2].chars().collect();
        assert!(
            rule[attached.x as usize..attached.right() as usize]
                .iter()
                .all(|&c| c == '━')
        );
        let first = layout.tabs[0].1;
        assert_eq!(
            rule[first.x as usize], '─',
            "only the attached tab is underlined"
        );
        // A divider down the gap after each tab, meeting the rule.
        let gap = first.right() as usize;
        assert_eq!(lines[0].chars().nth(gap), Some('│'), "{}", lines[0]);
        assert_eq!(rule[gap], '┴');

        // Folder tabs: a bar over the attached tab and the rule open under it.
        app.settings.tabs = TabStyle::Folder;
        let lines = draw(&app);
        let layout = app.tab_layout(app.split_body(Rect::new(0, 0, 100, 20)).1.unwrap());
        let attached = layout.tabs[1].1;
        let (bar, rule): (Vec<char>, Vec<char>) =
            (lines[0].chars().collect(), lines[3].chars().collect());
        let span = attached.x as usize..attached.right() as usize;
        assert!(bar[span.clone()].iter().all(|&c| c == '▂'), "{}", lines[0]);
        assert!(rule[span].iter().all(|&c| c == ' '), "{}", lines[3]);
        assert_eq!(rule[attached.x as usize - 1], '┘');
        assert_eq!(rule[attached.right() as usize], '└');
        assert!(lines[1].contains("2 proj2") && lines[4].starts_with("inside session 2"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Tabs in a column on either side: the session beside it, each tab two rows
    /// with one between, the rule on the edge toward the session thick along the
    /// attached tab, and clicks finding the tab they land on.
    #[tokio::test]
    async fn tabs_stand_in_a_column_on_either_side() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-side-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        app.sessions = (1..=3)
            .map(|id| {
                let mut s = info(id);
                s.name = format!("proj{id}");
                s.command = vec!["/usr/bin/fish".into()];
                s
            })
            .collect();
        app.sessions[1].status.agent = "claude".into();
        app.sessions[1].status.state = AgentState::Working;
        app.sessions[2].status.agent = "codex".into();
        app.sessions[2].status.state = AgentState::NeedsInput;
        app.queue = vec![queued(3)];
        let mut view = Attached::new(2, "proj2".into(), None);
        view.apply(update(true, vec![row(0, "inside session 2")]));
        app.view = Some(view);

        let body = Rect::new(0, 0, 100, 20);
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        for (side, style) in [
            (TabSide::Left, TabStyle::Underline),
            (TabSide::Right, TabStyle::Underline),
            (TabSide::Left, TabStyle::Cards),
            (TabSide::Right, TabStyle::Cards),
        ] {
            app.settings.tab_side = side;
            app.settings.tabs = style;
            let (main, tabs) = app.split_body(body);
            let tabs = tabs.expect("wide enough for a column");
            assert_eq!(
                (tabs.width, tabs.height, main.width),
                (SIDE_WIDTH, 20, 100 - SIDE_WIDTH)
            );
            assert_eq!(main.x, if side == TabSide::Left { SIDE_WIDTH } else { 0 });
            terminal
                .draw(|frame| {
                    render_rows(
                        app.view.as_ref().unwrap().shown(),
                        main,
                        frame.buffer_mut(),
                        None,
                    );
                    app.draw_tabs(frame, tabs);
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            snapshot(&buffer, &format!("tabs-{}-{}", side.name(), style.name()));
            let at = |x: u16, y: u16| buffer[(x, y)].symbol().to_string();
            let text = |r: Rect, y: u16| (r.x..r.right()).map(|x| at(x, y)).collect::<String>();

            let layout = app.tab_layout(tabs);
            assert!(text(layout.home, 0).contains("home"), "{side:?}");
            let rects: Vec<Rect> = layout.tabs.iter().map(|&(_, r)| r).collect();
            assert_eq!(rects.len(), 3);
            assert_eq!(rects[1].y, rects[0].y + 3, "two rows and a gap");
            assert!(text(rects[1], rects[1].y).contains("2 proj2"));
            assert!(text(rects[2], rects[2].y + 1).contains("codex · ● needs input"));
            assert!(
                text(main, main.y).starts_with("inside session 2"),
                "{side:?} {style:?}"
            );
            if style == TabStyle::Underline {
                let rule_x = if side == TabSide::Left {
                    tabs.right() - 1
                } else {
                    tabs.x
                };
                assert_eq!(at(rule_x, rects[1].y), "┃", "{side:?}: attached tab marked");
                assert_eq!(at(rule_x, rects[0].y), "│", "{side:?}: others not");
            }
            assert_eq!(
                app.tab_hit(tabs, rects[2].x + 3, rects[2].y + 1),
                Some(TabHit::Session(3))
            );
            assert_eq!(
                app.tab_hit(tabs, layout.plus.x + 1, layout.plus.y),
                Some(TabHit::New)
            );
            assert_eq!(app.tab_hit(tabs, layout.home.x + 1, 0), Some(TabHit::Home));
        }

        // Too narrow for a column: the session gets the whole width.
        app.settings.tab_side = TabSide::Left;
        assert!(app.split_body(Rect::new(0, 0, 60, 20)).1.is_none());
        assert!(
            app.split_body(Rect::new(0, 0, 100, 8)).1.is_none(),
            "too short"
        );

        // A short column slides to keep the attached tab in view.
        app.sessions = (1..=12).map(info).collect();
        app.view = Some(Attached::new(11, "eleven".into(), None));
        let layout = app.tab_layout(Rect::new(0, 0, SIDE_WIDTH, 16));
        assert!(layout.more_before.is_some());
        assert!(layout.tabs.iter().any(|&(i, _)| i == 10));
        for &(_, r) in &layout.tabs {
            assert!(r.bottom() <= layout.plus.y, "tabs overlap the +");
        }
        assert!(layout.plus.bottom() <= 16);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The settings panel: arrows change the setting under the cursor, Esc closes.
    #[tokio::test]
    async fn settings_panel_changes_what_it_shows() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir =
            std::env::temp_dir().join(format!("valkyrie-tui-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        // Saves go to a throwaway file, not this machine's.
        app.settings_file = dir.join("valkyrie/settings.toml");
        app.on_home_key(HomeKey::Settings).await;
        assert_eq!(app.settings_cursor, Some(0));
        // Down three times to "Tabs on", right: left. Mouse reports (SGR, and legacy with a
        // space and `l` for bytes) and pasted text are not keys.
        app.on_input(b"jjj\x1b[C\x1b[<0;5;5M\x1b[M lj\x1b[200~ll,q\x1b[201~".to_vec())
            .await;
        assert_eq!(app.settings.tab_side, TabSide::Left);
        assert_eq!(app.settings.theme.name, "dracula");
        let saved = std::fs::read_to_string(dir.join("valkyrie/settings.toml")).unwrap();
        assert!(saved.contains("tab_side = \"left\""), "{saved}");

        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| app.draw_settings(frame, 3)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        snapshot(&buffer, "settings");
        let screen: String = (0..24)
            .map(|y| {
                (0..100)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert!(
            screen.contains("Settings") && screen.contains("‹ left ›"),
            "{screen}"
        );
        assert!(screen.contains("settings.toml"), "{screen}");

        app.on_input(b"\x1b".to_vec()).await;
        assert_eq!(app.settings_cursor, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn tab_strip_slides_to_the_cursor() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-slide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        // Not whatever this machine's settings.toml says.
        app.settings = Settings::default();
        app.sessions = (1..=20).map(info).collect();
        let area = Rect::new(0, 0, 80, 2);
        let layout = app.tab_layout(area);
        assert_eq!(layout.tabs[0].0, 0);
        assert!(layout.more_after.is_some() && layout.more_before.is_none());
        app.tab_cursor = Some(17);
        let layout = app.tab_layout(area);
        assert!(layout.tabs.iter().any(|(i, _)| *i == 17));
        assert!(layout.more_before.is_some());
        assert!(layout.plus.right() <= area.right());
        for (_, r) in &layout.tabs {
            assert!(r.right() <= layout.plus.x, "tabs overlap the +");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A tab split in two: one tab on the strip, each pane drawn in its own area
    /// with a divider between, and the mouse goes to the pane under it.
    #[tokio::test]
    async fn split_tab_draws_its_panes_and_routes_the_mouse() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-split-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        // Not whatever this machine's settings.toml says: the two-row cards.
        app.settings = Settings::default();
        app.settings.tabs = TabStyle::Cards;
        app.fixed_size = Some(Size { cols: 81, rows: 23 });
        app.sessions = (1..=3)
            .map(|id| {
                let mut s = info(id);
                s.name = format!("proj{id}");
                s.command = vec!["/usr/bin/fish".into()];
                s
            })
            .collect();
        let mut layout = Pane::leaf(1);
        layout.split(1, 2, Side::Right);
        app.layouts = vec![layout.clone()];
        app.layout = Some(layout);
        let mut left = Attached::new(1, "proj1".into(), None);
        left.apply(update(true, vec![row(0, "left pane")]));
        let mut right = Attached::new(2, "proj2".into(), None);
        right.apply(update(true, vec![row(0, "right pane")]));
        app.view = Some(left);
        app.others = vec![right];

        assert_eq!(app.tabs(), vec![vec![0, 1], vec![2]]);
        assert_eq!(
            app.tab_session(0).map(|s| s.id),
            Some(1),
            "the focused pane"
        );
        assert_eq!(app.attached_index(), Some(0));

        let mut terminal = Terminal::new(TestBackend::new(81, 24)).unwrap();
        let mut draw = |app: &App| {
            terminal
                .draw(|frame| {
                    let [body, _] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                        .areas(frame.area());
                    let (main, tabs) = app.split_body(body);
                    app.draw_panes(frame, main, true);
                    app.draw_tabs(frame, tabs.unwrap());
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect()
                })
                .collect::<Vec<String>>()
        };
        let lines = draw(&app);
        assert!(lines[1].contains("⊞2 fish"), "{}", lines[1]);
        assert!(
            !lines[0].contains("proj2"),
            "one tab for both panes: {}",
            lines[0]
        );
        assert!(lines[0].contains("proj3"), "{}", lines[0]);
        assert!(lines[2].starts_with("left pane"), "{}", lines[2]);
        // 80 columns of room: 40 each, the divider between.
        assert_eq!(lines[2].chars().nth(40), Some('│'));
        assert_eq!(
            &lines[2].chars().skip(41).take(10).collect::<String>(),
            "right pane"
        );
        assert_eq!(lines[22].chars().nth(40), Some('│'));

        // A click in the right pane focuses it and, the program not wanting the
        // mouse, starts a selection in its own cells.
        app.on_mouse(Mouse::at(MouseKind::Press, 45, 3)).await;
        let view = app.view.as_ref().unwrap();
        assert_eq!(view.id, 2);
        assert_eq!(view.selection.map(|s| s.anchor), Some((4, 1)));
        assert_eq!(app.mouse_owner, Some(2));
        // A drag that strays into the other pane still belongs to this one.
        app.on_mouse(Mouse::at(MouseKind::Drag, 10, 3)).await;
        assert_eq!(app.view.as_ref().unwrap().selection.unwrap().head, (0, 1));
        app.on_mouse(Mouse::at(MouseKind::Release, 10, 3)).await;
        assert_eq!(app.mouse_owner, None);
        assert_eq!(app.view.as_ref().unwrap().id, 2);

        // A program that wants the mouse gets the click; Valkyrie doesn't select.
        app.others[0].modes.mouse_click = true;
        app.others[0].modes.mouse_sgr = true;
        app.on_mouse(Mouse::at(MouseKind::Press, 3, 4)).await;
        let view = app.view.as_ref().unwrap();
        assert_eq!((view.id, view.selection), (1, None));
        app.on_mouse(Mouse::at(MouseKind::Release, 3, 4)).await;

        // A right click opens the pane menu there, even over a mouse program.
        app.on_mouse(Mouse::at(MouseKind::RightPress, 5, 6)).await;
        let menu = app.menu.expect("pane menu");
        assert_eq!(
            (menu.kind, menu.session, (menu.x, menu.y)),
            (MenuKind::Pane, 1, (5, 6))
        );
        assert_eq!(menu.items()[0], "Split right");
        let lines = draw_menu_lines(&app);
        assert!(lines.iter().any(|l| l.contains("Split down")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("Close pane")));
        app.on_keys(b"\x1b".to_vec()).await;
        assert_eq!(app.menu, None);
        // With Ctrl held, a right click goes to a program that wants the mouse.
        let ctrl_right = Mouse {
            code: 2 | 16,
            ..Mouse::at(MouseKind::RightPress, 5, 6)
        };
        app.on_mouse(ctrl_right).await;
        assert_eq!(app.menu, None);
        // A release lost outside the window leaves pane 1 owning the mouse; the next
        // press, over pane 2, is a new gesture there.
        app.on_mouse(Mouse::at(MouseKind::Press, 3, 4)).await;
        assert_eq!(app.mouse_owner, Some(1));
        app.on_mouse(Mouse::at(MouseKind::Press, 45, 3)).await;
        assert_eq!(
            (app.mouse_owner, app.view.as_ref().unwrap().id),
            (Some(2), 2)
        );
        app.on_mouse(Mouse::at(MouseKind::Release, 45, 3)).await;
        app.focus(1);
        // Near the bottom right it stays on screen.
        app.on_mouse(Mouse::at(MouseKind::RightPress, 79, 22)).await;
        let rect = app.menu.unwrap().rect();
        assert!(rect.right() <= 81 && rect.bottom() <= 24, "{rect:?}");
        app.menu = None;

        // Dragging the divider moves it; each pane gets its new size.
        app.on_mouse(Mouse::at(MouseKind::Press, 40, 10)).await;
        assert!(app.divider_drag.is_some());
        app.on_mouse(Mouse::at(MouseKind::Drag, 20, 10)).await;
        let main = app.main_area().unwrap();
        let (panes, _) = app.pane_layout(main);
        assert_eq!((panes[0].1.width, panes[1].1.width), (20, 60));
        assert_eq!(app.pane_mut(2).unwrap().size, Size { cols: 60, rows: 21 });
        assert_eq!(app.pane_mut(1).unwrap().size, Size { cols: 20, rows: 21 });
        // The daemon's layout, still at the old ratio, doesn't undo a drag under way.
        app.sync_panes().await;
        assert_eq!(app.pane_mut(1).unwrap().size, Size { cols: 20, rows: 21 });
        app.divider_drag = None;

        // `o` cycles focus through the panes (the right click above focused 2).
        app.focus(1);
        app.on_keys(vec![TABS_KEY, b'o']).await;
        assert_eq!(app.view.as_ref().unwrap().id, 2);
        app.on_keys(vec![TABS_KEY, b'O']).await;
        assert_eq!(app.view.as_ref().unwrap().id, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fit_cuts_with_an_ellipsis() {
        assert_eq!(fit("api-auth", 8), "api-auth");
        assert_eq!(fit("api-auth", 5), "api-…");
        assert_eq!(fit("日本語", 4), "日…");
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
        render_rows(&view.rows, area, &mut buf, None);
        assert_eq!(buf[(2, 1)].symbol(), "日");
        assert_eq!(buf[(4, 1)].symbol(), "x");
        assert!(buf[(4, 1)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_theme_colors_defaults_and_ansi_but_not_exact_colors() {
        let span = |x, fg| Span {
            x,
            text: "x".into(),
            style: Style {
                fg,
                ..Style::default()
            },
        };
        let rows = [Row {
            y: 0,
            spans: vec![
                span(0, Color::Default),
                span(1, Color::Indexed(1)),
                span(2, Color::Indexed(100)),
                span(3, Color::Rgb(1, 2, 3)),
            ],
            wrapped: false,
        }];
        let t = &theme::BLACKOUT;
        let area = Rect::new(0, 0, 6, 2);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        render_rows(&rows, area, &mut buf, Some(t));
        assert_eq!(buf[(0, 0)].fg, t.fg);
        assert_eq!(buf[(1, 0)].fg, t.ansi[1]);
        assert_eq!(buf[(2, 0)].fg, style::Color::Indexed(100));
        assert_eq!(buf[(3, 0)].fg, style::Color::Rgb(1, 2, 3));
        assert_eq!(buf[(0, 0)].bg, t.bg);
        assert_eq!(
            buf[(5, 1)].bg,
            t.bg,
            "blank cells take the theme's background"
        );

        let mut plain = ratatui::buffer::Buffer::empty(area);
        render_rows(&rows, area, &mut plain, None);
        assert_eq!(plain[(0, 0)].fg, style::Color::Reset);
        assert_eq!(plain[(1, 0)].fg, style::Color::Indexed(1));
    }
}

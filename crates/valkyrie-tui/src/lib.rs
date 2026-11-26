//! Terminal client. Talks to the daemon only through `valkyrie-proto` (ADR-0004).
//!
//! Input is read as raw stdin bytes and forwarded untouched while attached, so the
//! program sees exactly what the user's terminal sends. For that to be correct the
//! outer terminal mirrors the program's input modes (app cursor, bracketed paste…).

mod avatar;
mod backlog;
mod batch;
mod blocks;
mod clip;
mod comments;
mod compose;
mod conflict;
mod digest;
mod docs;
mod edit;
mod history;
mod keys;
mod links;
mod markdown;
mod memory;
mod mini;
mod mouse;
mod panes;
mod ping;
mod prs;
mod quiet;
mod quota;
mod rain;
mod search;
mod settings;
mod supervisor;
mod term;
mod theme;
mod wiki;

use anyhow::Result;
use keys::{Action, Key, Keymap};
pub use links::open as open_link;
use mouse::{Input, Mouse, MouseKind, Selection};
pub use ping::deliver as deliver_notice;
use ping::{Kind, Ping, Pinger};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{self, Modifier, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Cell, HighlightSpacing, Padding, Paragraph, Row as TableRow, Table,
    TableState, Wrap,
};
use settings::{Field, Settings, TabSide, TabStyle};
pub use settings::{context_enabled, on_context_change};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use theme::{Theme, state_icon, state_word};
use tokio::sync::mpsc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use valkyrie_proto::client::{Client, DaemonInfo, Pushes};
use valkyrie_proto::{
    AgentState, AgentStatus, ClientBuild, Color, CursorShape, Decision, Item, ItemChange, KeptItem,
    Modes, NewItem, Pane, QueueItem, ReviewAction, Row, ScreenUpdate, ScrollAnchor, ServerMsg,
    SessionId, SessionInfo, ShippedItem, Side, Size, SpawnSpec, Style,
};

/// Ctrl-] — detach from the attached session, whatever the keys page maps.
pub const DETACH_KEY: u8 = keys::HOME_KEY;
/// Ctrl-\ — tab mode's key unless remapped; tests type it.
#[cfg(test)]
const TABS_KEY: u8 = 0x1c;

/// The first direct key in `bytes`: where it starts, what it does, and its length.
fn find_direct(bytes: &[u8], map: &Keymap) -> Option<(usize, Action, usize)> {
    (0..bytes.len()).find_map(|i| map.at(&bytes[i..]).map(|(a, n)| (i, a, n)))
}

/// A run of Alt-n: where it started (`None`: home), where it last landed, and every
/// session it has shown, so each one that needs you comes up once.
struct NeedingRun {
    from: Option<SessionId>,
    at: SessionId,
    shown: Vec<SessionId>,
}

/// How long the TUI keeps trying to reach the daemon after the connection drops. An
/// upgrade handoff (ADR-0006) takes milliseconds; a daemon restart is not coming back.
const RECONNECT_FOR: Duration = Duration::from_secs(10);

static UPDATE_CHECK: std::sync::OnceLock<fn() -> Option<String>> = std::sync::OnceLock::new();

/// Sets how the TUI learns of a newer release: `f` names it (`v0.0.2 (abc1234)`), or
/// gives `None`. It runs on its own thread, at start and when the Updates setting is
/// turned on, so it may block on the network.
pub fn on_update_check(f: fn() -> Option<String>) {
    let _ = UPDATE_CHECK.set(f);
}

static ADD_DEVICE: std::sync::OnceLock<fn() -> Result<String, String>> = std::sync::OnceLock::new();

/// Sets how `p` on the home screen adds a device to the web app: `f` makes a short
/// pairing code (`KQ7-4M2`) for its pairing screen (ADR-0009).
pub fn on_add_device(f: fn() -> Result<String, String>) {
    let _ = ADD_DEVICE.set(f);
}

static REMOTE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Says the daemon is another machine's (`valk --remote HOST`): its paths are
/// that machine's, this one's login environment means nothing there, and its
/// binary isn't one to run here.
pub fn set_remote(host: String) {
    let _ = REMOTE.set(host);
}

fn remote() -> Option<&'static str> {
    REMOTE.get().map(String::as_str)
}

/// The login environment a session started from here gets: none on another
/// machine, where this one's agent socket and display mean nothing.
fn spawn_env() -> Vec<(String, Option<String>)> {
    if remote().is_some() {
        Vec::new()
    } else {
        valkyrie_proto::login_env()
    }
}

/// The terminal, through a backend that stays silent while nothing changes.
type Term = Terminal<quiet::Quiet<CrosstermBackend<std::io::Stdout>>>;

pub async fn run(
    client: Client,
    pushes: Pushes,
    attach_to: Option<SessionId>,
    socket: PathBuf,
) -> Result<()> {
    // Now, while it names this binary: an upgrade renames a new one over it.
    let exe = std::env::current_exe();
    valkyrie_proto::binary_id();
    // Raw mode, the alternate screen and ratatui's panic hook; then the same
    // terminal behind `Quiet`.
    drop(ratatui::try_init()?);
    #[cfg(windows)]
    vt_input();
    let mut terminal = Terminal::new(quiet::Quiet::new(CrosstermBackend::new(std::io::stdout())))
        .inspect_err(|_| ratatui::restore())?;
    // ratatui's own hook (installed by init) restores raw mode and the screen; chain
    // ours in front so mirrored input modes don't outlive a panic either.
    let ratatui_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = reset_terminal_modes();
        ratatui_hook(info);
    }));
    let mut app = App::new(client, socket.clone());
    let result = app.run(&mut terminal, pushes, attach_to).await;
    let _ = reset_terminal_modes();
    ratatui::restore();
    // Quit with a doc still open in your editor: nothing will save it.
    if let Some(e) = &app.editing {
        let what = match e.what {
            Edited::Doc { .. } => "A doc was still open in your editor and wasn't saved",
            Edited::Proposal { .. } => {
                "A proposal was still open in your editor and wasn't accepted"
            }
        };
        eprintln!("{what}; what you wrote goes to {}", e.path.display());
    }
    // Another machine's daemon names a binary there, never one to run here.
    #[cfg(unix)]
    if let Err(e) = &result
        && remote().is_none()
        && let Some(restart) = e.downcast_ref::<Restart>()
        && let Some(exe) = restart.daemon.follow(exe.ok())
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--socket").arg(&socket).arg("attach");
        cmd.args(restart.attach.map(|id| id.to_string()));
        let failed = cmd.exec();
        anyhow::bail!(
            "{restart}; restarting as {} failed: {failed}",
            exe.display()
        );
    }
    #[cfg(windows)]
    let _ = exe;
    result
}

/// The daemon came back from an upgrade, or speaks another protocol: this TUI is
/// out of date even when it still speaks the protocol. On Unix `run` then restarts
/// the TUI as the daemon's binary (else the one it was started from, which the
/// upgrade replaced), on the same session.
#[derive(Debug)]
struct Restart {
    /// What the daemon said when it came back.
    daemon: DaemonInfo,
    #[cfg_attr(windows, allow(dead_code))]
    attach: Option<SessionId>,
}

impl std::fmt::Display for Restart {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        if let Some(host) = remote() {
            // Never "run the new valk": this machine's is the one behind.
            write!(
                f,
                "valk on {host} now speaks protocol {}, this one {}; bring both to the \
                 same release (`valk upgrade` on each keeps every session)",
                self.daemon.protocol,
                valkyrie_proto::PROTOCOL
            )
        } else if self.daemon.protocol == valkyrie_proto::PROTOCOL {
            write!(f, "the daemon was upgraded; run the new valk")
        } else {
            write!(
                f,
                "the daemon now speaks protocol {}, this valk {}; run the new valk",
                self.daemon.protocol,
                valkyrie_proto::PROTOCOL
            )
        }
    }
}

impl std::error::Error for Restart {}

/// What the backlog's `S` task found (`asking`) or shipped.
struct Shipping {
    asking: bool,
    unfetched: usize,
    result: std::result::Result<(Vec<ShippedItem>, Vec<KeptItem>), String>,
}

struct App {
    client: Client,
    socket: PathBuf,
    /// What the daemon said when this TUI connected: an upgrade keeps its `boot`
    /// and raises its `generation`, a restart changes its `boot`.
    daemon: Option<DaemonInfo>,
    /// The ranked attention queue, as last pushed by the daemon.
    queue: Vec<QueueItem>,
    sessions: Vec<SessionInfo>,
    /// Every project's decisions waiting on a human, oldest first, as last pushed.
    proposals: Vec<Decision>,
    /// Every backlog item, as last pushed (DESIGN §8.12).
    items: Vec<Item>,
    /// The Inbox's top row: a task to start or file (backlog #276).
    composer: compose::Composer,
    /// Start agents: the composer's lines, one session each (backlog #280).
    batch: Option<batch::Batch>,
    /// The supervisor at a glance, for the composer's last line: what it
    /// watches, and the last thing it did.
    glance: Option<Glance>,
    /// The repositories pickers offer (backlog #240), as the backlog last fetched it.
    repo_list: valkyrie_proto::RepoList,
    /// Ready items the supervisor offers to start, as last pushed with the items.
    launches: Vec<valkyrie_proto::LaunchOffer>,
    /// The plan's quota, as last pushed (protocol 30).
    quota: Option<valkyrie_proto::Quota>,
    /// The disks running low on space (backlog #300): one Needs-you row.
    disk: Vec<valkyrie_proto::LowDisk>,
    /// Where a background task (`P`'s model call) reports a line to say.
    said: Option<mpsc::UnboundedSender<String>>,
    /// The backlog page is open (`b`).
    backlog: Option<backlog::Page>,
    /// A flagged decision `d` was pressed on once: retiring has no undo, so it
    /// takes a second `d`.
    retire_armed: Option<(PathBuf, u32)>,
    /// How far down the shown proposal's text is scrolled: whose it is, the top
    /// row, and how many rows show. Drawing keeps it in range.
    proposal_scroll: std::cell::Cell<ProposalScroll>,
    /// The links in the proposal text last drawn, by row of the whole text.
    proposal_links: std::cell::RefCell<Vec<markdown::Link>>,
    /// One selection over the proposals, then the queue rows, then the session rows.
    selected: usize,
    /// What the home screen last drew where, first match wins: its rows (each with
    /// its index in the selection), the backlog panel, the bar's chips, the preview.
    /// What a click or the wheel is matched against.
    home_rows: std::cell::RefCell<Vec<(Rect, Hit)>>,
    /// The mouse is Valkyrie's on the home screen too (written once per detach).
    home_captured: bool,
    view: Option<Attached>,
    status: String,
    quit: bool,
    /// Theme, tabs and sound; `,` opens the panel that changes them.
    settings: Settings,
    /// Where `settings` are kept.
    settings_file: PathBuf,
    /// A newer release, for the home screen's header.
    newer: Option<String>,
    /// The update check under way.
    newer_rx: Option<std::sync::mpsc::Receiver<Option<String>>>,
    /// Where the backlog's `S` reports back from its task, while `run` reads it.
    shipping: Option<mpsc::UnboundedSender<Shipping>>,
    /// That task is under way: another `S` waits for it.
    ship_busy: bool,
    /// The settings panel is open, its cursor on this field.
    settings_cursor: Option<usize>,
    /// The mini terminal's shell, once started (`mini.rs`).
    mini: Option<mini::Mini>,
    /// The keys page is open (`.` on the home screen).
    keys_page: Option<KeysPage>,
    /// The away digest is open (`w` on the home screen).
    digest: Option<digest::Page>,
    /// The supervisor page is open (`s` on the home screen).
    supervisor: Option<supervisor::Page>,
    /// The Docs page is open (`D` on the home screen).
    docs: Option<docs::Page>,
    /// The docs a session wrote, read beside its panes (`D` in tab mode, or a
    /// click on a mention of one; backlog #311).
    beside: Option<Beside>,
    /// Which docs page a `docs::Do` answers: home's, or the one beside a session.
    docs_on: DocsOn,
    /// The repository wiki is open (`W` on the home screen).
    wiki: Option<wiki::Page>,
    /// Memory's Decisions, Skills or Notes tab (`g m`).
    memory: Option<memory::Page>,
    /// Backlog's Pull requests tab: the PRs the supervisor's poll watches.
    prs: Option<prs::Page>,
    /// A doc open in your editor, in a session of its own.
    editing: Option<Editing>,
    /// When the open section page last loaded on its own (`reload_page`).
    page_loaded: Instant,
    /// What the last `review_at` would take back, for Memory's `z`.
    last_answer: Option<Option<memory::Undo>>,
    /// The selected session's screen text, for the preview pane.
    preview: Option<(SessionId, String)>,
    pinger: Pinger,
    /// The last ping, shown in the bar for `TOAST_FOR`.
    toast: Option<(Ping, Instant)>,
    /// Spots agents' new backlog proposals, for a toast (#194).
    new_proposals: ping::Proposals,
    /// The start of a mouse report the last read cut off.
    partial: Vec<u8>,
    /// Tab mode: the next key acts on the attached tab (split, rename, …) rather
    /// than going to the program.
    tab_mode: bool,
    /// The tab attached before this one, for Alt-p.
    previous: Option<SessionId>,
    /// The section page you attached from, set aside until you come home, so
    /// home is the section and row you left (spec: Two layers).
    parked: Option<Parked>,
    /// `g` was pressed: the next key picks a section.
    g_pending: bool,
    /// The cursor was put somewhere, by a key, a click or an attach. Until then
    /// it stays on the top row while the first pushes arrive, rather than
    /// following whichever row loaded first (backlog #267).
    cursor_moved: bool,
    /// `x` pressed once on this session: a second closes it (rule 9: x always
    /// confirms).
    kill_armed: Option<SessionId>,
    /// The `?` sheet of every key is open.
    help: bool,
    /// How wide the bar under a session was last drawn, so a click finds its
    /// keys where they were drawn.
    bar_cols: std::cell::Cell<u16>,
    /// How wide home's list was last drawn: under 100 columns a session's
    /// detail is its own screen (spec: Layout by width).
    home_cols: std::cell::Cell<u16>,
    /// That screen is open, for the selected session.
    home_detail: bool,
    /// The Alt-n run going on, if the last switch was one.
    needing: Option<NeedingRun>,
    /// The session being renamed, and the name typed so far.
    renaming: Option<(SessionId, edit::LineEdit)>,
    /// A tab's menu, opened with a right click or Alt-w.
    menu: Option<Menu>,
    /// The decisions to review, opened over a session with `d` in tab mode, its
    /// cursor on this proposal.
    reviewing: Option<usize>,
    /// The tab being dragged to a new place.
    tab_drag: Option<SessionId>,
    /// The tab last pressed, and when: a second press soon after renames it.
    tab_press: Option<(SessionId, Instant)>,
    /// The mouse is over the minimal tabs' rail, which opens it.
    rail_hover: bool,
    /// An Alt key has come through, so the terminal sends Option as Meta, or isn't
    /// a Mac's: no hint about it is needed.
    alt_works: bool,
    /// Bracketed paste is on in the outer terminal for a backlog prompt
    /// (`sync_paste`).
    paste_forced: bool,
    /// The tab under the mouse, and since when: resting there peeks at it.
    hover_tab: Option<(usize, Instant)>,
    /// The attached tab's other panes, when it is split; `view` is the focused one.
    others: Vec<Attached>,
    /// The attached tab's split layout (`None`: one pane).
    layout: Option<Pane>,
    /// The focused pane fills the screen, the others hidden but running
    /// (`toggle_zoom`). This TUI's alone; the layout doesn't change.
    zoomed: bool,
    /// Every split tab's layout, from the last session list.
    layouts: Vec<Pane>,
    /// The divider being dragged, and the ratio it was last dragged to.
    divider_drag: Option<(valkyrie_proto::layout::Divider, u16)>,
    /// The pane a button went down in: its drag and release go there too.
    mouse_owner: Option<SessionId>,
    /// Whether the mouse capture has been written to the terminal.
    captured: bool,
    /// Tests draw at this size (terminal minus the bar) instead of the terminal's.
    fixed_size: Option<Size>,
}

struct Attached {
    id: SessionId,
    name: String,
    /// `name` is the agent's topic, not the user's (`SessionInfo::auto_name`).
    auto_name: bool,
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
    /// A short message for the bar (`copied 42 chars`), and until when it shows.
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
/// How long the hint about Option stays: long enough to follow it.
const OPTION_HINT_FOR: Duration = Duration::from_secs(15);

impl Attached {
    fn new(id: SessionId, name: String, status: Option<AgentStatus>) -> Self {
        Self {
            id,
            name,
            auto_name: false,
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
        self.notice_for(text, NOTICE_FOR);
    }

    fn notice_for(&mut self, text: String, time: Duration) {
        self.notice = Some((text, Instant::now() + time));
    }
}

impl App {
    fn new(client: Client, socket: PathBuf) -> Self {
        Self {
            client,
            socket,
            daemon: None,
            queue: Vec::new(),
            sessions: Vec::new(),
            proposals: Vec::new(),
            items: Vec::new(),
            composer: compose::Composer::default(),
            batch: None,
            glance: None,
            repo_list: Default::default(),
            quota: None,
            disk: Vec::new(),
            backlog: None,
            retire_armed: None,
            proposal_scroll: Default::default(),
            proposal_links: Default::default(),
            selected: 0,
            home_rows: Default::default(),
            home_captured: false,
            view: None,
            status: String::new(),
            quit: false,
            settings: Settings::load(),
            settings_file: settings::path(),
            newer: None,
            newer_rx: None,
            shipping: None,
            ship_busy: false,
            launches: Vec::new(),
            said: None,
            settings_cursor: None,
            keys_page: None,
            digest: None,
            supervisor: None,
            docs: None,
            beside: None,
            docs_on: DocsOn::Home,
            wiki: None,
            memory: None,
            prs: None,
            editing: None,
            page_loaded: Instant::now(),
            last_answer: None,
            preview: None,
            pinger: Pinger::default(),
            toast: None,
            new_proposals: ping::Proposals::default(),
            partial: Vec::new(),
            tab_mode: false,
            previous: None,
            parked: None,
            g_pending: false,
            cursor_moved: false,
            kill_armed: None,
            help: false,
            bar_cols: std::cell::Cell::new(80),
            home_cols: std::cell::Cell::new(120),
            home_detail: false,
            needing: None,
            renaming: None,
            menu: None,
            reviewing: None,
            tab_press: None,
            tab_drag: None,
            rail_hover: false,
            alt_works: false,
            paste_forced: false,
            hover_tab: None,
            others: Vec::new(),
            layout: None,
            zoomed: false,
            layouts: Vec::new(),
            divider_drag: None,
            mouse_owner: None,
            captured: false,
            fixed_size: None,
            mini: None,
        }
    }

    async fn run(
        &mut self,
        terminal: &mut Term,
        mut pushes: Pushes,
        attach_to: Option<SessionId>,
    ) -> Result<()> {
        let mut stdin = stdin_bytes();
        let mut winch = Resizes::new()?;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        // Only redraws, for the working spinner; runs while one is on screen.
        let mut anim = tokio::time::interval(SPIN_EVERY);
        anim.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        self.daemon = self.client.hello_as(ClientBuild::this("tui")).await.ok();
        self.report_cell(None);
        if let Err(e) = self.client.watch_queue().await {
            self.status = format!("queue unavailable: {e:#}");
        }
        // Best effort: an older daemon doesn't know notices.
        let _ = self.client.watch_notices(true).await;
        self.refresh().await;
        // The composer's repositories and supervisor line (backlog #276).
        self.load_repos().await;
        self.load_glance().await;
        if let Some(id) = attach_to {
            self.attach(id).await;
        }
        self.look_for_updates();
        let (shipping_tx, mut shipping) = mpsc::unbounded_channel();
        self.shipping = Some(shipping_tx);
        let (said_tx, mut said) = mpsc::unbounded_channel();
        self.said = Some(said_tx);

        while !self.quit {
            self.draw(terminal)?;
            tokio::select! {
                bytes = stdin.recv() => match bytes {
                    Some(bytes) => {
                        self.on_input(bytes).await;
                        self.sync_paste();
                    }
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
                    self.mini_reap().await;
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
                    if self.page_loaded.elapsed() >= PAGE_RELOAD {
                        self.reload_page().await;
                    }
                    // An editor that ended while you were elsewhere.
                    self.finish_edit().await;
                    self.update_preview().await;
                    if let Some(newer) = self.newer_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                        self.newer = newer;
                        self.newer_rx = None;
                    }
                }
                Some(done) = shipping.recv() => self.on_shipping(done),
                Some(line) = said.recv() => self.say(line),
                _ = anim.tick(), if self.animating() => {}
                _ = sleep_until(self.pinger.next_due()), if self.pinger.next_due().is_some() => {
                    self.fire_pings();
                }
                _ = sleep_until(self.peek_due()), if self.peek_due().is_some() => {
                    self.update_preview().await;
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
            let info = match client.hello_as(ClientBuild::this("tui")).await {
                // Another machine's upgrade leaves this valk as it was: carry on
                // while it still speaks the protocol.
                Ok(info)
                    if info.protocol == valkyrie_proto::PROTOCOL
                        && (remote().is_some()
                            || !self.daemon.as_ref().is_some_and(|d| info.upgraded_from(d))) =>
                {
                    info
                }
                Ok(info) => {
                    return Err(Restart {
                        daemon: info,
                        attach: self.view.as_ref().map(|v| v.id),
                    }
                    .into());
                }
                Err(_) => continue,
            };
            self.client = client;
            // Session ids start over in a restarted daemon: re-attaching by id could
            // land in an unrelated program.
            let same = self.daemon.as_ref().is_some_and(|d| d.boot == info.boot);
            if !same {
                self.pinger.reset();
            }
            self.daemon = Some(info);
            self.status = if same {
                "reconnected to the daemon".into()
            } else {
                self.view = None;
                self.others.clear();
                self.layout = None;
                self.reset_outer_modes();
                "the daemon restarted; restorable sessions came back with new ids".into()
            };
            if let Err(e) = self.client.watch_queue().await {
                self.status = format!("queue unavailable: {e:#}");
            }
            let _ = self.client.watch_notices(true).await;
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
            || (self.settings.theme.rain.is_some() && self.view.is_none())
    }

    /// Fetches the screen for the preview pane: the selected session's at home, the
    /// peeked tab's while attached.
    async fn update_preview(&mut self) {
        let id = match &self.view {
            None => self.selection().0,
            Some(_) => self.peeked().map(|s| s.id),
        };
        let Some(id) = id else {
            self.preview = None;
            return;
        };
        // A slow daemon leaves the last one, rather than holding up the keys.
        let dump = tokio::time::timeout(PREVIEW_WAIT, self.client.dump(id)).await;
        if let Ok(text) = dump {
            self.preview = text.ok().map(|text| (id, text));
        }
    }

    /// The tab the mouse rests on, when it is another session than the attached
    /// tab's: a peek at it shows over the session.
    fn peeked(&self) -> Option<&SessionInfo> {
        let s = self.tab_session(self.peek_tab()?)?;
        (!self.pane_ids().contains(&s.id)).then_some(s)
    }

    /// The tab under the mouse, once it has rested there `HOVER_DELAY`: a pointer
    /// on its way to a click doesn't flash a peek.
    fn peek_tab(&self) -> Option<usize> {
        self.hover_tab
            .filter(|(_, at)| at.elapsed() >= HOVER_DELAY)
            .map(|(i, _)| i)
    }

    /// When the peek under the mouse is due, until it shows.
    fn peek_due(&self) -> Option<Instant> {
        self.hover_tab
            .map(|(_, at)| at + HOVER_DELAY)
            .filter(|&due| due > Instant::now())
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
                        .map(|s| (s.name.clone(), s.auto_name));
                    if let Some(pane) = self.pane_mut(id) {
                        pane.status = status;
                        if let Some((name, auto)) = name {
                            pane.name = name;
                            pane.auto_name = auto;
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
        let mut out = term::outer();
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
        self.drop_g();
        self.home_detail = false;
        self.help = false;
        self.kill_armed = None;
        self.attach_tab(id).await;
    }

    async fn detach(&mut self) {
        self.drop_g();
        self.help = false;
        self.kill_armed = None;
        if let Some(view) = &self.view {
            self.previous = Some(view.id);
        }
        self.view = None;
        self.others.clear();
        self.layout = None;
        self.mouse_owner = None;
        self.divider_drag = None;
        self.tab_mode = false;
        self.hover_tab = None;
        self.menu = None;
        self.reviewing = None;
        // Detaching ends its watch too; the next press attaches it again.
        if let Some(m) = &mut self.mini {
            m.open = false;
        }
        self.reset_outer_modes();
        let _ = self.client.detach().await;
        self.unpark();
        self.refresh().await;
        // The preview showed a peeked tab, or nothing, while attached.
        self.update_preview().await;
    }

    async fn on_input(&mut self, mut bytes: Vec<u8>) {
        if !self.partial.is_empty() {
            let mut joined = std::mem::take(&mut self.partial);
            joined.extend(bytes);
            bytes = joined;
        }
        if self.view.is_none() && self.backlog.is_none() && !self.mini_open() {
            // Home: reports are clicks, and never typed into a prompt.
            if let Some(cut) = mouse::unfinished(&bytes) {
                self.partial = bytes.split_off(cut);
            }
            for input in mouse::split(&bytes) {
                match input {
                    Input::Bytes(keys) => Box::pin(self.home_screen_keys(keys)).await,
                    Input::Mouse(m) => self.on_home_mouse(m).await,
                }
            }
            return;
        }
        if self.keys_page.is_some() {
            return self.keys_page_keys(&bytes);
        }
        if self.digest.is_some() {
            return self.digest_keys(&bytes).await;
        }
        if self.supervisor.is_some() {
            return self.supervisor_keys(&bytes).await;
        }
        if self.wiki.is_some() {
            return self.wiki_keys(&bytes).await;
        }
        if self.memory.is_some() && self.view.is_none() {
            return self.memory_keys(&bytes).await;
        }
        if self.prs.is_some() && self.view.is_none() {
            return self.prs_keys(&bytes).await;
        }
        // Over a session, the Docs page waits: your editor on a doc is one.
        if self.docs_shown() {
            return self.docs_keys(&bytes).await;
        }
        if self.backlog.is_some() {
            // A mouse report cut off by the read isn't typed into a prompt.
            if let Some(cut) = mouse::unfinished(&bytes) {
                self.partial = bytes.split_off(cut);
            }
            if self.view.is_none() {
                // At home the backlog is a section: a click is the strip's or a
                // row's, never keys to the page (backlog #281), and a key goes
                // to `g` and the strip's keys first. The cut-off report stays
                // after these bytes, not before.
                let partial = std::mem::take(&mut self.partial);
                for input in mouse::split(&bytes) {
                    match input {
                        Input::Mouse(m) => self.on_home_mouse(m).await,
                        // A click or key earlier in the read left the backlog.
                        Input::Bytes(keys) if self.backlog.is_none() => {
                            Box::pin(self.on_input(keys)).await;
                        }
                        Input::Bytes(keys) => {
                            let Some(rest) = self.section_keys(keys).await else {
                                continue;
                            };
                            if self.backlog.is_none() {
                                Box::pin(self.on_input(rest)).await;
                            } else {
                                self.backlog_keys(&rest).await;
                            }
                        }
                    }
                }
                self.partial = partial;
                return;
            }
            return self.backlog_keys(&bytes).await;
        }
        if self.settings_cursor.is_some() {
            return self.settings_keys(&bytes);
        }
        if self.mini_open() {
            return self.mini_input(bytes).await;
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
                // A key closes a peek the mouse opened, as it would a tooltip.
                if matches!(input, Input::Bytes(_)) {
                    self.hover_tab = None;
                }
                match (&self.view, input) {
                    // A key earlier in the read opened the backlog page.
                    (_, Input::Bytes(rest)) if self.backlog.is_some() => {
                        Box::pin(self.backlog_keys(&rest)).await;
                    }
                    (_, Input::Mouse(_)) if self.backlog.is_some() => {}
                    // The key sheet is open over the session: keys are its, and a
                    // press closes it and goes no further.
                    (Some(_), Input::Bytes(rest)) if self.help => {
                        Box::pin(self.help_keys(&rest)).await;
                    }
                    (Some(_), Input::Mouse(m)) if self.help => {
                        if m.kind == MouseKind::Press {
                            self.help = false;
                        }
                    }
                    // Detached mid-read: the rest belongs to the home screen.
                    (None, Input::Bytes(rest)) => Box::pin(self.home_input(&rest)).await,
                    (None, Input::Mouse(_)) => {}
                    (Some(_), Input::Mouse(m)) if self.reviewing.is_some() => {
                        self.on_proposal_mouse(m)
                    }
                    (Some(_), Input::Mouse(m)) => self.on_mouse(m).await,
                    // A finished program: any key returns home (or to the next pane),
                    // unless you are reading its scrollback.
                    (Some(view), Input::Bytes(keys))
                        if view.exited.is_some()
                            && view.scroll.is_none()
                            && !self.tab_mode
                            && self.renaming.is_none()
                            && self.menu.is_none()
                            && self.reviewing.is_none()
                            && find_direct(&keys, &self.settings.keys).is_none() =>
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
        self.home_screen_keys(bytes).await;
    }

    /// Typed keys while on the home screen, whichever page or prompt is over it.
    async fn home_screen_keys(&mut self, bytes: Vec<u8>) {
        // Over a page, the row above its keys says what the last key did, not
        // something said before it (backlog #277).
        if self.page_open() {
            self.status.clear();
        }
        if self.help {
            return self.help_keys(&bytes).await;
        }
        let Some(bytes) = self.section_keys(bytes).await else {
            return;
        };
        if self.keys_page.is_some() {
            return self.keys_page_keys(&bytes);
        }
        if self.digest.is_some() {
            return self.digest_keys(&bytes).await;
        }
        if self.supervisor.is_some() {
            return self.supervisor_keys(&bytes).await;
        }
        if self.wiki.is_some() {
            return self.wiki_keys(&bytes).await;
        }
        if self.memory.is_some() {
            return self.memory_keys(&bytes).await;
        }
        if self.prs.is_some() {
            return self.prs_keys(&bytes).await;
        }
        if self.docs.is_some() {
            return self.docs_keys(&bytes).await;
        }
        if self.settings_cursor.is_some() {
            return self.settings_keys(&bytes);
        }
        if self.renaming.is_some() {
            return self.rename_keys(&bytes).await;
        }
        // The decisions opened from the Inbox take the keys until Esc.
        if self.reviewing.is_some() {
            return self.review_keys(&bytes).await;
        }
        if self.batch.is_some() && self.view.is_none() {
            return self.batch_keys(&bytes).await;
        }
        if self.composing() {
            return self.compose_keys(&bytes).await;
        }
        self.option_hint(&bytes);
        self.home_input(&bytes).await;
    }

    /// What the home screen drew at `at`.
    fn home_hit(&self, at: Position) -> Option<Hit> {
        let hits = self.home_rows.borrow();
        hits.iter()
            .find(|(area, _)| area.contains(at))
            .map(|&(_, hit)| hit)
    }

    /// A press on a home row selects it, and on the selected row does what Enter
    /// does; a chip or the backlog panel does what its key does. The wheel moves the
    /// selection, or scrolls a proposal's text over the preview.
    async fn on_home_mouse(&mut self, m: Mouse) {
        self.drop_g();
        // Over the `?` sheet, a press closes it and goes no further.
        if self.help {
            if m.kind == MouseKind::Press {
                self.help = false;
            }
            return;
        }
        // The strip stays on top of every section page.
        if m.kind == MouseKind::Press
            && let Some(Hit::Section(section)) = self.home_hit(Position::new(m.x, m.y))
        {
            return self.go_section(section).await;
        }
        if self.docs.is_some() {
            return self.on_docs_mouse(m).await;
        }
        // A section page's list: a click or the wheel is the j/k and ↩ it takes.
        if let Some(keys) = self.page_mouse_keys(&m) {
            return self.page_keys(&keys).await;
        }
        if self.backlog.is_some()
            || self.keys_page.is_some()
            || self.digest.is_some()
            || self.supervisor.is_some()
            || self.wiki.is_some()
            || self.memory.is_some()
            || self.prs.is_some()
            || self.settings_cursor.is_some()
            || self.renaming.is_some()
        {
            return;
        }
        // Over the open decisions: the wheel and links are the dialog's, not the
        // rows' behind it.
        if self.reviewing.is_some() {
            return self.on_proposal_mouse(m);
        }
        let hit = self.home_hit(Position::new(m.x, m.y));
        match m.kind {
            MouseKind::Press => match hit {
                Some(Hit::Row(row)) if row == self.selected => {
                    self.on_home_key(HomeKey::Enter).await
                }
                Some(Hit::Row(row)) => {
                    self.selected = row;
                    self.cursor_moved = true;
                    self.kill_armed = None;
                    // A click off the composer hands the keys back; the draft stays.
                    if row != 0 && std::mem::take(&mut self.composer.typing) {
                        self.sync_paste();
                    }
                    self.update_preview().await;
                }
                Some(Hit::Key(key)) => self.on_home_key(key).await,
                Some(Hit::Section(section)) => self.go_section(section).await,
                // Links are a proposal's: after one, the preview holds a session.
                Some(Hit::Preview) if self.proposal().is_some() => {
                    if let Some(url) = self.proposal_link_at(m.x, m.y) {
                        self.open_url(&url);
                    }
                }
                Some(Hit::Preview) | None => {}
            },
            MouseKind::WheelUp | MouseKind::WheelDown => {
                let down = m.kind == MouseKind::WheelDown;
                if hit == Some(Hit::Preview) && self.proposal().is_some() {
                    self.scroll_proposal_rows(if down { WHEEL_ROWS } else { -WHEEL_ROWS });
                } else {
                    let key = if down { HomeKey::Down } else { HomeKey::Up };
                    self.on_home_key(key).await;
                }
            }
            _ => {}
        }
    }

    /// The keys a press or the wheel stands for over the open section page's
    /// list (backlog #281): ↩ on the selected row, j/k to another, j/k a notch.
    /// None off the rows, or with no list page open.
    fn page_mouse_keys(&self, m: &Mouse) -> Option<Vec<u8>> {
        let hits = if let Some(page) = &self.backlog {
            &page.hits
        } else if let Some(page) = &self.prs {
            &page.hits
        } else if let Some(page) = &self.memory {
            &page.hits
        } else if let Some(page) = &self.digest {
            &page.hits
        } else {
            return None;
        };
        match m.kind {
            MouseKind::Press => hits.click(m.x, m.y),
            MouseKind::WheelDown => Some(b"j".to_vec()),
            MouseKind::WheelUp => Some(b"k".to_vec()),
            _ => None,
        }
    }

    /// `keys` to the open section page, as if typed there.
    async fn page_keys(&mut self, keys: &[u8]) {
        if self.backlog.is_some() {
            self.backlog_keys(keys).await;
        } else if self.prs.is_some() {
            self.prs_keys(keys).await;
        } else if self.memory.is_some() {
            self.memory_keys(keys).await;
        } else if self.digest.is_some() {
            self.digest_keys(keys).await;
        }
    }

    /// Takes the mouse on the home screen: clicks, no motion. Written once per
    /// detach; attaching writes its own modes (`update_capture`).
    fn capture_home(&mut self) {
        if self.home_captured {
            return;
        }
        self.home_captured = true;
        let mut out = term::outer();
        let _ = write!(out, "{}{}", mouse::CAPTURE_OFF, mouse::CAPTURE_ON);
        let _ = out.flush();
    }

    /// Keys on the home screen.
    async fn home_input(&mut self, bytes: &[u8]) {
        let mut answered = false;
        for (key, end) in home_keys_at(bytes, &self.settings.keys) {
            // One answer a read: after it the queue moves, and a second digit
            // (a paste, a held key) could land on another session.
            if let HomeKey::Answer(_) = key
                && std::mem::replace(&mut answered, true)
            {
                continue;
            }
            Box::pin(self.on_home_key(key)).await;
            // The rest of the read is the backlog page's, once a key opened it.
            if self.backlog.is_some() {
                return self.backlog_keys(&bytes[end..]).await;
            }
            // Or the composer's, once `n`, `/` or `#` focused it.
            if self.composing() {
                return self.compose_keys(&bytes[end..]).await;
            }
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
        if self.reviewing.is_some() {
            return self.review_keys(&bytes).await;
        }
        if self.tab_mode {
            return self.tab_keys(&bytes).await;
        }
        // The docs beside the session have the keyboard, but for tab mode's key.
        if self.beside_shown() {
            let toggle = self.settings.keys.get(Action::Tabs).map(Key::bytes);
            if let Some(toggle) = toggle
                && let Some(at) = bytes
                    .windows(toggle.len())
                    .position(|w| w == toggle.as_slice())
            {
                self.beside_keys(&bytes[..at]).await;
                self.tab_mode = true;
                return Box::pin(self.tab_keys(&bytes[at + toggle.len()..])).await;
            }
            return self.beside_keys(&bytes).await;
        }
        self.option_hint(&bytes);
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
        match find_direct(rest, &self.settings.keys) {
            Some((i, action, len)) => {
                if i > 0 {
                    let _ = self.client.input(id, rest[..i].to_vec());
                }
                let after = &rest[i + len..];
                if action == Action::Tabs {
                    self.tab_mode = true;
                    // Keys typed right after ^\ in the same read belong to the strip.
                    return Box::pin(self.tab_keys(after)).await;
                }
                self.on_direct(action).await;
                // Keys typed right after it in the same read go where it led.
                Box::pin(self.after_direct(after)).await;
            }
            None => {
                let keys = match self.image_paste(rest).await {
                    Some(paste) => paste,
                    None => rest.to_vec(),
                };
                let _ = self.client.input(id, keys);
            }
        }
    }

    /// An agent's image-paste key with an image on the clipboard: the saved image's
    /// path, to paste in the key's place (see `clip`).
    async fn image_paste(&self, keys: &[u8]) -> Option<Vec<u8>> {
        let view = self.view.as_ref()?;
        let session = self.sessions.iter().find(|s| s.id == view.id)?;
        if !clip::is_image_key(keys) || !matches!(session.status.agent.as_str(), "claude" | "codex")
        {
            return None;
        }
        let bracketed = view.modes.bracketed_paste;
        let dir = valkyrie_proto::pastes::dir();
        // The TUI waits for it, so a clipboard that never answers (a hung owner,
        // broken WSL interop) gives up and the key goes through as typed.
        let read = tokio::task::spawn_blocking(move || clip::save_image(&dir));
        let path = tokio::time::timeout(Duration::from_secs(3), read)
            .await
            .ok()?
            .ok()??;
        Some(clip::paste_text(&path, bracketed))
    }

    /// Keys that came in the same read as a direct key, after it.
    async fn after_direct(&mut self, keys: &[u8]) {
        if keys.is_empty() {
            return;
        }
        if self.backlog.is_some() {
            self.backlog_keys(keys).await;
        } else if self.menu.is_some() {
            self.menu_keys(keys).await;
        } else if self.view.is_some() {
            self.on_keys(keys.to_vec()).await;
        } else {
            self.home_input(keys).await;
        }
    }

    /// Does what a direct key says, attached or from home.
    async fn on_direct(&mut self, action: Action) {
        let here = self.view.as_ref().map(|v| v.id);
        match action {
            // Attached, `on_keys` hands the strip the keyboard; home has no strip.
            Action::Tabs => {}
            Action::NewTab => self.new_shell(self.attached_cwd()).await,
            Action::Home => {
                if here.is_some() {
                    self.detach().await;
                }
            }
            Action::Tab(i) => match self.tab_session(i as usize).map(|s| s.id) {
                Some(id) => self.switch_to(id).await,
                None => self.say(format!("no tab {}", i + 1)),
            },
            Action::NextTab | Action::PrevTab => {
                let step = if action == Action::NextTab { 1 } else { -1 };
                let n = self.tabs().len() as isize;
                if n == 0 {
                    return;
                }
                // From home, forward is the first tab and back the last.
                let i = match self.attached_index() {
                    Some(i) => (i as isize + step).rem_euclid(n),
                    None if step > 0 => 0,
                    None => n - 1,
                };
                if let Some(id) = self.tab_session(i as usize).map(|s| s.id) {
                    self.switch_to(id).await;
                }
            }
            Action::Needing => self.next_needing().await,
            Action::Last => {
                match self
                    .previous
                    .filter(|&id| self.sessions.iter().any(|s| s.id == id))
                {
                    Some(id) => self.switch_to(id).await,
                    None => self.say("no tab to go back to".into()),
                }
            }
            Action::Close => {
                if let Some(id) = here {
                    self.open_menu(id, MENU_CLOSE);
                }
            }
            Action::Pane => self.cycle_focus(1),
            Action::Zoom => self.toggle_zoom(),
            Action::Mini => self.toggle_mini().await,
            Action::Capture => {
                self.open_backlog().await;
                if let Some(page) = &mut self.backlog {
                    page.typing = Some(backlog::Typing::add());
                    page.capture = true;
                }
            }
        }
    }

    /// Whether `key` is the one tab mode is mapped to.
    fn is_tabs_key(&self, key: &[u8]) -> bool {
        self.settings
            .keys
            .get(Action::Tabs)
            .map(Key::bytes)
            .as_deref()
            == Some(key)
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

    /// Keys in tab mode: one acts on the attached tab, then the keyboard goes back
    /// to the session. Getting around (tabs, home, panes, what needs you) is the
    /// direct keys' (`keys`), so tab mode keeps only what they don't do. Keys after
    /// the one that hands the keyboard back go to the session.
    async fn tab_keys(&mut self, mut keys: &[u8]) {
        let toggle = self.settings.keys.get(Action::Tabs).map(Key::bytes);
        while self.tab_mode && !keys.is_empty() {
            // Tab mode's own key gives the keyboard back.
            if let Some(toggle) = &toggle
                && keys.starts_with(toggle)
            {
                keys = &keys[toggle.len()..];
                self.tab_mode = false;
                continue;
            }
            // The capture key works here too, so a thought isn't lost to the strip.
            if let Some((Action::Capture, len)) = self.settings.keys.at(keys) {
                self.tab_mode = false;
                self.on_direct(Action::Capture).await;
                return self.backlog_keys(&keys[len..]).await;
            }
            let key = next_key(keys);
            // A lone Esc is a key; Esc with more after it is Alt plus that key.
            let whole = key.len() == keys.len();
            keys = &keys[key.len()..];
            match key {
                b"," => {
                    self.tab_mode = false;
                    self.settings_cursor = Some(0);
                    return self.settings_keys(keys);
                }
                // The strip stays in tab mode, so you see the change and can undo it.
                b"m" => self.change(Field::TabDetail, 1),
                b"r" => {
                    if let Some(view) = &self.view {
                        let name = self
                            .sessions
                            .iter()
                            .find(|s| s.id == view.id)
                            .map_or_else(|| view.name.clone(), |s| s.name.clone());
                        self.renaming = Some((view.id, edit::LineEdit::new(name)));
                        self.tab_mode = false;
                        // What follows `r` in the same read is the new name.
                        return Box::pin(self.rename_keys(keys)).await;
                    }
                }
                // Splits of the focused pane, vim's way round: `s` below, `v` beside.
                b"s" | b"v" | b"S" | b"V" => {
                    self.tab_mode = false;
                    let side = match key {
                        b"s" => Side::Down,
                        b"v" => Side::Right,
                        b"S" => Side::Up,
                        _ => Side::Left,
                    };
                    self.split(side).await;
                }
                b"z" => {
                    self.tab_mode = false;
                    self.toggle_zoom();
                }
                // The docs this session wrote, beside it; again, they close.
                b"D" => {
                    self.tab_mode = false;
                    if self.beside_shown() {
                        self.close_beside();
                    } else {
                        self.open_beside(None).await;
                    }
                    return;
                }
                b"p" => {
                    self.tab_mode = false;
                    self.open_focused_pane_menu();
                    return Box::pin(self.menu_keys(keys)).await;
                }
                b"b" => {
                    self.tab_mode = false;
                    match self.toast_proposal() {
                        Some(id) => self.open_backlog_at(id).await,
                        None => self.open_backlog().await,
                    }
                    return;
                }
                // The backlog proposal on the toast, answered without leaving.
                b"a" | b"x" => {
                    self.tab_mode = false;
                    self.answer_proposal(key == b"a").await;
                }
                b"d" => {
                    self.tab_mode = false;
                    if self.proposals.is_empty() {
                        if let Some(view) = &mut self.view {
                            view.notice("no decisions to review".into());
                        }
                    } else {
                        self.reviewing = Some(0);
                        return Box::pin(self.review_keys(keys)).await;
                    }
                }
                [DETACH_KEY] => {
                    self.tab_mode = false;
                    self.detach().await;
                    return;
                }
                // The key sheet, over the session; its keys are the sheet's.
                b"?" => {
                    self.tab_mode = false;
                    self.help = true;
                    return;
                }
                b"q" => self.tab_mode = false,
                b"\x1b" if whole => self.tab_mode = false,
                _ => {}
            }
        }
        if !keys.is_empty() && self.view.is_some() {
            Box::pin(self.on_keys(keys.to_vec())).await;
        }
    }

    /// Attaches to the next session that needs you, in queue order after this one,
    /// each once; when none is left, goes back to where the run started.
    async fn next_needing(&mut self) {
        let here = self.view.as_ref().map(|v| v.id);
        // A run goes on while you are where it left you; anywhere else starts one.
        let mut run = match self.needing.take() {
            Some(run) if Some(run.at) == here => run,
            _ => NeedingRun {
                from: here,
                at: SessionId::default(),
                shown: here.into_iter().collect(),
            },
        };
        let n = self.queue.len();
        let start = self
            .queue
            .iter()
            .position(|q| Some(q.session) == here)
            .map_or(0, |i| i + 1);
        let next = (0..n)
            .map(|k| self.queue[(start + k) % n].session)
            .find(|id| !run.shown.contains(id) && Some(*id) != run.from);
        match next {
            Some(id) => {
                self.switch_to(id).await;
                run.at = id;
                run.shown.push(id);
                self.needing = Some(run);
            }
            None if run.from != here => match run.from {
                Some(id) if self.sessions.iter().any(|s| s.id == id) => {
                    self.switch_to(id).await;
                    self.say("back where you were".into());
                }
                Some(_) => self.say("nothing else needs you".into()),
                None => self.detach().await,
            },
            None => self.say("nothing else needs you".into()),
        }
    }

    /// Keys while a name is being typed: Enter saves (blank: back to the directory's
    /// name), Esc cancels, and the rest edit it (`LineEdit::key`).
    async fn rename_keys(&mut self, mut keys: &[u8]) {
        let mut typed = Vec::new();
        while self.renaming.is_some() && !keys.is_empty() {
            let key = next_key(keys);
            let whole = key.len() == keys.len();
            keys = &keys[key.len()..];
            if key.len() == 1 && key[0] >= 0x20 && key[0] != 0x7f {
                typed.push(key[0]);
                continue;
            }
            let tabs = self.is_tabs_key(key);
            let Some((id, name)) = &mut self.renaming else {
                break;
            };
            name.insert(&String::from_utf8_lossy(&std::mem::take(&mut typed)));
            name.truncate(NAME_MAX);
            match key {
                b"\r" | b"\n" => {
                    let (id, name) = (*id, name.text().trim().to_string());
                    self.renaming = None;
                    let name = (!name.is_empty()).then_some(name);
                    if let Err(e) = self.client.rename(id, name).await {
                        self.say(format!("rename failed: {e:#}"));
                    }
                    self.refresh().await;
                }
                b"\x1b" if whole => self.renaming = None,
                [0x03] => self.renaming = None,
                _ if tabs => self.renaming = None,
                k => {
                    name.key(k);
                }
            }
        }
        if let Some((_, name)) = &mut self.renaming {
            name.insert(&String::from_utf8_lossy(&typed));
            // A name fits a tab.
            name.truncate(NAME_MAX);
        }
    }

    /// Bracketed paste on in the outer terminal while a backlog prompt is open, so a
    /// multi-line paste reaches it as one, at home too; after, the program's mode.
    fn sync_paste(&mut self) {
        // A backlog prompt, or a search being typed on any page (#273): a
        // pasted line break mustn't end it.
        let want = self.backlog.as_ref().is_some_and(|p| p.typing.is_some())
            || self.searching()
            || self.composing();
        let program = self.view.as_ref().is_some_and(|v| v.modes.bracketed_paste);
        let Some(final_byte) = paste_change(want, self.paste_forced, program) else {
            return;
        };
        self.paste_forced = want;
        let mut out = term::outer();
        let _ = write!(out, "\x1b[?2004{final_byte}");
        let _ = out.flush();
    }

    /// An attached session's bar, and a toast on the row over it.
    fn draw_session_bar(&self, frame: &mut ratatui::Frame, view: &Attached, body: Rect, bar: Rect) {
        self.bar_cols.set(bar.width);
        let panel = style::Style::new().bg(self.settings.theme.panel);
        // A toast takes its own row over the bar for its few seconds, never the
        // bar's own line, where the hints would push it off (rule 9, backlog #203).
        if let Some(toast) = self.toast()
            && bar.y > body.y
        {
            let row = Rect::new(body.x, bar.y - 1, body.width, 1);
            frame.render_widget(ratatui::widgets::Clear, row);
            frame.render_widget(
                Paragraph::new(Line::from(self.toast_line(toast))).style(panel),
                row,
            );
        }
        frame.render_widget(Paragraph::new(self.bar(view).line).style(panel), bar);
    }

    /// Over a section's page, the row above its keys: a toast, else what a key
    /// said, as at home (rule 9: never the hint line).
    fn draw_said(&self, frame: &mut ratatui::Frame) {
        // Over a section's page, a toast takes the row above its keys, as at
        // home (rule 9: never the hint line).
        let t = self.settings.theme;
        let area = frame.area();
        // Not over the backlog's own dialog (the launch form).
        let dialog = self
            .backlog
            .as_ref()
            .is_some_and(|p| matches!(p.typing, Some(backlog::Typing::Launch(_))));
        // What a key said goes there too: a refusal, or where an editor's
        // text was kept, is seen on the page that said it (backlog #277).
        let said = match self.toast() {
            Some(toast) => Some(Line::from(self.toast_line(toast))),
            None if !self.status.is_empty() => {
                Some(Line::from(format!(" {} ", self.status).fg(t.fg).bold()))
            }
            None => None,
        };
        if self.view.is_none()
            && self.page_open()
            && !dialog
            && area.height > 2
            && let Some(said) = said
        {
            let row = Rect::new(area.x, area.bottom() - 2, area.width, 1);
            frame.render_widget(ratatui::widgets::Clear, row);
            frame.render_widget(
                Paragraph::new(said).style(style::Style::new().bg(t.panel)),
                row,
            );
        }
    }

    /// A section's page is over home: Backlog, Memory, History, or one of the
    /// pages reached by a key (Docs, Wiki, PRs, the supervisor).
    fn page_open(&self) -> bool {
        self.backlog.is_some()
            || self.memory.is_some()
            || self.digest.is_some()
            || self.docs.is_some()
            || self.wiki.is_some()
            || self.prs.is_some()
            || self.supervisor.is_some()
    }

    /// A page's `/` search is being typed.
    fn searching(&self) -> bool {
        self.backlog.as_ref().is_some_and(|p| p.search.typing)
            || self.memory.as_ref().is_some_and(|p| p.search.typing)
            || self.docs.as_ref().is_some_and(|p| p.search.typing)
            || self.wiki.as_ref().is_some_and(|p| p.search.typing)
            || self.prs.as_ref().is_some_and(|p| p.search.typing)
            || self.supervisor.as_ref().is_some_and(|p| p.search.typing)
    }

    /// The program's modes as the outer terminal should have them: a backlog prompt
    /// keeps bracketed paste on (`sync_paste`).
    fn outer_modes(&self, m: Modes) -> Modes {
        forced_modes(m, self.paste_forced)
    }

    /// Back to a plain shell's modes; `sync_paste` turns paste back on if a prompt
    /// is still open.
    fn reset_outer_modes(&mut self) {
        let _ = reset_terminal_modes();
        self.paste_forced = false;
        self.home_captured = false;
    }

    /// A Mac's Option typed a character where an Alt key was meant (the terminal
    /// doesn't send Option as Meta): says where to change that, until an Alt key has
    /// come through. The character still goes to the program.
    fn option_hint(&mut self, bytes: &[u8]) {
        // Alt keys are two bytes, Esc and the key; control keys one.
        if let Some((_, 2)) = self.settings.keys.at(bytes) {
            self.alt_works = true;
        }
        let meant = keys::option_typed(bytes).and_then(|k| self.settings.keys.action(Key::Alt(k)));
        if self.alt_works || meant.is_none() {
            return;
        }
        let fix = keys::option_fix(|var| std::env::var(var).ok());
        let text = format!("⌥ isn't alt in this terminal · {fix}");
        match &mut self.view {
            Some(view) => view.notice_for(text, OPTION_HINT_FOR),
            None => self.status = text,
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

    /// The composer has the keys: typing, on its row, at home.
    fn composing(&self) -> bool {
        self.composer.typing && self.view.is_none() && self.row() == Some(InboxRow::Compose)
    }

    /// The composer, typing: its row selected, `with` the draft (`#`).
    async fn compose_focus(&mut self, with: &str) {
        self.cursor_moved = true;
        self.selected = 0;
        self.home_detail = false;
        self.composer.focus(with);
        self.sync_paste();
        // The pickers' list, for tab; a slow daemon leaves the last one.
        self.load_repos().await;
        self.update_preview().await;
    }

    /// The repositories tab steps through, and what ↩ starts with.
    fn compose_defaults(&self) -> (Vec<PathBuf>, compose::Defaults) {
        let known: Vec<PathBuf> = self
            .repo_list
            .repos
            .iter()
            .map(|r| r.path.clone())
            .collect();
        let repos = compose::repos(&known, &self.items);
        let d = compose::defaults(&self.composer, &repos, &self.items);
        (repos, d)
    }

    /// The `#` search's matches.
    fn compose_found(&self) -> Vec<compose::Found> {
        match self.composer.item_query() {
            Some(q) => compose::find(q, &self.items, &self.sessions, self.boot()),
            None => Vec::new(),
        }
    }

    /// Keys while the composer is typing: they're the draft's, not the Inbox's.
    async fn compose_keys(&mut self, bytes: &[u8]) {
        for did in self.composer.keys(bytes) {
            match did {
                compose::Composed::Kept => {}
                compose::Composed::Done => self.sync_paste(),
                compose::Composed::Repo => {
                    let (repos, d) = self.compose_defaults();
                    self.composer.repo = Some(compose::step_repo(&d, &repos));
                }
                compose::Composed::Agent => {
                    let (_, d) = self.compose_defaults();
                    self.composer.agent = Some(compose::step_agent(&d));
                }
                compose::Composed::Start if self.composer.item_query().is_some() => {
                    self.compose_pick().await
                }
                // Several lines: one session each, checked first (#280).
                compose::Composed::Start if self.composer.task().contains('\n') => {
                    self.open_batch();
                    return;
                }
                compose::Composed::Start => self.compose_start().await,
                compose::Composed::File => self.compose_file().await,
            }
            // A start attached: the rest of the read is the session's.
            if self.view.is_some() {
                return;
            }
        }
        let found = self.compose_found().len();
        self.composer.pick = self.composer.pick.min(found.saturating_sub(1));
    }

    /// ↩ on a draft of several lines: the Start agents dialog, a row each.
    fn open_batch(&mut self) {
        let (repos, d) = self.compose_defaults();
        let b = batch::Batch::new(self.composer.task(), &d, &repos);
        if b.rows.is_empty() {
            return;
        }
        self.batch = Some(b);
        self.composer.typing = false;
        self.sync_paste();
    }

    /// Keys while Start agents is open: the dialog's alone.
    async fn batch_keys(&mut self, bytes: &[u8]) {
        let (repos, _) = self.compose_defaults();
        let Some(b) = &mut self.batch else { return };
        for did in b.keys(bytes, &repos) {
            match did {
                // Back to the draft, as the rows now have it, typing again.
                batch::Did::Back => {
                    if let Some(b) = self.batch.take()
                        && !b.rows.is_empty()
                    {
                        self.composer.text = b.draft();
                    }
                    self.composer.typing = true;
                    self.sync_paste();
                }
                batch::Did::Start => self.start_batch().await,
            }
        }
    }

    /// ↩ in Start agents: a session for every row, each in a worktree of its
    /// own. Started rows leave the dialog; one that failed stays, with why,
    /// to change and try again. Nothing attaches: they're several.
    async fn start_batch(&mut self) {
        let Some(mut b) = self.batch.take() else {
            return;
        };
        let size = self.session_size().unwrap_or(Size { cols: 80, rows: 24 });
        let mut started: Vec<String> = Vec::new();
        let mut left = Vec::new();
        for mut row in b.rows.drain(..) {
            let spec = compose::spawn_spec(&row.defaults(), &row.task, size, spawn_env());
            match self.client.spawn(spec).await {
                Ok(info) => started.push(info.name),
                Err(e) => {
                    row.failed = Some(format!("{e:#}"));
                    left.push(row);
                }
            }
        }
        self.refresh().await;
        let n = started.len();
        let said = format!("started {n} session{}", if n == 1 { "" } else { "s" });
        if left.is_empty() {
            self.composer.clear();
            self.say(said);
            return;
        }
        b.rows = left;
        b.cursor = 0;
        let failed = b.rows.len();
        self.batch = Some(b);
        self.say(format!(
            "{said}, {failed} couldn't start: tab or shift-tab, then ↩"
        ));
    }

    /// ↩: starts the agent on the task in a worktree of its own and attaches
    /// (a shell starts in the repository itself).
    async fn compose_start(&mut self) {
        let (_, d) = self.compose_defaults();
        let size = self.session_size().unwrap_or(Size { cols: 80, rows: 24 });
        let spec = compose::spawn_spec(&d, self.composer.task(), size, spawn_env());
        match self.client.spawn(spec).await {
            Ok(info) => {
                self.composer.clear();
                self.composer.typing = false;
                self.sync_paste();
                self.refresh().await;
                self.attach(info.id).await;
            }
            Err(e) => self.say(format!("couldn't start it: {e:#}")),
        }
    }

    /// ^f: the draft as a backlog item in the repository shown. Typed by you,
    /// so a real item, not a proposal.
    async fn compose_file(&mut self) {
        let task = self.composer.task().to_owned();
        if task.is_empty() || self.composer.item_query().is_some() {
            return self.say("type a task to file it".into());
        }
        let (_, d) = self.compose_defaults();
        let (title, body) = compose::as_item(&task);
        let item = valkyrie_proto::NewItem {
            title,
            body,
            cwd: d.repo,
            ..Default::default()
        };
        match self.client.add_item(item).await {
            Ok(item) => {
                self.composer.clear();
                self.composer.typing = false;
                self.sync_paste();
                self.say(format!("filed {}: {}", compose::label(&item), item.title));
            }
            Err(e) => self.say(format!("couldn't file it: {e:#}")),
        }
    }

    /// ↩ on a `#` match: a ready item starts, one with a session opens it.
    async fn compose_pick(&mut self) {
        let found = self.compose_found();
        let Some(f) = found.get(self.composer.pick).cloned() else {
            return self.say("no open item matches".into());
        };
        match f.what {
            compose::Can::Start => self.compose_launch(f.id).await,
            compose::Can::Open(id, _) => {
                self.composer.clear();
                self.composer.typing = false;
                self.sync_paste();
                self.attach(id).await;
            }
            compose::Can::No(why) => self.say(format!("{} is {why}", f.label)),
        }
    }

    /// `s` on the composer: the top ready item, without typing.
    async fn start_ready(&mut self) {
        let top = compose::ready(&self.items, &self.sessions, self.boot())
            .first()
            .map(|i| i.id);
        match top {
            Some(id) => self.compose_launch(id).await,
            None => self.say("nothing ready in the backlog".into()),
        }
    }

    /// Starts backlog item `id` as the backlog's ↩ does, and attaches.
    async fn compose_launch(&mut self, id: u32) {
        let Some(item) = self.items.iter().find(|i| i.id == id) else {
            return;
        };
        let agent = backlog::last_agent(item);
        let size = self.session_size().unwrap_or(Size { cols: 80, rows: 24 });
        match self.client.launch_item(id, agent, true, size, None).await {
            Ok(info) => {
                self.composer.clear();
                self.composer.typing = false;
                self.sync_paste();
                self.refresh().await;
                self.attach(info.id).await;
            }
            Err(e) => self.say(format!("couldn't start it: {e:#}")),
        }
    }

    /// The supervisor at a glance, for the composer's last line.
    async fn load_glance(&mut self) {
        let watch = tokio::time::timeout(PREVIEW_WAIT, self.client.supervisor_watch()).await;
        let log = tokio::time::timeout(PREVIEW_WAIT, self.client.supervisor_log()).await;
        if let Ok(Ok(w)) = watch {
            self.glance = Some(Glance {
                on: w.on,
                watching: w.prs.len(),
                last: match log {
                    Ok(Ok((entries, _))) => entries.into_iter().max_by_key(|e| e.at),
                    _ => self.glance.as_ref().and_then(|g| g.last.clone()),
                },
            });
        }
    }

    /// Starts `$SHELL` (in `cwd`, else the daemon's default) and attaches to it.
    async fn new_shell(&mut self, cwd: Option<PathBuf>) {
        let shell = valkyrie_proto::default_shell();
        let size = self.session_size().unwrap_or(Size { cols: 80, rows: 24 });
        let spec = SpawnSpec {
            command: vec![shell],
            cwd,
            name: None,
            size,
            env: spawn_env(),
            worktree: None,
            hidden: false,
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
    /// double click renames, a right click opens the tab's menu.
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
                self.tab_mode = false;
                self.detach().await;
            }
            (MouseKind::Press, Some(TabHit::Session(id))) => {
                self.tab_mode = false;
                // Terminals don't report double clicks: two presses close together are one.
                let double = self
                    .tab_press
                    .is_some_and(|(last, at)| last == id && at.elapsed() < DOUBLE_CLICK);
                self.tab_press = (!double).then(|| (id, Instant::now()));
                if double {
                    return self.start_rename(id);
                }
                // A click on the attached tab keeps the pane you were in.
                if !self.pane_ids().contains(&id) {
                    self.switch_to(id).await;
                }
            }
            (MouseKind::Press, Some(TabHit::New)) => {
                self.tab_mode = false;
                self.new_shell(self.attached_cwd()).await;
            }
            (MouseKind::RightPress, Some(TabHit::Session(id))) => {
                self.tab_mode = false;
                self.open_menu(id, MENU_RENAME);
            }
            _ => {}
        }
    }

    /// The bar under `view`, the attached session.
    fn bar(&self, view: &Attached) -> Bar {
        let mode = if self.renaming.is_some() {
            BarMode::Renaming
        } else if self.menu.is_some() {
            BarMode::Menu
        } else if self.reviewing.is_some() {
            BarMode::Reviewing
        } else if self.tab_mode {
            BarMode::Tabs
        } else {
            BarMode::Session
        };
        attached_bar(
            view,
            &self.place_of(view.id),
            self.bar_panes(view.id),
            mode,
            self.need_you(),
            self.settings.hints,
            self.bar_cols.get(),
            self.settings.theme,
            &self.settings.keys,
        )
    }

    /// Where a session works, as its bar says it: `api · fix/login · #4` (its
    /// repository, then its backlog item's branch and number, if it's on one).
    fn place_of(&self, id: SessionId) -> String {
        let mut place = Vec::new();
        if let Some(s) = self.sessions.iter().find(|s| s.id == id) {
            place.extend(s.cwd.file_name().map(|n| n.to_string_lossy().into_owned()));
        }
        let item = self.items.iter().find(|i| {
            i.boot == self.boot()
                && i.sessions.contains(&id)
                && matches!(
                    i.status,
                    valkyrie_proto::ItemStatus::Open | valkyrie_proto::ItemStatus::Proposed
                )
        });
        if let Some(item) = item {
            place.extend(item.branch.clone());
            place.push(format!("#{}", item.number));
        }
        place.join(" · ")
    }

    /// Where `id` is among the attached tab's panes (from 1), how many there are,
    /// and whether it is zoomed; `None` for a tab of one pane.
    fn bar_panes(&self, id: SessionId) -> Option<BarPanes> {
        let order = self.layout.as_ref()?.sessions();
        let at = order.iter().position(|&p| p == id)?;
        Some(BarPanes {
            at: at + 1,
            of: order.len(),
            zoomed: self.is_zoomed(),
        })
    }

    /// A click on the bar under a session: a key's chip does what the key does, and
    /// `◇ N to review` lists the decisions.
    async fn bar_click(&mut self, x: u16) {
        let Some(view) = &self.view else { return };
        let bar = self.bar(view);
        let Some(&(_, _, click)) = bar
            .clicks
            .iter()
            .find(|&&(from, to, _)| (from..to).contains(&x))
        else {
            return;
        };
        match click {
            BarClick::Action(Action::Tabs) => {
                self.tab_mode = true;
            }
            BarClick::Action(action) => self.on_direct(action).await,
            BarClick::Help => self.help = true,
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
        if let Err(e) = self.client.move_session(id, to).await {
            self.say(format!("move failed: {e:#}"));
        }
    }

    /// Opens a tab's menu under the tab (beside it, for a column), the cursor on
    /// `item`.
    fn open_menu(&mut self, id: SessionId, item: usize) {
        let tabs = self.tabs_area().unwrap_or(Rect::new(
            0,
            0,
            MENU_WIDTH,
            self.settings.tabs.rows(self.settings.minimal_tabs),
        ));
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
                b"q" => self.menu = None,
                k if self.is_tabs_key(k) => self.menu = None,
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

    /// Opens the rename prompt on a tab, starting from its name.
    fn start_rename(&mut self, id: SessionId) {
        let name = self
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone());
        self.renaming = Some((id, edit::LineEdit::new(name.unwrap_or_default())));
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
            return self.start_rename(id);
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
        // Your editor on a doc: back to the doc, saved.
        if let Some(e) = self.editing.as_mut().filter(|e| e.session == id) {
            // It ended, so it ran.
            e.seen = true;
            if self.view.as_ref().is_some_and(|v| v.id == id) {
                self.detach().await;
            }
            return self.finish_edit().await;
        }
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

    /// Asks, off the UI thread, whether a newer release is out, unless the Updates
    /// setting is off or an answer is already coming.
    fn look_for_updates(&mut self) {
        let Some(&check) = UPDATE_CHECK.get() else {
            return;
        };
        if !self.settings.updates || self.newer_rx.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(check());
        });
        self.newer_rx = Some(rx);
    }

    fn shipping_tx(&self) -> Result<mpsc::UnboundedSender<Shipping>> {
        self.shipping
            .clone()
            .ok_or_else(|| anyhow::anyhow!("not ready to ship yet"))
    }

    /// What the backlog's `S` found, or shipped. Found, it waits on a second `S`
    /// to ship exactly what was shown.
    fn on_shipping(&mut self, done: Shipping) {
        self.ship_busy = false;
        let (shipped, kept) = match done.result {
            Ok(found) => found,
            Err(e) => return self.say(e),
        };
        let open = self.backlog.is_some();
        // Asking, a filtered page offers only the items it shows.
        let (shipped, kept) = match self.backlog.as_ref() {
            Some(page) if done.asking => {
                let shown = |id: u32| self.items.iter().any(|i| i.id == id && page.holds(i));
                (
                    shipped.into_iter().filter(|s| shown(s.id)).collect(),
                    kept.into_iter().filter(|k| shown(k.id)).collect(),
                )
            }
            _ => (shipped, kept),
        };
        let (arm, said) = backlog::answer(done.asking, open, &shipped, &kept, done.unfetched);
        if let (Some(ids), Some(page)) = (arm, &mut self.backlog) {
            page.ship_armed = Some(ids);
        }
        self.say(said);
    }

    /// Steps a setting, keeps it, and lays the screen out again if the tabs moved.
    fn change(&mut self, field: Field, by: isize) {
        let before = self.settings;
        self.settings.cycle(field, by);
        self.settings.save_field(field, &self.settings_file);
        if field == Field::Context {
            settings::context_changed(self.settings.context);
        }
        if field == Field::Updates {
            self.look_for_updates();
        }
        if matches!(field, Field::Theme | Field::SessionColors) {
            self.report_colors(None);
            for id in self.pane_ids() {
                self.report_colors(Some(id));
            }
        }
        // So you hear what you picked.
        if field == Field::Sound && self.settings.sound {
            ping::play(self.settings.noise, Kind::Request);
        }
        let strip = |s: &Settings| (s.tabs, s.tab_side, s.minimal_tabs);
        if strip(&before) != strip(&self.settings) {
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
                b"." => {
                    self.settings_cursor = None;
                    self.keys_page = Some(KeysPage::default());
                    return;
                }
                _ => {}
            }
        }
    }

    /// Keys while the keys page is open: ↑/↓ pick an action; Enter waits for the
    /// key to give it (Esc gives up); Backspace clears it; `r` puts back its
    /// default and `R`, pressed twice, every default; Esc (or `q`) closes.
    fn keys_page_keys(&mut self, mut keys: &[u8]) {
        while let Some(page) = &mut self.keys_page
            && !keys.is_empty()
        {
            let action = Action::ALL[page.cursor];
            if page.capturing {
                // The whole read is the key: Alt sends Esc and it at once.
                page.capturing = false;
                if keys == b"\x1b" {
                    return;
                }
                // A mouse report, an arrow, a paste: not a key to map; keep waiting.
                if keys.starts_with(b"\x1b[") || keys.starts_with(b"\x1bO") {
                    page.capturing = true;
                    return;
                }
                match Key::from_bytes(keys) {
                    Some(key) => self.rebind(action, Some(key)),
                    None => self.say(
                        "that key stays the program's: try Alt and a key, or Ctrl and a letter"
                            .into(),
                    ),
                }
                return;
            }
            let key = next_key(keys);
            let whole = key.len() == keys.len();
            keys = &keys[key.len()..];
            let last = Action::ALL.len() - 1;
            if key != b"R" {
                page.reset_armed = false;
            }
            match key {
                b"k" | b"\x1b[A" | b"\x1bOA" => page.cursor = page.cursor.saturating_sub(1),
                b"j" | b"\x1b[B" | b"\x1bOB" => page.cursor = (page.cursor + 1).min(last),
                b"\r" | b"\n" | b" " => {
                    page.capturing = true;
                    let text = format!("press the key for {}", action.label().to_lowercase());
                    self.say(text);
                }
                [0x7f] | [0x08] | b"\x1b[3~" => self.rebind(action, None),
                b"r" => {
                    let had = self.settings.keys.reset(action);
                    self.keys_changed(action, had);
                }
                b"R" if page.reset_armed => {
                    page.reset_armed = false;
                    self.settings.keys = Keymap::default();
                    self.settings.save_keys(&self.settings_file);
                    self.say("every key is back to its default".into());
                }
                b"R" => {
                    page.reset_armed = true;
                    self.say("R again puts every key back to its default".into());
                }
                b"\x1b" if whole => self.keys_page = None,
                b"q" | b"." | [0x03] => self.keys_page = None,
                _ => {}
            }
        }
    }

    fn rebind(&mut self, action: Action, key: Option<Key>) {
        let had = self.settings.keys.bind(action, key);
        self.keys_changed(action, had);
    }

    /// Saves the map and says what changed, and what lost its key to it.
    fn keys_changed(&mut self, action: Action, had: Option<Action>) {
        self.settings.save_keys(&self.settings_file);
        let key = self
            .settings
            .keys
            .get(action)
            .map_or("no key".into(), Key::label);
        let mut text = format!("{}: {key}", action.label().to_lowercase());
        if let Some(had) = had {
            text += &format!(
                " (taken from {}, which has none now)",
                had.label().to_lowercase()
            );
        }
        self.say(text);
    }

    async fn on_mouse(&mut self, m: Mouse) {
        if self.menu.is_some() {
            return self.menu_click(m).await;
        }
        if m.kind == MouseKind::Press && self.screen().is_ok_and(|s| m.y == s.rows) {
            return self.bar_click(m.x).await;
        }
        let tabs = self.tabs_area();
        let on_strip = tabs.filter(|tabs| tabs.contains(Position::new(m.x, m.y)));
        // On the rail it opens; anywhere else it shuts.
        if self.rail() {
            self.rail_hover = on_strip.is_some();
        }
        // Resting on a tab peeks at it, after a moment, so a quick click doesn't
        // flash one; anything else, or anywhere else, ends that.
        let hover = on_strip.filter(|_| m.is_motion()).and_then(|tabs| {
            match self.tab_hit(tabs, m.x, m.y) {
                Some(TabHit::Session(id)) => self.tab_of(id),
                _ => None,
            }
        });
        if hover != self.hover_tab.map(|(i, _)| i) {
            self.hover_tab = hover.map(|i| (i, Instant::now()));
        }
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
        // Over the docs beside the session: theirs.
        if let Some(area) = self.beside_area()
            && area.contains(Position::new(m.x, m.y))
        {
            self.docs_on = DocsOn::Beside;
            return self.docs_mouse(m).await;
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
                    // A click, not a drag: on a link, it opens.
                    view.selection = None;
                    let (id, url) = (view.id, links::url_at(view.shown(), view.cols, at.0, at.1));
                    if let Some(url) = url {
                        self.open_link(id, &url);
                    } else if let Some(doc) = links::doc_at(view.shown(), at.0, at.1) {
                        // A doc it mentions, read beside it (backlog #311).
                        self.open_beside(Some(doc.to_string())).await;
                    }
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

    /// Opens `url` from pane `id`, and says so in its bar.
    fn open_link(&mut self, id: SessionId, url: &str) {
        let said = match links::open(url) {
            Ok(links::Opened::Browser) => format!("opened {url}"),
            Ok(links::Opened::Copied) => format!("copied {url} (no browser over SSH)"),
            Err(e) => format!("couldn't open {url}: {e}"),
        };
        if let Some(pane) = self.pane_mut(id) {
            pane.notice(said);
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
        self.inbox_rows().len()
    }

    /// The Inbox's rows in the order they're drawn: the sessions waiting on you,
    /// one row for what's up for a decision, one for the items agents filed, then
    /// the rest of the sessions, Running before Quiet (spec: Inbox).
    fn inbox_rows(&self) -> Vec<InboxRow> {
        let mut rows = vec![InboxRow::Compose];
        // First: with the disk full, nothing else gets done.
        if !self.disk.is_empty() {
            rows.push(InboxRow::Disk);
        }
        rows.extend((0..self.queue.len()).map(InboxRow::Queue));
        if self.to_decide() > 0 {
            rows.push(InboxRow::Decide);
        }
        if !self.filed().is_empty() {
            rows.push(InboxRow::Filed);
        }
        rows.extend(self.others().iter().map(|s| InboxRow::Session(s.id)));
        rows
    }

    /// The section on screen: the one whose page is open, else the Inbox.
    fn section(&self) -> Section {
        if self.backlog.is_some() || self.prs.is_some() {
            Section::Backlog
        } else if self.docs_shown() || self.wiki.is_some() || self.memory.is_some() {
            Section::Memory
        } else if self.digest.is_some() {
            Section::History
        } else {
            Section::Inbox
        }
    }

    /// Whether `g` and `[ ]` are the sections' now: no dialog or prompt over home,
    /// and the section's page on its list, not typing or reading.
    fn sections_listen(&self) -> bool {
        self.view.is_none()
            && self.keys_page.is_none()
            && self.settings_cursor.is_none()
            && self.renaming.is_none()
            && self.reviewing.is_none()
            && !self.composing()
            && self.batch.is_none()
            && self.supervisor.is_none()
            && self.backlog.as_ref().is_none_or(backlog::Page::at_list)
            && self.docs.as_ref().is_none_or(docs::Page::at_list)
            && self.wiki.as_ref().is_none_or(wiki::Page::at_list)
            && self.memory.as_ref().is_none_or(memory::Page::at_list)
            && self.digest.as_ref().is_none_or(digest::Page::at_list)
            && self.prs.as_ref().is_none_or(prs::Page::at_list)
    }

    /// `g i/b/m/h` go to a section and `[ ]` cycle the section's tabs, before
    /// the page has the keys; `g g` is the page's own `g` (to the top). Returns
    /// the keys left for the page, from the first one that isn't these.
    async fn section_keys(&mut self, bytes: Vec<u8>) -> Option<Vec<u8>> {
        if !self.sections_listen() {
            self.drop_g();
        }
        let mut rest: &[u8] = &bytes;
        while !rest.is_empty() && self.sections_listen() {
            let key = next_key(rest);
            if std::mem::take(&mut self.g_pending) {
                self.status.clear();
                match Section::ALL.iter().find(|s| s.key() == key) {
                    Some(&section) => self.go_section(section).await,
                    // `g g`, or `g` then anything: the page's `g`, then the key.
                    None => {
                        let mut out = b"g".to_vec();
                        if key != b"g" {
                            out.extend_from_slice(rest);
                        } else {
                            out.extend_from_slice(&rest[key.len()..]);
                        }
                        return Some(out);
                    }
                }
            } else if key == b"g" {
                self.g_pending = true;
                self.status = "g: i inbox · b backlog · m memory · h history".into();
            } else if (key == b"[" || key == b"]")
                && matches!(self.section(), Section::Memory | Section::Backlog)
            {
                self.cycle_tab(key == b"]").await;
            } else {
                return Some(rest.to_vec());
            }
            rest = &rest[key.len()..];
        }
        (!rest.is_empty()).then(|| rest.to_vec())
    }

    /// Keys over the `?` sheet: `t` starts the tutor; Esc, `q` or `?` close it.
    async fn help_keys(&mut self, bytes: &[u8]) {
        match next_key(bytes) {
            b"t" => {
                self.help = false;
                self.start_tutor().await;
            }
            b"\x1b" | b"q" | b"?" | [0x03] => self.help = false,
            _ => {}
        }
    }

    /// The `?` sheet: every key, by what it does on every screen (spec: One key
    /// grammar), the old keys that still work, and the tutor at its foot. A
    /// dialog: what's behind it dims.
    fn draw_help(&self, frame: &mut ratatui::Frame) {
        let t = self.settings.theme;
        let area = frame.area();
        dim(frame, area, t);
        let rows: &[(&str, &str)] = &[
            ("j k · ↩ · esc", "move · open or attach · back"),
            ("tab", "the next thing that waits on you (alt n attached)"),
            ("1-9", "answer the selected ask with that choice"),
            ("a · d", "accept or allow · reject or deny"),
            ("x", "close, after a second x"),
            ("r", "rename"),
            ("n · s", "new session, typed · start the top ready item"),
            ("# · ^f", "a backlog item to start · file what you typed"),
            ("/ · [ ]", "search · cycle a section's tabs or filter"),
            (
                "g i · g b · g m · g h",
                "Inbox · Backlog · Memory · History",
            ),
            (", · ?", "settings (. there: the keys) · this sheet"),
            ("alt h · ctrl ]", "home, from a session"),
            ("q", "quit"),
        ];
        let mut lines = vec![Line::default()];
        for (keys, does) in rows {
            lines.push(Line::from(vec![
                format!("  {keys:<22}").fg(t.accent).bold(),
                does.to_string().fg(t.secondary()),
            ]));
        }
        lines.push(Line::default());
        lines.push(Line::from(
            "  Still work for now: b backlog · D docs · W wiki · w h history · s supervisor · . keys"
                .fg(t.muted),
        ));
        lines.push(Line::default());
        lines.push(Line::from(vec![
            "  t".fg(t.accent).bold(),
            " starts the tutor, a walk through all of it".fg(t.secondary()),
        ]));
        let width = 78.min(area.width);
        let height = (lines.len() as u16 + 3).min(area.height);
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        frame.render_widget(ratatui::widgets::Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).block(
                panel(Line::from(" Every key ".fg(t.fg).bold()), t, true).title_bottom(
                    Line::from(vec![" esc ".fg(t.accent).bold(), "close ".fg(t.dim())]).centered(),
                ),
            ),
            rect,
        );
    }

    /// `^\`'s panel over a session: what each key does there, bottom right, the
    /// session dimmed behind (spec: Attached). Only the keys tab mode has.
    fn draw_which_key(&self, frame: &mut ratatui::Frame, main: Rect) {
        let t = self.settings.theme;
        let rows: &[(&str, &str)] = &[
            ("s · v", "split below · beside"),
            ("z", "zoom"),
            ("p", "pane menu"),
            ("r", "rename"),
            ("b", "backlog"),
            ("d", "decisions"),
            ("D", "its docs, beside it"),
            ("m", "minimal tabs"),
            (",", "settings"),
            ("?", "every key"),
        ];
        let mut lines: Vec<Line> = rows
            .iter()
            .map(|(k, w)| {
                Line::from(vec![
                    format!(" {k:<6}").fg(t.accent).bold(),
                    w.to_string().fg(t.secondary()),
                ])
            })
            .collect();
        lines.push(Line::default());
        lines.push(Line::from(vec![
            " esc ".fg(t.accent).bold(),
            "back".fg(t.dim()),
        ]));
        let width = 34.min(main.width);
        let height = (lines.len() as u16 + 2).min(main.height);
        let rect = Rect::new(
            main.right().saturating_sub(width + 1),
            main.bottom().saturating_sub(height),
            width,
            height,
        )
        .intersection(main);
        frame.render_widget(ratatui::widgets::Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).block(panel(Line::from(" actions ".fg(t.fg).bold()), t, true)),
            rect,
        );
    }

    /// Forgets a pending `g`, and the hint it put up.
    fn drop_g(&mut self) {
        if std::mem::take(&mut self.g_pending) {
            self.status.clear();
        }
    }

    /// Opens `section`'s page in place of whatever section is open (its first
    /// tab), or closes them all for the Inbox.
    async fn go_section(&mut self, section: Section) {
        if section == self.section() {
            return;
        }
        self.backlog = None;
        self.prs = None;
        self.docs = None;
        self.wiki = None;
        self.memory = None;
        self.digest = None;
        match section {
            Section::Inbox => {}
            Section::Backlog => self.open_backlog().await,
            Section::Memory => self.open_memory(memory::Tab::Decisions).await,
            Section::History => self.open_digest().await,
        }
    }

    /// `]` (forward) or `[` in Memory (Decisions, Skills, Docs, Wiki, Notes) or
    /// Backlog (its filter row: All, Ready, Running, Review, then Pull
    /// requests): the next or last stop. History's are its window, which its
    /// page keeps.
    async fn cycle_tab(&mut self, forward: bool) {
        if self.section() == Section::Backlog {
            match backlog::Filter::step(self.backlog.as_ref().map(|p| p.filter), forward) {
                Some(filter) => {
                    if self.backlog.is_none() {
                        self.prs = None;
                        self.open_backlog().await;
                    }
                    if let Some(page) = &mut self.backlog {
                        page.filter = filter;
                        page.cursor = 0;
                        page.armed = None;
                    }
                }
                None => {
                    self.backlog = None;
                    self.open_prs().await;
                }
            }
            return;
        }
        let at = MEMORY_TABS
            .iter()
            .position(|&tab| tab == self.memory_tab())
            .unwrap_or(0);
        let n = MEMORY_TABS.len();
        let mut next = if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        };
        // The wiki is a repository's: with none to show, step past it.
        if MEMORY_TABS[next] == MemoryTab::Wiki && self.wiki_repo().is_none() {
            next = if forward {
                (next + 1) % n
            } else {
                (next + n - 1) % n
            };
        }
        self.docs = None;
        self.wiki = None;
        let kept = self.memory.take();
        match MEMORY_TABS[next] {
            MemoryTab::Docs => self.open_docs().await,
            MemoryTab::Wiki => self.open_wiki(self.selection().0).await,
            MemoryTab::Page(tab) => match kept {
                // The same data, another tab: no need to ask again.
                Some(mut page) => {
                    page.tab = tab;
                    self.memory = Some(page);
                }
                None => self.open_memory(tab).await,
            },
        }
    }

    /// Which of Memory's tabs is open.
    fn memory_tab(&self) -> MemoryTab {
        match &self.memory {
            Some(page) => MemoryTab::Page(page.tab),
            None if self.wiki.is_some() => MemoryTab::Wiki,
            None => MemoryTab::Docs,
        }
    }

    /// Section pages are live (#273): the open one loads again every
    /// `PAGE_RELOAD`, keeping its cursor, so there's no reload key. Backlog
    /// items and proposals come pushed. Not while a session is attached over it.
    async fn reload_page(&mut self) {
        self.page_loaded = Instant::now();
        if self.view.is_some() {
            return;
        }
        if self.memory.is_some() {
            self.load_memory().await;
        }
        if self.prs.is_some() {
            self.load_prs().await;
        }
        if self.supervisor.is_some() {
            self.load_supervisor().await;
        }
        if self.wiki.is_some() {
            self.load_wiki().await;
        }
        if self.digest.is_some() {
            self.fetch_digest(true).await;
        }
        if let Some(page) = &mut self.docs {
            let acts = page.refresh();
            self.fetch_docs(false).await;
            for act in acts {
                self.docs_on = DocsOn::Home;
                Box::pin(self.docs_do(act)).await;
            }
        }
        if let Some(b) = &mut self.beside {
            let acts = b.page.refresh();
            self.load_beside().await;
            for act in acts {
                self.docs_on = DocsOn::Beside;
                Box::pin(self.docs_do(act)).await;
            }
        }
        // Home, on the composer: its supervisor line and repositories.
        if self.section() == Section::Inbox && self.row() == Some(InboxRow::Compose) {
            self.load_glance().await;
            self.load_repos().await;
        }
    }

    async fn open_prs(&mut self) {
        // Just loaded: the next live reload is a full `PAGE_RELOAD` away.
        self.page_loaded = Instant::now();
        self.prs = Some(prs::Page::default());
        self.load_prs().await;
    }

    async fn load_prs(&mut self) {
        let got = self
            .client
            .supervisor_watch()
            .await
            .map_err(|e| format!("{e:#}"));
        if let Some(page) = &mut self.prs {
            page.loaded(got);
        }
    }

    async fn prs_keys(&mut self, bytes: &[u8]) {
        let Some(page) = &mut self.prs else { return };
        for act in page.keys(bytes) {
            match act {
                prs::Do::Close => self.prs = None,
                prs::Do::Open(url) => self.open_url(&url),
            }
        }
    }

    /// Item id to the number its repository calls it, for what shows item ids.
    fn item_numbers(&self) -> std::collections::HashMap<u32, u32> {
        self.items.iter().map(|i| (i.id, i.number)).collect()
    }

    async fn open_memory(&mut self, tab: memory::Tab) {
        // Just loaded: the next live reload is a full `PAGE_RELOAD` away.
        self.page_loaded = Instant::now();
        self.memory = Some(memory::Page::new(tab));
        self.load_memory().await;
    }

    /// Every project's decisions, and the notes of each repository the daemon
    /// knows or a decision names (notes are asked for one repository at a time).
    async fn load_memory(&mut self) {
        let got = async {
            let decisions = self.client.decisions(None).await?;
            let mut repos: Vec<PathBuf> = self
                .repo_list
                .repos
                .iter()
                .map(|r| r.path.clone())
                .collect();
            for d in &decisions {
                if d.project.as_os_str() != USER_SCOPE && !repos.contains(&d.project) {
                    repos.push(d.project.clone());
                }
            }
            let mut notes = Vec::new();
            let mut failed = 0;
            for repo in repos {
                // One repository's failure leaves the rest's notes, and says so.
                match self.client.notes(repo).await {
                    Ok(listing) => notes.extend(listing.notes),
                    Err(_) => failed += 1,
                }
            }
            anyhow::Ok((decisions, notes, failed))
        }
        .await
        .map_err(|e| format!("{e:#}"));
        if let Some(page) = &mut self.memory {
            match got {
                Ok((decisions, notes, failed)) => {
                    page.notes_failed = failed;
                    page.loaded(Ok((decisions, notes)));
                }
                Err(e) => page.loaded(Err(e)),
            }
        }
    }

    async fn memory_keys(&mut self, bytes: &[u8]) {
        let Some(page) = &mut self.memory else { return };
        for act in page.keys(bytes, &self.proposals) {
            match act {
                memory::Do::Close => self.memory = None,
                memory::Do::Review { index, accept } => {
                    self.last_answer = None;
                    if let Some(said) = self.review_at(index, accept).await {
                        self.status = said;
                    }
                    if let (Some(undo), Some(page)) = (self.last_answer.take(), &mut self.memory) {
                        page.undo = undo;
                    }
                    self.reload_memory_decisions().await;
                }
                memory::Do::Edit { index } => self.edit_proposal(index).await,
                memory::Do::Undo => self.undo_review().await,
            }
        }
    }

    /// After an answer: an accepted one joins its kind's group; the notes stay.
    async fn reload_memory_decisions(&mut self) {
        if self.memory.is_none() {
            return;
        }
        let got = self
            .client
            .decisions(None)
            .await
            .map_err(|e| format!("{e:#}"));
        if let Some(page) = &mut self.memory {
            page.decisions_loaded(got);
        }
    }

    /// `e` on a proposal: its words in your editor, accepted as you leave them
    /// (`accept_edited`).
    async fn edit_proposal(&mut self, i: usize) {
        let Some(d) = self.proposals.get(i) else {
            return;
        };
        // A flagged decision or a proposed scope is in use already: `a` or `d`.
        if d.status != valkyrie_proto::DecisionStatus::Proposed {
            return self.say(format!(
                "#{} is in use already: e rewrites a proposal",
                d.id
            ));
        }
        let was = memory::Wording::of(d);
        let (text, title) = (was.text(), d.title.clone());
        let what = Edited::Proposal {
            project: d.project.clone(),
            id: d.id,
            seen: d.updated,
            habit: memory::is_habit(d),
            was,
        };
        self.start_edit(what, text, title).await;
    }

    /// Accepts a proposal as you rewrote it. Your file goes only once it's
    /// accepted; otherwise the message says where it is.
    async fn accept_edited(&mut self, what: Edited, text: String, path: PathBuf) {
        let Edited::Proposal {
            project,
            id,
            seen,
            habit,
            was,
        } = what
        else {
            return;
        };
        let mut w = match memory::Wording::parse(&text, habit) {
            Ok(w) => w,
            Err(why) => {
                return self.say(format!(
                    "not accepted, yours is in {}: {why}",
                    path.display()
                ));
            }
        };
        // A scope sent settles it (ADR-0014 §2): only one you changed goes, and
        // only then does `z` put the old one back. No `scope:` line: left as is.
        let mut was = was;
        if w.scope.is_none() || w.scope == was.scope {
            w.scope = None;
            was.scope = None;
        }
        let action = ReviewAction::Revise {
            title: w.title,
            body: w.body,
            kind: w.kind,
            scope: w.scope,
        };
        match self
            .client
            .review(project.clone(), id, action, Some(seen))
            .await
        {
            Ok(d) => {
                let _ = std::fs::remove_file(&path);
                if let Some(page) = &mut self.memory {
                    page.undo = Some(memory::Undo {
                        project,
                        id,
                        title: d.title.clone(),
                        seen: d.updated,
                        how: memory::Reverse::Reopen(Some(was)),
                    });
                }
                self.say(format!("accepted #{id} {}, as you wrote it", d.title));
                self.reload_memory_decisions().await;
            }
            Err(e) => self.say(format!(
                "not accepted, yours is in {}: {e:#}",
                path.display()
            )),
        }
    }

    /// `z` on Memory: takes back the last answer given there. An accept or a
    /// reject goes back to review (in its own words, if you rewrote it on the
    /// way in); a retire comes back into use.
    async fn undo_review(&mut self) {
        let Some(u) = self.memory.as_mut().and_then(|p| p.undo.take()) else {
            return self.say("nothing to undo".into());
        };
        let id = u.id;
        let action = ReviewAction::Reopen;
        let said = match self
            .client
            .review(u.project.clone(), id, action, Some(u.seen))
            .await
        {
            Err(e) => format!("couldn't undo: {e:#}"),
            Ok(d) => match u.how {
                memory::Reverse::Reopen(None)
                    if d.status == valkyrie_proto::DecisionStatus::Active =>
                {
                    format!("#{id} {} is in use again", d.title)
                }
                memory::Reverse::Reopen(None) => {
                    format!("#{id} {} is up for review again", d.title)
                }
                memory::Reverse::Reopen(Some(was)) => {
                    let back = ReviewAction::Edit {
                        title: was.title,
                        body: was.body,
                        kind: was.kind,
                        scope: was.scope,
                    };
                    match self
                        .client
                        .review(u.project, id, back, Some(d.updated))
                        .await
                    {
                        Ok(d) => format!(
                            "#{id} {} is up for review again, as it was proposed",
                            d.title
                        ),
                        Err(e) => format!("#{id} is up for review again, but in your words: {e:#}"),
                    }
                }
            },
        };
        self.say(said);
        self.reload_memory_decisions().await;
    }

    /// Sets the open section page aside for the session it attaches to.
    fn park(&mut self) {
        // What was waiting on the daemon is answered by the attach itself.
        if let Some(p) = &mut self.wiki {
            p.starting = false;
        }
        if let Some(p) = &mut self.digest {
            p.resuming = false;
        }
        let page = if let Some(p) = self.backlog.take() {
            Some(Parked::Backlog(Box::new(p)))
        } else if let Some(p) = self.wiki.take() {
            Some(Parked::Wiki(Box::new(p)))
        } else {
            self.digest.take().map(|p| Parked::Digest(Box::new(p)))
        };
        if page.is_some() {
            self.parked = page;
        }
    }

    /// After an attach from a page: if it didn't attach after all, the page
    /// comes straight back rather than waiting to surface on a later detach.
    fn unpark_unless_attached(&mut self) {
        if self.view.is_none() {
            self.unpark();
        }
    }

    /// Back home: the page you attached from, as you left it, unless one is open.
    fn unpark(&mut self) {
        let open = self.backlog.is_some() || self.wiki.is_some() || self.digest.is_some();
        match self.parked.take() {
            _ if open => {}
            Some(Parked::Backlog(p)) => self.backlog = Some(*p),
            Some(Parked::Wiki(p)) => self.wiki = Some(*p),
            Some(Parked::Digest(p)) => self.digest = Some(*p),
            None => {}
        }
    }

    /// The strip on top of home: `ᛒ  Inbox 5   Backlog   Memory   History`, the
    /// one on screen bold and underlined, the Inbox count in the ask color.
    /// Under 100 columns the others are a letter. Also where each name is.
    fn strip_line(&self, width: u16, now: u64) -> (Line<'static>, Vec<(u16, u16, Section)>) {
        let t = self.settings.theme;
        // The rune as an accent chip, then the name where there's room (#286).
        let rune = match &t.rain {
            Some(colors) => rain::glitch(" ᛒ ", now, colors.glyphs),
            None => " ᛒ ".to_string(),
        };
        let mut spans = vec![rune.fg(t.bg).bg(t.accent).bold()];
        let narrow = width < 100;
        if !narrow {
            spans.push(" VALKYRIE".fg(t.accent).bold());
        }
        let here = self.section();
        let mut places = Vec::new();
        for section in Section::ALL {
            spans.push(if narrow { " " } else { "  " }.into());
            let from = Line::from(spans.clone()).width() as u16;
            let on = section == here;
            let name = match (narrow && !on, section) {
                (true, Section::Inbox) => "Inbox".to_string(),
                (true, s) => s.name()[..1].to_string(),
                (false, s) => s.name().to_string(),
            };
            // The one on screen is a chip on the selection color.
            let chip = |s: Span<'static>| if on { s.bg(t.selection) } else { s };
            spans.push(chip(" ".into()));
            spans.push(chip(if on {
                name.fg(t.fg).bold()
            } else {
                name.fg(t.dim())
            }));
            if section == Section::Inbox && self.need_you() > 0 {
                spans.push(chip(format!(" {}", self.need_you()).fg(t.needs).bold()));
            }
            spans.push(chip(" ".into()));
            let to = Line::from(spans.clone()).width() as u16;
            places.push((from, to, section));
        }
        // The section's tabs, the one open marked, and the keys between them.
        let open = self.memory_tab();
        let counted = |tab: memory::Tab, name: &str| match &self.memory {
            Some(page) => match page.count(tab, &self.proposals) {
                Some(n) => format!("{name} {n}"),
                None => name.to_string(),
            },
            None => name.to_string(),
        };
        let tabs: Vec<(String, bool)> = match here {
            Section::Backlog => backlog::Filter::ALL
                .iter()
                .map(|&f| {
                    let on = self.backlog.as_ref().is_some_and(|p| p.filter == f);
                    (f.label().to_string(), on)
                })
                .chain([(
                    match self.prs.as_ref().and_then(prs::Page::count) {
                        Some(n) => format!("Pull requests {n}"),
                        None => "Pull requests".into(),
                    },
                    self.prs.is_some(),
                )])
                .collect(),
            Section::Memory => MEMORY_TABS
                .iter()
                .map(|&tab| {
                    let name = match tab {
                        MemoryTab::Page(memory::Tab::Decisions) => {
                            counted(memory::Tab::Decisions, "Decisions")
                        }
                        MemoryTab::Page(memory::Tab::Skills) => {
                            counted(memory::Tab::Skills, "Skills")
                        }
                        MemoryTab::Page(memory::Tab::Notes) => counted(memory::Tab::Notes, "Notes"),
                        MemoryTab::Docs => "Docs".into(),
                        MemoryTab::Wiki => "Wiki".into(),
                    };
                    (name, tab == open)
                })
                .collect(),
            _ => Vec::new(),
        };
        if !tabs.is_empty() && !narrow {
            spans.push("    ".into());
            for (i, (name, on)) in tabs.iter().enumerate() {
                if i > 0 {
                    spans.push(" · ".fg(t.muted));
                }
                spans.push(if *on {
                    name.to_string().fg(t.fg).bold()
                } else {
                    name.to_string().fg(t.dim())
                });
            }
            spans.push("  [ ]".fg(t.accent).bold());
        }
        (Line::from(spans), places)
    }

    /// The Inbox frame's title: ` Inbox · 2 need you `, or all clear.
    fn inbox_title(&self) -> Line<'static> {
        let t = self.settings.theme;
        let need = self.need_you();
        Line::from(vec![
            " Inbox".fg(t.fg).bold(),
            " · ".fg(t.dim()),
            if need == 0 {
                "all clear ".fg(t.muted)
            } else {
                format!("{need} need{} you ", if need == 1 { "s" } else { "" }).fg(t.needs)
            },
        ])
    }

    /// The strip's right end, longest first: the host, how many are running and
    /// the time; then without the host, which can be long (a CI runner's is).
    fn strip_places(&self) -> [Vec<Span<'static>>; 2] {
        let t = self.settings.theme;
        let host = remote().map_or_else(valkyrie_proto::hostname, str::to_string);
        let running = self.sessions.iter().filter(|s| is_running(s)).count();
        let clock = jiff::ZonedDateTime::now().strftime("%H:%M").to_string();
        [
            vec![format!(" {host} · {running} running · {clock} ").fg(t.muted)],
            vec![format!(" {running} running · {clock} ").fg(t.muted)],
        ]
    }

    /// Whether home shows a session's detail as its own screen: narrow, opened
    /// with Enter, and a session still selected.
    fn narrow_detail(&self) -> bool {
        self.home_detail
            && self.home_cols.get() < 100
            && matches!(self.row(), Some(InboxRow::Queue(_) | InboxRow::Session(_)))
    }

    /// The row under the cursor.
    fn row(&self) -> Option<InboxRow> {
        self.inbox_rows().get(self.selected).copied()
    }

    /// What waits on a decision: proposals and ready items the supervisor offers
    /// to start. One Needs-you row, counting each.
    fn to_decide(&self) -> usize {
        self.proposals.len() + self.launches.len()
    }

    /// Backlog items agents filed that nobody has kept or dropped yet, oldest
    /// first. One an agent filed and took itself is its own work, under way.
    fn filed(&self) -> Vec<&Item> {
        let mut filed: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| i.status == valkyrie_proto::ItemStatus::Proposed && !i.agent_filed())
            .collect();
        filed.sort_by_key(|i| i.created);
        filed
    }

    /// The one count of what waits on you: every Needs-you row, a batched row
    /// counting its items. The header, the ⌂ and the status line all show it.
    fn need_you(&self) -> usize {
        self.queue.len()
            + self.to_decide()
            + self.filed().len()
            + usize::from(!self.disk.is_empty())
    }

    /// What a pack's avatar shows: formed while something needs you, gathering
    /// since the longest-running agent started, else latent.
    fn avatar_mood(&self) -> avatar::Mood {
        if self.need_you() > 0 {
            return avatar::Mood::Formed;
        }
        self.sessions
            .iter()
            .filter(|s| is_running(s))
            .map(|s| s.status.since_ms)
            .min()
            .map_or(avatar::Mood::Latent, |since_ms| avatar::Mood::Gathering {
                since_ms,
            })
    }

    /// The sessions the Inbox lists under Running, then Quiet: each one not
    /// already in Needs you, so every session is drawn once. Tab order within each.
    fn others(&self) -> Vec<&SessionInfo> {
        let (running, quiet): (Vec<&SessionInfo>, Vec<&SessionInfo>) = self
            .sessions
            .iter()
            .filter(|s| !self.queue.iter().any(|q| q.session == s.id))
            .partition(|s| is_running(s));
        running.into_iter().chain(quiet).collect()
    }

    /// The selection's row in the queue, if it is there.
    fn queue_index(&self) -> Option<usize> {
        match self.row()? {
            InboxRow::Queue(i) => Some(i),
            _ => None,
        }
    }

    /// Scrolls the shown proposal's text by `pages` (a page less a row, so the
    /// last row stays in sight).
    fn scroll_proposal(&self, pages: i32) {
        let page = i32::from(self.proposal_scroll.get().rows.saturating_sub(1).max(1));
        self.scroll_proposal_rows(pages * page);
    }

    fn scroll_proposal_rows(&self, rows: i32) {
        let mut at = self.proposal_scroll.get();
        at.top = (i32::from(at.top) + rows).clamp(0, i32::from(u16::MAX)) as u16;
        self.proposal_scroll.set(at);
    }

    /// The wheel over a proposal's text scrolls it, and a click on a link in it
    /// opens the link. Anything else is not for it.
    fn on_proposal_mouse(&mut self, m: Mouse) {
        let at = self.proposal_scroll.get();
        if !at.area.contains(ratatui::layout::Position::new(m.x, m.y)) {
            return;
        }
        match m.kind {
            MouseKind::WheelUp => self.scroll_proposal_rows(-WHEEL_ROWS),
            MouseKind::WheelDown => self.scroll_proposal_rows(WHEEL_ROWS),
            MouseKind::Press => {
                if let Some(url) = self.proposal_link_at(m.x, m.y) {
                    self.open_url(&url);
                }
            }
            _ => {}
        }
    }

    /// The link drawn at screen cell (`x`, `y`) of the proposal's text, if any.
    fn proposal_link_at(&self, x: u16, y: u16) -> Option<String> {
        let at = self.proposal_scroll.get();
        if !at.area.contains(ratatui::layout::Position::new(x, y)) {
            return None;
        }
        let row = usize::from(y - at.area.y + at.top);
        let col = x - at.area.x;
        self.proposal_links
            .borrow()
            .iter()
            .find(|l| l.row == row && (l.from..l.to).contains(&col))
            .and_then(|l| match &l.target {
                markdown::Target::Url(url) => Some(url.clone()),
                markdown::Target::Doc(_) => None,
            })
    }

    /// Opens `url` from the home screen, and says so.
    fn open_url(&mut self, url: &str) {
        let said = match links::open(url) {
            Ok(links::Opened::Browser) => format!("opened {url}"),
            Ok(links::Opened::Copied) => format!("copied {url} (no browser over SSH)"),
            Err(e) => format!("couldn't open {url}: {e}"),
        };
        self.say(said);
    }

    /// A proposal's text in `area`, scrolled as `proposal_scroll` says, with how
    /// much is left below.
    /// `head` (plain lines) comes first, then the body as markdown.
    fn draw_proposal_text(
        &self,
        frame: &mut ratatui::Frame,
        area: Rect,
        d: &Decision,
        head: Vec<Line<'static>>,
    ) {
        let t = self.settings.theme;
        let body = markdown::render(&d.body, area.width, t);
        let mut lines = head;
        if !body.lines.is_empty() {
            // A row between the head and the body.
            lines.push(Line::default());
        }
        // The head may wrap; the body comes back wrapped to fit, so its links'
        // rows only move down by the rows above it.
        let shift = Paragraph::new(lines.clone())
            .wrap(Wrap { trim: false })
            .line_count(area.width.max(1));
        *self.proposal_links.borrow_mut() = body
            .links
            .into_iter()
            .map(|l| markdown::Link {
                row: l.row + shift,
                ..l
            })
            .collect();
        lines.extend(body.lines);
        let mut at = self.proposal_scroll.get();
        // Another proposal, or a new version of this one, starts at its top.
        let whose = (fnv_path(&d.project), d.id, d.updated);
        if at.whose != Some(whose) {
            at = ProposalScroll {
                whose: Some(whose),
                ..Default::default()
            };
        }
        if area.width == 0 || area.height == 0 {
            // Nothing shows, so nothing is read.
            return;
        }
        let text = Paragraph::new(lines).wrap(Wrap { trim: false });
        let total = u16::try_from(text.line_count(area.width)).unwrap_or(u16::MAX);
        let last = total.saturating_sub(area.height);
        at.top = at.top.min(last);
        at.rows = area.height;
        at.area = area;
        self.proposal_scroll.set(at);
        frame.render_widget(text.scroll((at.top, 0)), area);
        if at.top < last && area.height > 0 {
            let more = format!(" ↓ {} more rows · PgDn / wheel ", last - at.top);
            let width = (more.chars().count() as u16).min(area.width);
            let row = Rect::new(
                area.x + area.width - width,
                area.y + area.height - 1,
                width,
                1,
            );
            frame.render_widget(
                Paragraph::new(more.fg(t.accent).bold()).style(style::Style::new().bg(t.panel)),
                row,
            );
        }
    }

    /// The proposal the cursor answers: the oldest, on the Decide row.
    fn proposal(&self) -> Option<&Decision> {
        match self.row()? {
            InboxRow::Decide => self.proposals.first(),
            _ => None,
        }
    }

    /// What the cursor is on, so it can follow that item when rows above it come or
    /// go (else `x` would kill the wrong session, `a` accept the wrong decision).
    fn anchor(&self) -> Anchor {
        match self.row() {
            Some(InboxRow::Queue(i)) => Anchor::Session(Some(self.queue[i].session)),
            Some(InboxRow::Session(id)) => Anchor::Session(Some(id)),
            Some(row) => Anchor::Record(row),
            None => Anchor::Session(None),
        }
    }

    fn restore(&mut self, anchor: Anchor) {
        if !self.cursor_moved {
            self.selected = self.resting_row();
            return;
        }
        let rows = self.inbox_rows();
        // A session can move between Needs you and the rows below it (it asks,
        // or it's seen): the cursor goes with it, wherever it is now.
        let found = match anchor {
            Anchor::Record(row) => rows.iter().position(|r| *r == row),
            Anchor::Session(id) => rows.iter().position(|r| match *r {
                InboxRow::Queue(i) => Some(self.queue[i].session) == id,
                InboxRow::Session(s) => Some(s) == id,
                _ => false,
            }),
        };
        if let Some(i) = found {
            self.selected = i;
        }
        self.clamp_selection();
    }

    /// Where the cursor rests until it's moved: the first row that waits on
    /// you, else the composer, so inbox zero is the composer (backlog #276).
    fn resting_row(&self) -> usize {
        self.inbox_rows().iter().position(waits).unwrap_or(0)
    }

    fn clamp_selection(&mut self) {
        self.selected = self.selected.min(self.rows().saturating_sub(1));
    }

    /// The session under the cursor, and the queue item if the cursor is in the queue.
    /// Neither on a proposal.
    fn selection(&self) -> (Option<SessionId>, Option<&QueueItem>) {
        match self.row() {
            Some(InboxRow::Queue(i)) => (Some(self.queue[i].session), Some(&self.queue[i])),
            Some(InboxRow::Session(id)) => (Some(id), None),
            _ => (None, None),
        }
    }

    /// Accepts or rejects the proposal under the cursor. The daemon pushes the
    /// shorter list, and the cursor lands on the next one.
    /// `a`/`d` on the selected row: accept or reject a proposal, or say an active
    /// decision flagged as maybe out of date still holds, or retire it.
    async fn review(&mut self, yes: bool) {
        if self.proposal().is_none() {
            return;
        }
        if let Some(said) = self.review_at(0, yes).await {
            self.status = said;
        }
    }

    /// Reviews proposal `i` (see `review`), and says how it went.
    async fn review_at(&mut self, i: usize, yes: bool) -> Option<String> {
        let d = self.proposals.get(i)?;
        let (project, id, title, seen) = (d.project.clone(), d.id, d.title.clone(), d.updated);
        // A scope proposed for an active decision (ADR-0014 §2): `a` takes it,
        // `d` leaves the decision where it was, everywhere.
        if let Some(scope) = scoping(d) {
            let scope = scope.to_vec();
            let (action, done) = match yes {
                true => (ReviewAction::Scope { scope }, "scoped"),
                false => (ReviewAction::DeclineScope, "kept everywhere:"),
            };
            let said = match self.client.review(project, id, action, Some(seen)).await {
                Ok(_) => {
                    // A scope answer isn't one `z` takes back.
                    self.answered(None);
                    format!("{done} #{id} {title}")
                }
                Err(e) => format!("review failed: {e:#}"),
            };
            return Some(said);
        }
        let flagged = d.status == valkyrie_proto::DecisionStatus::Active;
        let armed = self.retire_armed.take() == Some((project.clone(), id));
        if flagged && !yes && !armed {
            self.retire_armed = Some((project, id));
            return Some(format!("d again to retire #{id}; agents stop getting it"));
        }
        let (action, done, how) = match (flagged, yes) {
            (false, true) => (
                ReviewAction::Accept,
                "accepted",
                Some(memory::Reverse::Reopen(None)),
            ),
            (false, false) => (
                ReviewAction::Reject,
                "rejected",
                Some(memory::Reverse::Reopen(None)),
            ),
            // It holds as of now: there's nothing to take back.
            (true, true) => (ReviewAction::Confirm, "still holds:", None),
            (true, false) => (
                ReviewAction::Retire,
                "retired",
                Some(memory::Reverse::Reopen(None)),
            ),
        };
        let said = match self
            .client
            .review(project.clone(), id, action, Some(seen))
            .await
        {
            Ok(d) => {
                self.answered(how.map(|how| memory::Undo {
                    project,
                    id,
                    title: title.clone(),
                    seen: d.updated,
                    how,
                }));
                format!("{done} #{id} {title}")
            }
            Err(e) => format!("review failed: {e:#}"),
        };
        Some(said)
    }

    /// What the last answer would take back (`None`: it can't be). Memory
    /// keeps it for `z` when the answer was given there.
    fn answered(&mut self, undo: Option<memory::Undo>) {
        self.last_answer = Some(undo);
    }

    /// Keys over the decisions opened from a session: move, `a`/`d` as at home, and
    /// Esc back to the session. Keys after that go to the session.
    async fn review_keys(&mut self, mut keys: &[u8]) {
        let toggle = self.settings.keys.get(Action::Tabs).map(Key::bytes);
        while let Some(cursor) = self.reviewing
            && !keys.is_empty()
        {
            // As in tab mode, its key gives the keyboard back.
            if let Some(toggle) = &toggle
                && keys.starts_with(toggle)
            {
                keys = &keys[toggle.len()..];
                self.reviewing = None;
                continue;
            }
            let key = next_key(keys);
            let whole = key.len() == keys.len();
            keys = &keys[key.len()..];
            let last = self.proposals.len().saturating_sub(1);
            match key {
                b"k" | b"\x1b[A" | b"\x1bOA" => self.reviewing = Some(cursor.saturating_sub(1)),
                b"j" | b"\x1b[B" | b"\x1bOB" => self.reviewing = Some((cursor + 1).min(last)),
                b"\x1b[6~" | b" " => self.scroll_proposal(1),
                b"\x1b[5~" => self.scroll_proposal(-1),
                b"a" | b"d" => {
                    let said = self.review_at(cursor, key == b"a").await;
                    match (said, &mut self.view) {
                        (Some(said), Some(view)) => view.notice(said),
                        (Some(said), None) => self.status = said,
                        _ => {}
                    }
                }
                [DETACH_KEY] | b"q" => self.reviewing = None,
                b"\x1b" if whole => self.reviewing = None,
                _ => {}
            }
        }
        if !keys.is_empty() && self.view.is_some() {
            Box::pin(self.on_keys(keys.to_vec())).await;
        }
    }

    async fn on_home_key(&mut self, key: HomeKey) {
        let (selected, _) = self.selection();
        if matches!(
            key,
            HomeKey::Up | HomeKey::Down | HomeKey::NextWaiting | HomeKey::Enter
        ) {
            self.cursor_moved = true;
        }
        // Any other key keeps the session a first `x` asked about.
        if key != HomeKey::Kill {
            self.kill_armed = None;
        }
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
            HomeKey::Settings => self.settings_cursor = Some(0),
            HomeKey::Rename => {
                if let Some(id) = selected {
                    let name = self
                        .sessions
                        .iter()
                        .find(|s| s.id == id)
                        .map(|s| s.name.clone());
                    self.renaming = Some((id, edit::LineEdit::new(name.unwrap_or_default())));
                }
            }
            HomeKey::Quit => self.quit = true,
            HomeKey::Enter => {
                match self.row() {
                    Some(InboxRow::Compose) => self.compose_focus("").await,
                    // Narrow, Enter shows the session's detail first; Enter there
                    // attaches.
                    Some(InboxRow::Queue(_) | InboxRow::Session(_))
                        if self.home_cols.get() < 100 && !self.home_detail =>
                    {
                        self.home_detail = true;
                    }
                    _ if selected.is_some() => self.attach(selected.unwrap()).await,
                    // The proposals one at a time, a/d on each; the supervisor's
                    // page when only its offers wait.
                    Some(InboxRow::Decide) if !self.proposals.is_empty() => {
                        self.reviewing = Some(0)
                    }
                    Some(InboxRow::Decide) => self.open_supervisor().await,
                    Some(InboxRow::Disk) => self.free_disk(),
                    // Triage in the backlog, from the oldest.
                    Some(InboxRow::Filed) => {
                        if let Some(id) = self.filed().first().map(|i| i.id) {
                            self.open_backlog_at(id).await;
                        }
                    }
                    _ => {}
                }
            }
            HomeKey::Accept | HomeKey::Reject | HomeKey::Other if self.offered().is_some() => {
                self.take_step(key).await
            }
            HomeKey::Accept => self.review(true).await,
            HomeKey::Reject => self.review(false).await,
            HomeKey::Other => {}
            HomeKey::PageDown => self.scroll_proposal(1),
            HomeKey::PageUp => self.scroll_proposal(-1),
            HomeKey::Kill => {
                let Some(id) = selected else { return };
                if self.kill_armed != Some(id) {
                    self.kill_armed = Some(id);
                    return;
                }
                self.kill_armed = None;
                if let Err(e) = self.client.kill(id).await {
                    self.status = format!("close failed: {e:#}");
                }
                self.refresh().await;
            }
            HomeKey::NextWaiting => {
                // The next waiting row after the cursor, round to the first.
                let rows = self.inbox_rows();
                if rows.is_empty() {
                    return self.say("nothing waits on you".into());
                }
                let next = (self.selected + 1..rows.len())
                    .chain(0..=self.selected.min(rows.len().saturating_sub(1)))
                    .find(|&i| waits(&rows[i]));
                match next {
                    Some(i) => {
                        self.selected = i;
                        self.update_preview().await;
                    }
                    None => self.say("nothing waits on you".into()),
                }
            }
            HomeKey::Answer(n) => match self.selection() {
                (Some(id), Some(q)) if answerable(q) => {
                    let name = q.name.clone();
                    match self.client.input(id, vec![b'0' + n]) {
                        Ok(()) => self.status = format!("answered {name} with {n}"),
                        Err(e) => self.status = format!("couldn't answer: {e:#}"),
                    }
                }
                _ => self.say("1-9 answer a session that asks".into()),
            },
            HomeKey::Help => self.help = true,
            HomeKey::Back => self.home_detail = false,
            HomeKey::Compose => self.compose_focus("").await,
            HomeKey::ComposeItem => self.compose_focus("#").await,
            HomeKey::Direct(action) => self.on_direct(action).await,
            HomeKey::Keys => self.keys_page = Some(KeysPage::default()),
            HomeKey::AddDevice => self.add_device().await,
            HomeKey::Backlog => match self.toast_proposal() {
                Some(id) => self.open_backlog_at(id).await,
                None => self.open_backlog().await,
            },
            HomeKey::Toast => {
                if let Some(id) = self.toast_proposal() {
                    self.open_backlog_at(id).await;
                }
            }
            HomeKey::Digest => self.open_digest().await,
            // On the composer, `s` starts the top ready item (spec: n · s).
            HomeKey::Supervisor if self.row() == Some(InboxRow::Compose) => {
                self.start_ready().await
            }
            HomeKey::Supervisor => self.open_supervisor().await,
            // `h`, as it was: History, which now holds the past sessions too.
            HomeKey::Past => self.open_digest().await,
            HomeKey::Docs => self.open_docs().await,
            HomeKey::Wiki => self.open_wiki(selected).await,
            HomeKey::Adopt => self.adopt(selected).await,
        }
    }

    /// The selected queue row's flag and the step the supervisor offers on it
    /// (ADR-0019 §2).
    fn offered(&self) -> Option<(SessionId, u64, valkyrie_proto::Step)> {
        let q = self.queue.get(self.queue_index()?)?;
        let flag = q.flag.as_ref()?;
        Some((q.session, flag.seq, flag.step.clone()?))
    }

    /// `a` takes the step offered on the selected flagged session, `o` the other
    /// one, `d` leaves the flag for later.
    async fn take_step(&mut self, key: HomeKey) {
        use valkyrie_proto::StepAction;
        let Some((session, seq, step)) = self.offered() else {
            return;
        };
        let action = match key {
            HomeKey::Accept => step.action,
            HomeKey::Reject => StepAction::Leave,
            _ => match step.alt {
                Some(alt) => alt,
                None => return,
            },
        };
        self.status = match self.client.take_step(session, seq, action).await {
            Ok(said) => said,
            Err(e) => format!("{} failed: {e:#}", action.as_str()),
        };
        self.refresh().await;
    }

    /// Whether `A` can adopt the selected session into the backlog: a live claude
    /// or codex session not on an item already.
    fn adoptable(&self) -> Option<SessionId> {
        let id = self.selection().0?;
        let session = self.sessions.iter().find(|s| s.id == id)?;
        let agent = matches!(session.status.agent.as_str(), "claude" | "codex");
        let on_item = self.items.iter().any(|i| {
            i.boot == self.boot()
                && i.sessions.contains(&id)
                && matches!(
                    i.status,
                    valkyrie_proto::ItemStatus::Open | valkyrie_proto::ItemStatus::Proposed
                )
        });
        (session.exited.is_none() && agent && !on_item).then_some(id)
    }

    /// `A` at home: writes the selected agent session down as a backlog item, its
    /// card following that session (`valk todo adopt`).
    async fn adopt(&mut self, selected: Option<SessionId>) {
        if selected.is_none() {
            return;
        }
        let Some(id) = self.adoptable() else {
            self.status = "only a live claude or codex session not on an item yet".into();
            return;
        };
        self.status = match self
            .client
            .adopt_session(id, None, valkyrie_proto::ItemKind::Feature)
            .await
        {
            Ok(item) => format!("adopted as #{} {}", item.number, item.title),
            Err(e) => format!("adopt failed: {e:#}"),
        };
    }

    /// The daemon's boot, to tell this daemon's sessions from a restarted one's.
    fn boot(&self) -> u64 {
        self.daemon.as_ref().map_or(0, |d| d.boot)
    }

    /// The open page's rows: the items of the repository and kind it shows.
    fn backlog_rows(&self) -> Vec<(backlog::Lane, Item)> {
        let page = self.backlog.as_ref();
        let items: Vec<Item> = self
            .items
            .iter()
            .filter(|i| page.is_none_or(|p| p.holds(i)))
            .cloned()
            .collect();
        let mut rows = backlog::rows_of(&items, &self.items, &self.sessions, self.boot());
        if let Some(p) = page {
            rows.retain(|(lane, item)| p.filter.shows(*lane, item));
        }
        rows
    }

    async fn open_digest(&mut self) {
        // Just loaded: the next live reload is a full `PAGE_RELOAD` away.
        self.page_loaded = Instant::now();
        self.digest = Some(digest::Page::default());
        self.load_digest().await;
    }

    /// Asks the daemon for the digest the page's window and grouping want.
    async fn load_digest(&mut self) {
        self.fetch_digest(false).await;
    }

    /// The digest for the page's window: `live` is a reload that keeps the
    /// rows on screen until the answer comes (`reload_page`).
    async fn fetch_digest(&mut self, live: bool) {
        let Some(page) = &mut self.digest else {
            return;
        };
        if !live {
            page.loading();
        }
        let tz = jiff::tz::TimeZone::system();
        let since = page.win.since(now_ms(), &tz);
        // The sessions, what the supervisor did, and what ended (for `R`).
        let text = |e: anyhow::Error| format!("{e:#}");
        let digest = self
            .client
            .digest(since, valkyrie_proto::DigestGroup::Day)
            .await
            .map_err(text);
        let steps = self
            .client
            .supervisor_log()
            .await
            .map(|(log, _)| log)
            .map_err(text);
        let past = self.client.history().await.map_err(text);
        if let Some(page) = &mut self.digest {
            if live {
                page.refreshed(digest, steps, past, &tz);
            } else {
                page.loaded(digest, steps, past, &tz);
            }
        }
    }

    /// Keys while the away digest is open.
    async fn digest_keys(&mut self, bytes: &[u8]) {
        let Some(page) = &mut self.digest else {
            return;
        };
        for act in page.keys(bytes, &jiff::tz::TimeZone::system()) {
            match act {
                digest::Do::Close => self.digest = None,
                digest::Do::Load => self.load_digest().await,
                digest::Do::Open(id) => {
                    self.park();
                    self.attach(id).await;
                    self.unpark_unless_attached();
                    return;
                }
                digest::Do::Copy(text) => match mouse::copy_to_clipboard(&text) {
                    Ok(()) => self.say("copied the summary as markdown".into()),
                    Err(e) => self.say(format!("couldn't copy: {e}")),
                },
                digest::Do::Say(text) => self.say(text),
                digest::Do::Resume(key) => {
                    let size = self.session_size().unwrap_or(Size { cols: 80, rows: 24 });
                    match self.client.resume(&key, size).await {
                        Ok(info) => {
                            self.park();
                            self.refresh().await;
                            self.attach(info.id).await;
                            self.unpark_unless_attached();
                            return;
                        }
                        Err(e) => {
                            if let Some(page) = &mut self.digest {
                                page.resuming = false;
                            }
                            self.say(format!("resume failed: {e:#}"));
                        }
                    }
                }
            }
        }
    }

    /// What the supervisor waits on you for, from what the app holds.
    fn waiting(&self) -> Vec<supervisor::Wait> {
        supervisor::waiting(&self.queue, &self.launches, &self.proposals)
    }

    async fn open_supervisor(&mut self) {
        // Just loaded: the next live reload is a full `PAGE_RELOAD` away.
        self.page_loaded = Instant::now();
        self.supervisor = Some(supervisor::Page::default());
        self.refresh_numbers();
        self.load_supervisor().await;
    }

    async fn load_supervisor(&mut self) {
        let got = self
            .client
            .supervisor_log()
            .await
            .map_err(|e| e.to_string());
        let watch = self
            .client
            .supervisor_watch()
            .await
            .map_err(|e| e.to_string());
        if let Some(page) = &mut self.supervisor {
            page.loaded(got);
            page.watched(watch);
        }
    }

    /// Keys while the supervisor page is open.
    async fn supervisor_keys(&mut self, bytes: &[u8]) {
        let waits = self.waiting();
        let Some(page) = &mut self.supervisor else {
            return;
        };
        for act in page.keys(bytes, &waits) {
            match act {
                supervisor::Do::Close => self.supervisor = None,
                supervisor::Do::Say(text) => self.say(text),
                supervisor::Do::OpenSession => {
                    let size = self.session_size().unwrap_or(Size { cols: 80, rows: 24 });
                    // Another machine's daemon starts it in its own default.
                    let (cwd, env) = match remote() {
                        Some(_) => (std::path::PathBuf::new(), Vec::new()),
                        None => (
                            std::env::current_dir().unwrap_or_default(),
                            valkyrie_proto::login_env(),
                        ),
                    };
                    match self.client.supervisor_session(cwd, size, env).await {
                        Ok(info) => {
                            self.supervisor = None;
                            self.refresh().await;
                            self.attach(info.id).await;
                            return;
                        }
                        Err(e) => self.say(format!("supervisor session: {e:#}")),
                    }
                }
                supervisor::Do::TakeStep {
                    session,
                    seq,
                    action,
                } => {
                    let said = match self.client.take_step(session, seq, action).await {
                        Ok(said) => said,
                        Err(e) => format!("{} failed: {e:#}", action.as_str()),
                    };
                    self.say(said);
                    self.refresh().await;
                    self.load_supervisor().await;
                }
                supervisor::Do::AnswerLaunch {
                    project,
                    id,
                    launch,
                } => {
                    let said = match self.client.answer_launch(project, id, launch).await {
                        Ok(Some(s)) => format!("started session #{}", s.id),
                        Ok(None) => "left it: the next ready item is offered instead".into(),
                        Err(e) => format!("{e:#}"),
                    };
                    self.say(said);
                    self.refresh().await;
                    self.load_supervisor().await;
                }
                supervisor::Do::Review {
                    project,
                    id,
                    seen,
                    accept,
                } => {
                    let action = if accept {
                        ReviewAction::Accept
                    } else {
                        ReviewAction::Reject
                    };
                    let said = match self.client.review(project, id, action, Some(seen)).await {
                        Ok(d) => format!(
                            "{} policy #{id} {}",
                            if accept { "accepted" } else { "rejected" },
                            d.title
                        ),
                        Err(e) => format!("review failed: {e:#}"),
                    };
                    self.say(said);
                    self.refresh().await;
                }
            }
        }
    }

    /// Opens the wiki of the selected session's repository, else the one the TUI
    /// started in.
    /// The repository a wiki opens for: the selected session's, else the one
    /// the TUI runs in, if either is one.
    fn wiki_repo_for(&self, selected: Option<SessionId>) -> Option<PathBuf> {
        selected
            .and_then(|id| self.sessions.iter().find(|s| s.id == id))
            .map(|s| s.cwd.clone())
            .or_else(|| std::env::current_dir().ok())
            .filter(|d| remote().is_some() || in_repository(d))
    }

    /// The repository `]` would open Memory's Wiki tab for.
    fn wiki_repo(&self) -> Option<PathBuf> {
        self.wiki_repo_for(self.selection().0)
    }

    async fn open_wiki(&mut self, selected: Option<SessionId>) {
        // Just loaded: the next live reload is a full `PAGE_RELOAD` away.
        self.page_loaded = Instant::now();
        let Some(cwd) = self.wiki_repo_for(selected) else {
            self.say("the wiki is a repository's: select a session in one".into());
            return;
        };
        self.wiki = Some(wiki::Page::new(cwd));
        self.load_wiki().await;
    }

    async fn load_wiki(&mut self) {
        let Some(repo) = self.wiki.as_ref().map(|p| p.repo.clone()) else {
            return;
        };
        let got = self.client.wiki(repo).await.map_err(|e| format!("{e:#}"));
        if let Some(page) = &mut self.wiki {
            if let Ok(l) = &got {
                page.repo = l.root.clone();
            }
            page.loaded(got);
        }
    }

    /// Starts the agent that teaches Valkyrie (backlog #226), or opens the one
    /// already teaching.
    async fn start_tutor(&mut self) {
        let size = self.session_size().unwrap_or(Size { cols: 80, rows: 24 });
        match self.client.tutor("claude".into(), size, spawn_env()).await {
            Ok(info) => {
                self.refresh().await;
                self.attach(info.id).await;
            }
            Err(e) => self.say(format!("couldn't start the tutor: {e:#}")),
        }
    }

    /// Keys while the wiki is open: an exploration opens the session it started.
    async fn wiki_keys(&mut self, bytes: &[u8]) {
        let Some(page) = &mut self.wiki else {
            return;
        };
        for act in page.keys(bytes) {
            match act {
                wiki::Do::Close => self.wiki = None,
                wiki::Do::Read(slug) => {
                    let Some(repo) = self.wiki.as_ref().map(|p| p.repo.clone()) else {
                        return;
                    };
                    match self.client.wiki_page(repo, slug).await {
                        Ok(full) => {
                            if let Some(page) = &mut self.wiki {
                                page.read(full);
                            }
                        }
                        Err(e) => self.say(format!("couldn't read it: {e:#}")),
                    }
                }
                wiki::Do::Explore { stale } => {
                    let Some(repo) = self.wiki.as_ref().map(|p| p.repo.clone()) else {
                        return;
                    };
                    let size = self.session_size().unwrap_or(Size { cols: 80, rows: 24 });
                    match self
                        .client
                        .wiki_explore(repo, stale, "claude".into(), size, spawn_env())
                        .await
                    {
                        Ok(info) => {
                            self.park();
                            self.refresh().await;
                            self.attach(info.id).await;
                            self.unpark_unless_attached();
                            return;
                        }
                        Err(e) => {
                            if let Some(page) = &mut self.wiki {
                                page.starting = false;
                            }
                            self.say(format!("couldn't start the wiki agent: {e:#}"));
                        }
                    }
                }
                wiki::Do::Say(text) => self.say(text),
            }
        }
    }

    /// Whether the Docs page is on screen: it's open, and you're home, not in a
    /// session (your editor on a doc, say).
    fn docs_shown(&self) -> bool {
        self.docs.is_some() && self.view.is_none()
    }

    /// Whether the docs beside a session are on screen: open on the attached one.
    fn beside_shown(&self) -> bool {
        matches!((&self.beside, &self.view), (Some(b), Some(v)) if b.session == v.id)
    }

    /// The docs page a `docs::Do` answers (`docs_on`).
    fn docs_page(&mut self) -> Option<&mut docs::Page> {
        match self.docs_on {
            DocsOn::Home => self.docs.as_mut(),
            DocsOn::Beside => self.beside.as_mut().map(|b| &mut b.page),
        }
    }

    /// Opens the docs the attached session wrote beside it, reading `key` if
    /// given (a click on a mention of it). Open on that session already, it
    /// stays as it is, but for `key`.
    async fn open_beside(&mut self, key: Option<String>) {
        let Some(session) = self.view.as_ref().map(|v| v.id) else {
            return;
        };
        if !self.beside_shown() {
            let mut page = docs::Page::default();
            page.beside = true;
            page.numbers = self.items.iter().map(|i| (i.id, i.number)).collect();
            self.beside = Some(Beside { session, page });
            self.relayout();
            self.load_beside().await;
        }
        let act = key.and_then(|key| self.beside.as_mut()?.page.read(&key));
        if let Some(act) = act {
            self.docs_on = DocsOn::Beside;
            self.docs_do(act).await;
        }
    }

    /// The docs the session beside wrote, asked for again. Listing them marks
    /// nothing read: the Docs page does that.
    async fn load_beside(&mut self) {
        let Some(session) = self.beside.as_ref().map(|b| b.session) else {
            return;
        };
        let got = self.client.session_docs(session).await;
        // A live reload that failed keeps the list it had.
        if let Some(b) = &mut self.beside
            && (got.is_ok() || !b.page.listed())
        {
            b.page.loaded(got.map_err(|e| e.to_string()));
        }
    }

    fn close_beside(&mut self) {
        if self.beside.take().is_some() {
            self.relayout();
        }
    }

    /// Keys while the docs beside a session are open.
    async fn beside_keys(&mut self, bytes: &[u8]) {
        let Some(b) = &mut self.beside else { return };
        let acts = b.page.keys(bytes);
        for act in acts {
            self.docs_on = DocsOn::Beside;
            Box::pin(self.docs_do(act)).await;
        }
    }

    async fn open_docs(&mut self) {
        // Just loaded: the next live reload is a full `PAGE_RELOAD` away.
        self.page_loaded = Instant::now();
        self.docs = Some(docs::Page::default());
        self.refresh_numbers();
        self.load_docs().await;
    }

    /// Asks the daemon for the docs list. Opening it marks what agents wrote
    /// until now read, as the web app's list does; this list still shows them
    /// as new.
    async fn load_docs(&mut self) {
        self.fetch_docs(true).await;
    }

    /// The docs list; `seen` marks what agents wrote until now read. A live
    /// reload doesn't: you opened the list, not what arrived since.
    async fn fetch_docs(&mut self, seen: bool) {
        let got = self.client.docs_listing().await.map_err(|e| e.to_string());
        // A live reload that failed keeps the list it had.
        if !seen && got.is_err() {
            return;
        }
        let at = got.as_ref().ok().map(|l| l.at).filter(|_| seen);
        if let Some(page) = &mut self.docs {
            page.loaded(got);
        }
        if let Some(at) = at
            && let Err(e) = self.client.docs_seen(at).await
        {
            self.say(format!("couldn't mark docs read: {e}"));
        }
    }

    /// Keys while the Docs page is open.
    async fn docs_keys(&mut self, bytes: &[u8]) {
        let Some(page) = &mut self.docs else {
            return;
        };
        for act in page.keys(bytes) {
            self.docs_on = DocsOn::Home;
            Box::pin(self.docs_do(act)).await;
        }
    }

    /// Does what the Docs page asked.
    async fn docs_do(&mut self, act: docs::Do) {
        match act {
            docs::Do::Close => match self.docs_on {
                DocsOn::Home => self.docs = None,
                DocsOn::Beside => self.close_beside(),
            },
            docs::Do::Url(url) => self.open_url(&url),
            docs::Do::Open(key) => {
                let got = self
                    .client
                    .open_doc(key.clone())
                    .await
                    .map_err(|e| e.to_string());
                let next = self.docs_page().and_then(|page| page.opened(&key, got));
                if let Some(next) = next {
                    Box::pin(self.docs_do(next)).await;
                }
            }
            docs::Do::Comments(key) => {
                let got = self
                    .client
                    .doc_comments(key.clone())
                    .await
                    .map_err(|e| e.to_string());
                if let Some(page) = self.docs_page() {
                    page.comments(&key, got, false);
                }
            }
            docs::Do::Comment { key, quote, text } => {
                match self.client.comment_doc(key.clone(), quote, text).await {
                    Ok((list, to)) => {
                        if let Some(page) = self.docs_page() {
                            page.comments(&key, Ok(list), true);
                        }
                        // As the web app says it.
                        self.say(match to {
                                Some(to) => format!("{to} hears it at its next prompt."),
                                None => "The session that wrote it isn't running: the comment stays open here.".into(),
                            });
                    }
                    // The comment stays in its box, to send again.
                    Err(e) => {
                        if let Some(page) = self.docs_page() {
                            page.comments(&key, Err(e.to_string()), true);
                        }
                        self.say(format!("couldn't comment: {e}"));
                    }
                }
            }
            docs::Do::Resolve { key, id } => {
                match self.client.resolve_comment(key.clone(), id).await {
                    Ok(list) => {
                        if let Some(page) = self.docs_page() {
                            page.comments(&key, Ok(list), false);
                        }
                    }
                    Err(e) => self.say(format!("couldn't close it: {e}")),
                }
            }
            docs::Do::Edit {
                key,
                base,
                body,
                title,
            } => {
                self.start_edit(Edited::Doc { key, base }, body, title)
                    .await
            }
            docs::Do::Save { key, base, body } => self.save_doc(key, base, body, None).await,
            docs::Do::History(key) => {
                let got = self.client.doc_history(key.clone(), None).await;
                if let Some(page) = self.docs_page() {
                    page.history(&key, got.map(|(v, _)| v).map_err(|e| e.to_string()));
                }
            }
            docs::Do::Peek { key, version } => {
                match self
                    .client
                    .doc_history(key.clone(), Some(version.clone()))
                    .await
                {
                    Ok((_, Some(text))) => {
                        if let Some(page) = self.docs_page() {
                            page.peeked(&key, &version, text);
                        }
                    }
                    Ok((_, None)) => self.say("that version is gone".into()),
                    Err(e) => self.say(format!("couldn't open it: {e}")),
                }
            }
            docs::Do::Restore { key, base, version } => {
                match self.client.restore_doc(key.clone(), base, version).await {
                    Ok((doc, false)) => {
                        if let Some(page) = self.docs_page() {
                            page.changed(&key, doc);
                        }
                        // As the web app says it.
                        self.say("Restored. What it replaced is in its history.".into());
                    }
                    Ok((doc, true)) => {
                        if let Some(page) = self.docs_page() {
                            page.changed(&key, doc);
                        }
                        self.say("It changed since you opened it: this is the new version.".into());
                    }
                    Err(e) => self.say(format!("couldn't restore it: {e}")),
                }
            }
            docs::Do::Delete { key, base, title } => {
                match self.client.delete_doc(key.clone(), base).await {
                    Ok(Ok(token)) => {
                        if let Some(page) = self.docs_page() {
                            page.deleted(&key, token, title.clone());
                        }
                        self.say(format!("Deleted “{title}”: z puts it back."));
                        self.reload_docs_page().await;
                    }
                    Ok(Err(now)) => {
                        if let Some(page) = self.docs_page() {
                            page.changed(&key, now);
                        }
                        self.say("It changed since you opened it. This is the new version: d d to delete it.".into());
                    }
                    Err(e) => self.say(format!("couldn't delete it: {e}")),
                }
            }
            docs::Do::Undelete(token) => match self.client.undelete_doc(token.clone()).await {
                Ok(doc) => {
                    let next = self
                        .docs_page()
                        .and_then(|page| page.undeleted(&token, &doc));
                    self.say(format!("Put back as {}.", doc.path));
                    self.reload_docs_page().await;
                    if let Some(next) = next {
                        Box::pin(self.docs_do(next)).await;
                    }
                }
                Err(e) => {
                    if let Some(page) = self.docs_page() {
                        page.undo_failed(&token);
                    }
                    self.say(format!("couldn't put it back: {e}"));
                }
            },
            docs::Do::Deleted => {
                let got = self.client.deleted_docs().await.map_err(|e| e.to_string());
                if let Some(page) = self.docs_page() {
                    page.trash(got);
                }
            }
            docs::Do::Copy(text) => match mouse::copy_to_clipboard(&text) {
                Ok(()) => self.say("copied the doc as markdown".into()),
                Err(e) => self.say(format!("couldn't copy: {e}")),
            },
            docs::Do::Say(text) => self.say(text),
        }
    }

    /// The list `docs_on` names, loaded again after a delete or a put-back.
    async fn reload_docs_page(&mut self) {
        let on = self.docs_on;
        match on {
            DocsOn::Home => self.load_docs().await,
            DocsOn::Beside => self.load_beside().await,
        }
        self.docs_on = on;
    }

    /// Opens a doc's body (or a proposal's words) in your editor (`$VISUAL`,
    /// else `$EDITOR`, else `vi`), in a session of its own, and attaches to it.
    /// When it ends, what you wrote is saved, or accepted (`finish_edit`).
    async fn start_edit(&mut self, what: Edited, body: String, title: String) {
        if self.editing.is_some() {
            return self.say("something is already open in your editor".into());
        }
        // The file to edit would be on this machine, the editor on the other.
        if let Some(host) = remote() {
            return self.say(format!(
                "editing needs {host}'s own valk: `ssh -t {host} valk`"
            ));
        }
        let path = match edit_file(&body) {
            Ok(path) => path,
            Err(e) => return self.say(format!("couldn't make a file to edit: {e}")),
        };
        let mut command = editor_command();
        command.push(path.display().to_string());
        let size = self.session_size().unwrap_or(Size { cols: 80, rows: 24 });
        let spec = SpawnSpec {
            command,
            cwd: None,
            name: Some(format!("edit {}", fit(&title, 30))),
            size,
            env: spawn_env(),
            worktree: None,
            hidden: false,
        };
        match self.client.spawn(spec).await {
            Ok(info) => {
                self.editing = Some(Editing {
                    session: info.id,
                    what,
                    path,
                    body,
                    seen: false,
                });
                self.refresh().await;
                let listed = self.sessions.iter().any(|s| s.id == info.id);
                if let Some(e) = &mut self.editing {
                    e.seen |= listed;
                }
                self.attach(info.id).await;
            }
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                self.say(format!("couldn't start your editor: {e:#}"));
            }
        }
    }

    /// Once the editor's session has ended: saves what you wrote, if anything.
    /// It has to have been seen running first, so a list that missed it (a failed
    /// refresh) doesn't end it while you're still typing.
    async fn finish_edit(&mut self) {
        let sessions = &self.sessions;
        let Some(e) = &mut self.editing else { return };
        let running = sessions
            .iter()
            .any(|s| s.id == e.session && s.exited.is_none());
        if running {
            e.seen = true;
            return;
        }
        if !e.seen && sessions.iter().all(|s| s.id != e.session) {
            return;
        }
        let Some(e) = self.editing.take() else { return };
        match std::fs::read_to_string(&e.path) {
            Ok(text) if text == e.body => {
                let _ = std::fs::remove_file(&e.path);
                let still = match e.what {
                    Edited::Doc { .. } => "",
                    Edited::Proposal { .. } => ", so it's still up for review",
                };
                // An editor that forks into the background ends at once, unchanged.
                self.say(format!("no changes{still} (if your editor runs in the background, make it wait, as `code -w` does)"));
            }
            Ok(text) => match e.what {
                Edited::Doc { key, base } => self.save_doc(key, base, text, Some(e.path)).await,
                what @ Edited::Proposal { .. } => self.accept_edited(what, text, e.path).await,
            },
            Err(err) => self.say(format!(
                "couldn't read what you wrote in {}: {err}",
                e.path.display()
            )),
        }
    }

    /// Saves `body` over the version `base` names. `file` holds it too (your
    /// editor's), and goes only once it's saved: on a conflict or an error the
    /// message says where yours is, so it's never only on screen.
    async fn save_doc(&mut self, key: String, base: String, body: String, file: Option<PathBuf>) {
        let saved = self.client.save_doc(key.clone(), base, body.clone()).await;
        if let (Ok((_, false)), Some(f)) = (&saved, &file) {
            let _ = std::fs::remove_file(f);
        }
        let kept = || match &file {
            Some(f) => Ok(f.clone()),
            None => edit_file(&body),
        };
        match saved {
            Ok((doc, false)) => {
                // Either page may have it open: each takes only its own doc.
                if let Some(b) = &mut self.beside {
                    b.page.saved(&key, doc.clone());
                }
                if let Some(page) = &mut self.docs {
                    page.saved(&key, doc);
                }
                self.say("Saved.".into());
            }
            // The path first: a narrow bar cuts the end off.
            Ok((theirs, true)) => {
                let kept = kept();
                let beside = self
                    .beside
                    .as_mut()
                    .is_some_and(|b| b.page.conflict(&key, body.clone(), theirs.clone()));
                let home = self
                    .docs
                    .as_mut()
                    .is_some_and(|page| page.conflict(&key, body, theirs));
                let shown = beside || home;
                let why = if shown {
                    "it changed since you opened it: t, e or s"
                } else {
                    "it changed since you opened it"
                };
                self.say(match kept {
                    Ok(f) => format!("not saved, yours is in {}: {why}", f.display()),
                    Err(_) => format!("not saved: {why}"),
                });
            }
            Err(err) => match kept() {
                Ok(f) => self.say(format!("not saved, yours is in {}: {err}", f.display())),
                Err(_) => self.say(format!("couldn't save: {err}")),
            },
        }
    }

    /// The wheel scrolls the doc being read; a click on a link in it opens it,
    /// and on a wikilink reads that doc.
    async fn on_docs_mouse(&mut self, m: Mouse) {
        self.docs_on = DocsOn::Home;
        self.docs_mouse(m).await;
    }

    /// The mouse over the docs `docs_on` names.
    async fn docs_mouse(&mut self, m: Mouse) {
        let Some(page) = self.docs_page() else { return };
        match m.kind {
            MouseKind::WheelUp | MouseKind::WheelDown => {
                page.wheel(m.kind == MouseKind::WheelDown, WHEEL_ROWS)
            }
            MouseKind::Press => {
                page.pressed();
                match page.link_at(m.x, m.y) {
                    Some(markdown::Target::Url(url)) => self.open_url(&url),
                    Some(markdown::Target::Doc(key)) => {
                        if let Some(act) = page.follow(&key) {
                            Box::pin(self.docs_do(act)).await;
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    async fn open_backlog(&mut self) {
        let mut page = backlog::Page::default();
        page.launches = self.launches.clone();
        self.backlog = Some(page);
        self.backlog_facts().await;
    }

    /// The daemon's list of repositories, for an edit to move an item to: asked
    /// for as an edit opens, not to hold up the page. A daemon slow to answer
    /// leaves the list as it was, the items' own repositories at least.
    async fn load_repos(&mut self) {
        let asked = self.client.repos();
        if let Ok(Ok(list)) = tokio::time::timeout(Duration::from_millis(500), asked).await {
            self.repo_list = list;
        }
    }

    /// The repositories an edit can move an item to, by name.
    fn edit_roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self
            .items
            .iter()
            .filter_map(|i| i.project.clone())
            .chain(self.repo_list.repos.iter().map(|r| r.path.clone()))
            .collect();
        roots.sort_by(|a, b| a.file_name().cmp(&b.file_name()).then(a.cmp(b)));
        roots.dedup();
        roots
    }

    /// The backlog, its cursor on item `id` (a proposal's toast).
    async fn open_backlog_at(&mut self, id: u32) {
        self.toast = None;
        let mut page = backlog::Page::default();
        page.launches = self.launches.clone();
        self.backlog = Some(page);
        let at = self.backlog_rows().iter().position(|(_, i)| i.id == id);
        match (&mut self.backlog, at) {
            (Some(page), Some(at)) => page.cursor = at,
            _ => self.say(format!("#{} isn't on the board", self.item_no(id))),
        }
        self.backlog_facts().await;
    }

    /// The backlog proposal the toast up now is about.
    fn toast_proposal(&self) -> Option<u32> {
        self.toast().and_then(|t| t.proposal)
    }

    /// Accepts (open) or declines (drops) the proposal on the toast.
    async fn answer_proposal(&mut self, accept: bool) {
        let Some(id) = self.toast_proposal() else {
            return self.say("no backlog proposal to answer".into());
        };
        self.toast = None;
        // Answered elsewhere meanwhile (the web, the backlog page): leave it be.
        let proposed = self
            .items
            .iter()
            .any(|i| i.id == id && i.status == valkyrie_proto::ItemStatus::Proposed);
        if !proposed {
            return self.say(format!("#{} was already answered", self.item_no(id)));
        }
        let status = if accept {
            valkyrie_proto::ItemStatus::Open
        } else {
            valkyrie_proto::ItemStatus::Dropped
        };
        let said = match self
            .client
            .change_item(id, ItemChange::Status { status })
            .await
        {
            Ok((item, _)) if accept => format!("accepted #{} {}", item.number, item.title),
            Ok((item, _)) => format!("declined #{} {}", item.number, item.title),
            Err(e) => format!("can't answer #{}: {e:#}", self.item_no(id)),
        };
        self.say(said);
    }

    /// Reads the selected item's facts from git, unless they're for it already.
    async fn backlog_facts(&mut self) {
        let rows = self.backlog_rows();
        let Some(page) = &mut self.backlog else {
            return;
        };
        page.cursor = page.cursor.min(rows.len().saturating_sub(1));
        let Some((_, item)) = rows.get(page.cursor) else {
            return;
        };
        let fresh = page
            .facts
            .as_ref()
            .is_some_and(|(id, updated, _)| *id == item.id && *updated == item.updated);
        if fresh {
            return;
        }
        let (id, updated) = (item.id, item.updated);
        let facts = match item.checkout {
            Some(_) => self.client.item_facts(id).await.ok().flatten(),
            None => None,
        };
        if let Some(page) = &mut self.backlog {
            page.facts = Some((id, updated, facts));
        }
    }

    async fn backlog_keys(&mut self, bytes: &[u8]) {
        let rows = self.backlog_rows();
        let boot = self.boot();
        let roots = self.edit_roots();
        let Some(page) = &mut self.backlog else {
            return;
        };
        let mut bytes = bytes;
        // The capture key on the open page starts a new item, as `n` does.
        if page.typing.is_none()
            && let Some((Action::Capture, len)) = self.settings.keys.at(bytes)
        {
            page.typing = Some(backlog::Typing::add());
            bytes = &bytes[len..];
        }
        let editing = matches!(page.typing, Some(backlog::Typing::Edit { .. }));
        page.roots = roots;
        let todo = page.keys(bytes, &rows, &self.sessions, boot);
        for act in todo {
            // An item that couldn't be added goes back in the prompt, not lost.
            let retry = match &act {
                backlog::Do::Add { kind, text } => Some(backlog::Typing::Add {
                    kind: *kind,
                    text: edit::LineEdit::new(text.clone()),
                }),
                _ => None,
            };
            if let Err(e) = self.backlog_do(act).await {
                self.say(format!("{e:#}"));
                if let (Some(page), Some(retry)) = (&mut self.backlog, retry)
                    && page.typing.is_none()
                {
                    page.typing = Some(retry);
                }
            }
        }
        // A capture is done once its item is added or cancelled: back where it began.
        if self
            .backlog
            .as_ref()
            .is_some_and(|p| p.capture && p.typing.is_none())
        {
            self.backlog = None;
        }
        // An edit just opened: the daemon's list, for Shift-Tab to step through.
        if !editing
            && self
                .backlog
                .as_ref()
                .is_some_and(|p| matches!(p.typing, Some(backlog::Typing::Edit { .. })))
        {
            self.load_repos().await;
            let roots = self.edit_roots();
            if let Some(page) = &mut self.backlog {
                page.roots = roots;
            }
        }
        self.backlog_facts().await;
    }

    async fn backlog_do(&mut self, act: backlog::Do) -> Result<()> {
        use backlog::Do;
        match act {
            Do::Close => self.backlog = None,
            Do::Say(text) => self.say(text),
            Do::AddRepo { path, back } => {
                let mut back = *back;
                let edit = valkyrie_proto::RepoEdit::Add {
                    path: PathBuf::from(&path),
                };
                match self.client.repo_edit(edit).await {
                    Ok(list) => {
                        let root = added_repo(&self.repo_list, &list, &path);
                        if let (Some(root), backlog::Typing::Edit { project, .. }) =
                            (root, &mut back)
                        {
                            *project = Some(root);
                        }
                        self.repo_list = list;
                    }
                    Err(e) => self.say(format!("{e:#}")),
                }
                if let Some(page) = &mut self.backlog {
                    page.typing = Some(back);
                }
            }
            Do::Attach(id) => {
                self.park();
                self.switch_to(id).await;
                self.unpark_unless_attached();
            }
            Do::Launch {
                id,
                agent,
                model,
                worktree,
            } => {
                let size = self.session_size()?;
                let info = self
                    .client
                    .launch_item(id, agent, worktree, size, model)
                    .await?;
                self.park();
                self.refresh().await;
                self.switch_to(info.id).await;
                self.unpark_unless_attached();
            }
            Do::Status(id, status) => {
                let (item, note) = self
                    .client
                    .change_item(id, ItemChange::Status { status })
                    .await?;
                let said = format!("#{} {}", item.number, item.status.as_str());
                self.say(note.map_or(said.clone(), |n| format!("{said}: {n}")));
            }
            Do::Return(id) => {
                let (item, _) = self.client.change_item(id, ItemChange::Return).await?;
                self.say(format!("#{} back in the backlog", item.number));
            }
            Do::Delete(id) => {
                let (item, _) = self.client.delete_item(id).await?;
                self.say(format!("deleted #{} {}", item.number, item.title));
            }
            Do::Note { id, text } => {
                self.client.note_item(id, text).await?;
                self.say(format!("noted on #{}", self.item_no(id)));
            }
            Do::Edit {
                id,
                kind,
                title,
                priority,
                cwd,
            } => {
                let (item, _) = self
                    .client
                    .change_item(
                        id,
                        ItemChange::Edit {
                            title,
                            body: None,
                            kind,
                            cwd,
                            priority,
                            blocked_by: None,
                            parent: None,
                        },
                    )
                    .await?;
                self.say(format!(
                    "saved #{} {} ({})",
                    item.number,
                    item.title,
                    item.priority.as_str()
                ));
            }
            Do::CheckMerged | Do::ShipMerged(_) if self.ship_busy => {
                self.say("S is already under way".into());
            }
            Do::CheckMerged => {
                // The daemon judges against `origin` as last fetched and can't fetch
                // with the user's credentials, so fetch here first. Off the event
                // loop: a fetch can take a while, and the answer comes back on
                // `shipping`.
                // What the page's search shows.
                let page = self.backlog.as_ref();
                let mut projects: Vec<PathBuf> = self
                    .items
                    .iter()
                    .filter(|i| page.is_none_or(|p| p.holds(i)))
                    .filter(|i| i.status == valkyrie_proto::ItemStatus::Open && i.branch.is_some())
                    .filter_map(|i| i.project.clone())
                    .collect();
                projects.sort();
                projects.dedup();
                let (client, tx) = (self.client.clone(), self.shipping_tx()?);
                self.ship_busy = true;
                self.say("fetching origin…".into());
                tokio::spawn(async move {
                    let unfetched = tokio::task::spawn_blocking(move || {
                        projects
                            .iter()
                            .filter(|p| p.is_dir())
                            .filter(|p| valkyrie_proto::fetch_origin(p, true).is_err())
                            .count()
                    })
                    .await
                    .unwrap_or_default();
                    let result = client.ship_merged(true, None).await;
                    let _ = tx.send(Shipping {
                        asking: true,
                        unfetched,
                        result: result.map_err(|e| format!("{e:#}")),
                    });
                });
            }
            Do::ShipMerged(ids) => {
                let (client, tx) = (self.client.clone(), self.shipping_tx()?);
                self.ship_busy = true;
                self.say("shipping…".into());
                tokio::spawn(async move {
                    let result = client.ship_merged(false, Some(ids)).await;
                    let _ = tx.send(Shipping {
                        asking: false,
                        unfetched: 0,
                        result: result.map_err(|e| format!("{e:#}")),
                    });
                });
            }
            Do::AnswerLaunch {
                project,
                id,
                item,
                launch,
            } => match self.client.answer_launch(project, id, launch).await? {
                Some(info) => {
                    self.park();
                    self.refresh().await;
                    self.switch_to(info.id).await;
                    self.unpark_unless_attached();
                }
                None => self.say(format!(
                    "left #{}: it's offered again once it changes",
                    self.item_no(item)
                )),
            },
            Do::Add { kind, text } => {
                // For the attached tab's repository, else the one the TUI is in.
                // Another machine's paths can't be looked at here: the daemon
                // finds the attached tab's repository itself.
                let cwd = if remote().is_some() {
                    self.attached_cwd()
                } else {
                    self.attached_cwd()
                        .or_else(|| std::env::current_dir().ok())
                        .filter(|d| in_repository(d))
                };
                let item = self
                    .client
                    .add_item(NewItem {
                        title: String::new(),
                        body: text,
                        kind,
                        cwd,
                        ..NewItem::default()
                    })
                    .await?;
                self.say(format!("added #{} {}", item.number, item.title));
            }
        }
        Ok(())
    }

    async fn add_device(&mut self) {
        let Some(&make) = ADD_DEVICE.get() else {
            return;
        };
        // The code would pair a device with this machine's web app.
        if let Some(host) = remote() {
            self.status = format!("add a device on {host}: `ssh -t {host} valk web pair`");
            return;
        }
        // A paired device can do whatever the user can; an agent mustn't add one.
        if let Err(e) = self.client.vouch("adding a device").await {
            self.status = format!("can't add a device: {e:#}");
            return;
        }
        self.status = match make() {
            Ok(code) => format!(
                "add a device: type {code} into its Valkyrie pairing screen (works once, for 5 minutes)"
            ),
            Err(e) => format!("can't add a device: {e}"),
        };
    }

    /// Returns whether the queue or the proposals changed.
    fn on_push(&mut self, msg: ServerMsg) -> Result<bool> {
        if let ServerMsg::Proposals { items } = msg {
            let anchor = self.anchor();
            self.proposals = items;
            self.restore(anchor);
            // The reviewed one leaves, so the cursor lands on the next; the last
            // one closes the list.
            let last = self.proposals.len().checked_sub(1);
            self.reviewing = self.reviewing.zip(last).map(|(i, last)| i.min(last));
            return Ok(true);
        }
        if let ServerMsg::Items { items, launches } = msg {
            let new = self.new_proposals.on_items(&items);
            if self.settings.proposals
                && let Some(ping) = Ping::proposals(&new)
            {
                self.show_ping(ping);
            }
            self.items = items;
            self.refresh_numbers();
            if let Some(page) = &mut self.backlog {
                page.launches = launches.clone();
            }
            self.launches = launches;
            return Ok(true);
        }
        if let ServerMsg::Quota { quota } = msg {
            self.quota = quota;
            return Ok(true);
        }
        if let ServerMsg::Disk { low } = msg {
            let anchor = self.anchor();
            self.disk = low;
            self.restore(anchor);
            return Ok(true);
        }
        if let ServerMsg::Notice { text, sound } = msg {
            self.show_ping(Ping::notice(&text, sound));
            return Ok(true);
        }
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
        // A pane hidden behind a zoom has nowhere on screen to put an image.
        let hidden = self.is_zoomed() && self.others.iter().any(|p| p.id == session);
        let Some(msg) = self.mini_push(msg)? else {
            return Ok(false);
        };
        let focused = self.view.as_ref().is_some_and(|v| v.id == session);
        let forced = self.paste_forced;
        let Some(view) = self.pane_mut(session) else {
            return Ok(false);
        };
        let mut mouse_changed = false;
        match msg {
            ServerMsg::Screen { update, .. } => {
                let before = (view.modes, view.shape);
                view.apply(update);
                if focused && before != (view.modes, view.shape) {
                    write_modes(forced_modes(view.modes, forced), Some(view.shape))?;
                }
                mouse_changed = mouse::modes(before.0) != mouse::modes(view.modes);
            }
            ServerMsg::Graphics { .. } if hidden => {}
            ServerMsg::Graphics { x, y, data, .. } => {
                // At the cell the program's cursor was on, then the cursor back where
                // ratatui left it. Unicode placeholders (yazi, `kitten icat`) are
                // ordinary cells on the screen; this only delivers the image data.
                let mut out = term::outer();
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
        if self.view.is_none() {
            self.capture_home();
        }
        terminal.draw(|frame| {
            let [body, bar] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(frame.area());
            match &self.view {
                Some(view) => {
                    let (whole, tabs) = self.split_tabs(body);
                    let (main, docs_area) = self.beside_split(whole);
                    let cursor = !self.tab_mode
                        && self.renaming.is_none()
                        && self.menu.is_none()
                        && self.reviewing.is_none()
                        && self.settings_cursor.is_none()
                        && self.keys_page.is_none()
                        && self.digest.is_none()
                        && self.supervisor.is_none()
                        && self.wiki.is_none()
                        && !self.docs_shown()
                        && self.backlog.is_none()
                        && !self.mini_open()
                        && !self.rail_open();
                    // The docs beside have the keyboard: no cursor in a pane.
                    let cursor = cursor && docs_area.is_none();
                    // Covered whole on a narrow screen, the panes aren't drawn.
                    if docs_area != Some(main) {
                        self.draw_panes(frame, main, cursor);
                    }
                    if let (Some(area), Some(b)) = (docs_area, &self.beside) {
                        self.draw_beside(frame, area, area != main, &b.page);
                    }
                    if let Some(tabs) = tabs {
                        self.draw_tabs(frame, tabs);
                    }
                    if let Some(session) = self.peeked() {
                        self.draw_peek(frame, main, tabs, session);
                    }
                    if self.tab_mode {
                        dim(frame, main, self.settings.theme);
                        self.draw_which_key(frame, main);
                    }
                    if let Some(menu) = &self.menu {
                        dim(frame, main, self.settings.theme);
                        self.draw_menu(frame, menu);
                    }
                    if let Some(cursor) = self.reviewing {
                        dim(frame, main, self.settings.theme);
                        self.draw_reviews(frame, main, cursor);
                    }
                    self.draw_session_bar(frame, view, body, bar);
                }
                None => {
                    self.draw_home(frame, body, bar);
                    if let Some(cursor) = self.reviewing {
                        dim(frame, body, self.settings.theme);
                        self.draw_reviews(frame, body, cursor);
                    }
                }
            }
            self.draw_mini(frame);
            if let Some(page) = &self.digest {
                page.draw(frame, self.settings.theme);
            }
            if let Some(page) = &self.supervisor {
                page.draw(frame, &self.waiting(), now_ms(), self.settings.theme);
            }
            if let Some(page) = &self.wiki {
                page.draw(frame, self.settings.theme);
            }
            if let Some(page) = self.memory.as_ref().filter(|_| self.view.is_none()) {
                page.draw(frame, &self.proposals, self.settings.theme);
            }
            if let Some(page) = self.prs.as_ref().filter(|_| self.view.is_none()) {
                page.draw(frame, &self.item_numbers(), now_ms(), self.settings.theme);
            }
            if let Some(page) = self.docs.as_ref().filter(|_| self.view.is_none()) {
                page.draw(frame, self.settings.theme);
            }
            if let Some(page) = &self.backlog {
                let rows = self.backlog_rows();
                let t = self.settings.theme;
                page.draw(
                    frame,
                    &rows,
                    &self.items,
                    &self.sessions,
                    self.boot(),
                    now_ms(),
                    t,
                );
            }
            // Over a section's page, a toast or what a key said.
            self.draw_said(frame);
            let t = self.settings.theme;
            let area = frame.area();
            // Dialogs last, over a dimmed screen (rule 8: a box is a dialog).
            if self.help {
                self.draw_help(frame);
            }
            if let Some(cursor) = self.settings_cursor {
                dim(frame, area, t);
                self.draw_settings(frame, cursor);
            }
            if let Some(page) = &self.keys_page {
                dim(frame, area, t);
                self.draw_keys_page(frame, page);
            }
        })?;
        Ok(())
    }
}

/// How long a preview waits for the daemon's copy of a screen.
const PREVIEW_WAIT: Duration = Duration::from_millis(300);
/// How long the mouse rests on a tab before it peeks.
const HOVER_DELAY: Duration = Duration::from_secs(1);
/// How soon a second press on a tab makes a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// How often an open section page loads again on its own.
const PAGE_RELOAD: Duration = Duration::from_secs(30);

/// How long a ping's toast stays in the bar.
const TOAST_FOR: Duration = Duration::from_secs(6);
/// A backlog proposal's toast stays longer: long enough to answer it.
const PROPOSAL_TOAST_FOR: Duration = Duration::from_secs(15);

impl App {
    /// Tells the daemon this terminal's cell size in pixels, for image programs (as
    /// the default for new sessions, and for `session`). Terminals that do not report
    /// pixels (some over SSH) leave the daemon's guess.
    fn report_cell(&self, session: Option<SessionId>) {
        if let Some((width, height)) = cell_pixels() {
            let _ = self.client.cell_pixels(session, width, height);
        }
        self.report_colors(session);
    }

    /// Tells the daemon what colors sessions are drawn with, so a program that asks
    /// for the background (OSC 11) picks a matching light or dark palette. With the
    /// terminal's own colors it can't know them, so the daemon's dark guess stands.
    fn report_colors(&self, session: Option<SessionId>) {
        let colors = self
            .settings
            .themed_sessions
            .then(|| self.settings.theme.term_colors());
        let _ = self.client.term_colors(session, colors);
    }

    fn fire_pings(&mut self) {
        let viewing = self.pane_ids();
        let Some(ping) = self.pinger.due(&viewing, Instant::now()) else {
            return;
        };
        self.show_ping(ping);
    }

    /// A ping's sound, desktop notification and toast, as the settings allow.
    fn show_ping(&mut self, ping: Ping) {
        if ping.sound && self.settings.sound {
            ping::play(self.settings.noise, ping.kind);
        }
        if self.settings.desktop {
            ping::desktop("Valkyrie", &ping.text);
        }
        // A fresh request, or a proposal waiting on an answer, stays up over a
        // later finish; a proposal takes the bar from a request, as it pops once.
        let rank = |p: &Ping| u8::from(p.kind == Kind::Request || p.proposal.is_some());
        if self.toast().is_some_and(|t| rank(t) > rank(&ping)) {
            return;
        }
        self.toast = Some((ping, Instant::now()));
    }

    fn toast(&self) -> Option<&Ping> {
        self.toast
            .as_ref()
            .filter(|(ping, at)| {
                let lasts = match ping.proposal {
                    Some(_) => PROPOSAL_TOAST_FOR,
                    None => TOAST_FOR,
                };
                at.elapsed() < lasts
            })
            .map(|(ping, _)| ping)
    }

    /// Gives the docs and supervisor pages the number each item id goes by, for
    /// the lines that carry an id: a doc's item, a watched PR's.
    fn refresh_numbers(&mut self) {
        let numbers: std::collections::HashMap<u32, u32> =
            self.items.iter().map(|i| (i.id, i.number)).collect();
        if let Some(page) = &mut self.docs {
            page.numbers = numbers.clone();
        }
        if let Some(page) = &mut self.supervisor {
            page.numbers = numbers;
        }
    }

    /// What people call item `id`: its number in its repository.
    fn item_no(&self, id: u32) -> u32 {
        self.items
            .iter()
            .find(|i| i.id == id)
            .map_or(id, |i| i.number)
    }

    /// The keys that answer a proposal's toast: tab mode's own, once you're in it.
    /// The home screen's `a` is the decisions', so there `b` opens it to answer.
    fn toast_hint(&self, ping: &Ping) -> Option<String> {
        let id = self.item_no(ping.proposal?);
        if self.view.is_none() {
            return Some("b open".into());
        }
        if self.tab_mode {
            return Some(format!("a accept #{id} · x decline · b open"));
        }
        let tabs = self.settings.keys.get(Action::Tabs)?.label();
        Some(format!("{tabs} a accept #{id} · x decline"))
    }

    /// The toast's spans, as the bar shows them.
    fn toast_line(&self, ping: &Ping) -> Vec<Span<'static>> {
        let t = self.settings.theme;
        let mut spans = toast_spans(ping, self.toast_summary(ping), t);
        if let Some(hint) = self.toast_hint(ping) {
            spans.insert(spans.len() - 1, format!(" {hint} ").fg(t.muted));
        }
        spans
    }

    /// What the pinged session needs or finished, as it is now: a summary can
    /// arrive after the ping.
    fn toast_summary(&self, ping: &Ping) -> Option<&str> {
        self.sessions
            .iter()
            .find(|s| Some(s.id) == ping.session)
            .and_then(|s| s.status.summary.as_deref())
    }
}

async fn sleep_until(at: Option<Instant>) {
    if let Some(at) = at {
        tokio::time::sleep_until(at.into()).await;
    }
}

/// The longest summary a toast carries.
const TOAST_SUMMARY_MAX: usize = 60;

/// ` ● api needs input ` in the ping's color, then what it asks or finished.
fn toast_spans(ping: &Ping, summary: Option<&str>, t: &Theme) -> Vec<ratatui::text::Span<'static>> {
    let (icon, color) = match (ping.proposal, ping.kind) {
        // A filed item, as its Inbox row marks it.
        (Some(_), _) => ("■", t.accent_dim()),
        (None, Kind::Request) => ("●", t.needs),
        (None, Kind::Done) => ("●", t.done),
    };
    let mut spans = vec![format!(" {icon} {} ", ping.text).fg(color).bold()];
    if let Some(summary) = summary.map(one_line).filter(|s| !s.is_empty()) {
        spans.push(format!(" {} ", fit(&summary, TOAST_SUMMARY_MAX)).fg(color));
    }
    spans.push(" ".into());
    spans
}

/// `text` with its whitespace runs (newlines too) made single spaces.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How often the working spinner advances.
const SPIN_EVERY: Duration = Duration::from_millis(100);
/// Wide enough for the lists plus a preview of the selected session.
/// Rows the wheel scrolls a proposal's text by, a notch.
const WHEEL_ROWS: i32 = 3;
/// The most proposals shown at once; more scroll.
const PROPOSAL_ROWS: usize = 6;

impl App {
    fn draw_home(&self, frame: &mut ratatui::Frame, body: Rect, bar: Rect) {
        let t = self.settings.theme;
        let now = now_ms();
        let spin = (now / SPIN_EVERY.as_millis() as u64) as usize;
        frame.render_widget(
            Block::new().style(style::Style::new().bg(t.bg)),
            frame.area(),
        );

        // The notice line sits over the footer: a toast or what a key said, else a
        // faint rule. Never the hint line (rule 9).
        let [header, main, notice_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(body);
        self.home_rows.borrow_mut().clear();
        let (left, places) = self.strip_line(header.width, now);
        for (from, to, section) in places {
            self.home_rows.borrow_mut().push((
                Rect::new(header.x + from, header.y, to - from, 1),
                Hit::Section(section),
            ));
        }
        // The strip is a bar on the panel color, not loose text (backlog #286).
        frame.render_widget(Block::new().style(style::Style::new().bg(t.panel)), header);
        frame.render_widget(Paragraph::new(left.clone()), header);
        // On the right, the plan's quota and a newer release, each only where it
        // clears the counts on the left; the quota first, it's the one to act on.
        let notice = self
            .newer
            .as_ref()
            .filter(|_| self.settings.updates)
            .map(|newer| {
                vec![
                    format!(" ↑ {newer} ").fg(t.accent).bold(),
                    " valk upgrade ".fg(t.muted),
                ]
            });
        let quota = self.quota.as_ref().map(|q| quota::line(q, now, t).spans);
        let both = notice
            .clone()
            .zip(quota.clone())
            .map(|(notice, quota)| [quota, notice].concat());
        // Then the host, what's running and the time, as far as there is room.
        let places = self.strip_places();
        let notices: Vec<Vec<Span>> = [both, quota, notice].into_iter().flatten().collect();
        if let Some(right) = notices
            .iter()
            .flat_map(|n| places.iter().map(move |p| [n.clone(), p.clone()].concat()))
            .chain(places.iter().cloned())
            .chain([Vec::new()])
            .map(Line::from)
            .find(|right| left.width() + right.width() < header.width as usize)
        {
            frame.render_widget(Paragraph::new(right).right_aligned(), header);
        }

        // Each pane in a quiet frame, a column apart (backlog #286); the detail
        // draws its own.
        let (lists, split) = list_detail_areas(main);
        let preview = split.map(|(_, detail)| detail);
        self.home_cols.set(main.width);
        let inbox_area = lists;
        let in_proposals = self.proposal().is_some();
        if self.narrow_detail() {
            // Narrow: the selected session's detail is the screen; Esc goes back.
            self.draw_preview(frame, inbox_area);
        } else {
            // No padding: the rows draw their own margin, the ▌ in it.
            let block = frame_block(self.inbox_title(), t).padding(Padding::ZERO);
            let inner = block.inner(inbox_area);
            frame.render_widget(block, inbox_area);
            self.draw_inbox(frame, inner, now, spin);
        }

        if let Some(area) = preview {
            self.draw_preview(frame, area);
            self.home_rows.borrow_mut().push((area, Hit::Preview));
        }
        if let Some(colors) = &t.rain {
            let inside = Margin::new(1, 1);
            let regions: Vec<Rect> = [Some(inbox_area), preview]
                .into_iter()
                .flatten()
                .filter(|r| r.height > 2)
                .map(|r| r.inner(inside))
                .collect();
            let screen = frame.area();
            // A pack's avatar takes the rows the rain may use, read before it falls.
            let avatar = avatar::of(t).and_then(|a| {
                let mut free: Vec<Vec<bool>> = regions
                    .iter()
                    .map(|r| rain::free_rows(frame.buffer_mut(), r.intersection(screen)))
                    .collect();
                let i = avatar::pick(&regions, &free)?;
                Some((a, regions[i].intersection(screen), free.swap_remove(i)))
            });
            rain::draw(frame.buffer_mut(), screen, &regions, now, colors);
            if let Some((a, region, free)) = avatar {
                let mood = self.avatar_mood();
                avatar::draw(frame.buffer_mut(), screen, (region, &free), now, t, a, mood);
            }
        }

        let (notice_line, toast_cols) = self.notice_line(notice_area.width);
        if let Some(cols) = toast_cols {
            self.home_rows.borrow_mut().push((
                Rect::new(notice_area.x, notice_area.y, cols, 1),
                Hit::Key(HomeKey::Toast),
            ));
        }
        frame.render_widget(Paragraph::new(notice_line), notice_area);

        let (footer, chips) = match &self.renaming {
            Some((id, typed)) => (rename_line(*id, typed, t), Vec::new()),
            None => self.footer_line(bar.width, in_proposals),
        };
        self.home_rows.borrow_mut().extend(
            chips.into_iter().map(|(from, to, key)| {
                (Rect::new(bar.x + from, bar.y, to - from, 1), Hit::Key(key))
            }),
        );
        frame.render_widget(
            Paragraph::new(footer).style(style::Style::new().bg(t.panel)),
            bar,
        );
        // Start agents over a dimmed home (rule 8), but not the notice row or
        // the footer: what a start said, and the dialog's keys, stay readable.
        if let Some(b) = &self.batch {
            let behind = Rect {
                height: body.height.saturating_sub(1),
                ..body
            };
            dim(frame, behind, t);
            b.draw(frame, t);
        }
    }

    /// The line over the footer: a toast for a few seconds, else what a key just
    /// said, else a faint rule. With how wide a toast is, for its click.
    fn notice_line(&self, width: u16) -> (Line<'static>, Option<u16>) {
        let t = self.settings.theme;
        if let Some(toast) = self.toast() {
            let spans = self.toast_line(toast);
            let cols = spans.iter().map(|s| s.width() as u16).sum();
            return (Line::from(spans), toast.proposal.map(|_| cols));
        }
        let said = if self.kill_armed.is_some() {
            let name = self
                .kill_armed
                .and_then(|id| self.sessions.iter().find(|s| s.id == id))
                .map_or(String::new(), |s| s.name.clone());
            format!("close {name}? x again closes it · any other key keeps it")
        } else {
            self.status.clone()
        };
        if said.is_empty() {
            // Why the supervisor offers the selected row's step, beside its keys.
            if let Some((_, _, step)) = self.offered() {
                return (
                    Line::from(format!(" {} ", step.why).fg(t.muted).italic()),
                    None,
                );
            }
            return (Line::from("─".repeat(width as usize).fg(t.border)), None);
        }
        (Line::from(format!(" {said} ").fg(t.fg).bold()), None)
    }

    /// The footer: at most five keys for what's selected, the first the one
    /// that does the most, then the ones for getting around, faint, on the
    /// right (spec: One key grammar). Each key's columns, for a click.
    fn footer_line(
        &self,
        width: u16,
        on_proposal: bool,
    ) -> (Line<'static>, Vec<(u16, u16, HomeKey)>) {
        let t = self.settings.theme;
        // A dialog's footer is its own (rule 8).
        if let Some(b) = &self.batch {
            let mut spans: Vec<Span> = Vec::new();
            for (key, what) in batch::hints(b) {
                spans.push(" ".into());
                spans.push(format!(" {key} ").fg(t.accent).bold());
                spans.push(format!("{what} ").fg(t.dim()));
            }
            return (Line::from(spans), Vec::new());
        }
        let step = self.offered().map(|(_, _, step)| step);
        let asks = matches!(self.selection(), (_, Some(q)) if answerable(q));
        let mut hints: Vec<(String, String, HomeKey)> = Vec::new();
        let mut hint = |k: &str, w: &str, key: HomeKey| hints.push((k.into(), w.into(), key));
        // Typing: only the composer's keys, since letters type (spec: Home composer).
        let typing = self.composing();
        if typing && self.composer.item_query().is_some() {
            let found = self.compose_found();
            match found.get(self.composer.pick) {
                Some(
                    f @ compose::Found {
                        what: compose::Can::Start,
                        ..
                    },
                ) => hint("↩", &format!("start {}", f.label), HomeKey::Compose),
                Some(
                    f @ compose::Found {
                        what: compose::Can::Open(..),
                        ..
                    },
                ) => hint("↩", &format!("open {}", f.label), HomeKey::Compose),
                _ => {}
            }
            hint("↑↓", "pick", HomeKey::Compose);
            hint("esc", "back to text", HomeKey::Compose);
        } else if typing {
            let lines = self
                .composer
                .task()
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count();
            if lines > 1 {
                hint("↩", &format!("review and start {lines}"), HomeKey::Compose);
            } else {
                hint("↩", "start", HomeKey::Compose);
            }
            hint("^f", "file to backlog", HomeKey::Compose);
            hint("#", "item", HomeKey::Compose);
            hint("tab", "repo", HomeKey::Compose);
            hint("esc", "done", HomeKey::Compose);
        } else if self.kill_armed.is_some() {
            hint("x", "close it", HomeKey::Kill);
        } else if self.narrow_detail() {
            hint("↩", "attach", HomeKey::Enter);
            hint("esc", "back", HomeKey::Back);
        } else if let Some(step) = &step {
            hint("a", step.action.label(), HomeKey::Accept);
            if let Some(alt) = step.alt {
                hint("o", alt.label(), HomeKey::Other);
            }
            hint("d", "leave it", HomeKey::Reject);
            hint("↩", "open", HomeKey::Enter);
        } else if on_proposal {
            let d = self.proposal();
            if d.is_some_and(|d| d.fresh.review.is_some()) {
                hint("a", "still holds", HomeKey::Accept);
                hint("d", "retire", HomeKey::Reject);
            } else if d.and_then(scoping).is_some() {
                hint("a", "scope it", HomeKey::Accept);
                hint("d", "keep everywhere", HomeKey::Reject);
            } else {
                hint("a", "accept", HomeKey::Accept);
                hint("d", "reject", HomeKey::Reject);
            }
            hint("↩", "review all", HomeKey::Enter);
        } else {
            match self.row() {
                Some(InboxRow::Compose) => {
                    hint("n", "type", HomeKey::Compose);
                    if let Some(top) =
                        compose::ready(&self.items, &self.sessions, self.boot()).first()
                    {
                        hint(
                            "s",
                            &format!("start {}", compose::label(top)),
                            HomeKey::Supervisor,
                        );
                    }
                }
                Some(InboxRow::Decide) => hint("↩", "open", HomeKey::Enter),
                Some(InboxRow::Filed) => hint("↩", "triage", HomeKey::Enter),
                Some(InboxRow::Disk) => hint("↩", "free space", HomeKey::Enter),
                Some(_) => {
                    hint("↩", "open", HomeKey::Enter);
                    if asks {
                        hint("1-9", "answer", HomeKey::Answer(1));
                    }
                    hint("x", "close", HomeKey::Kill);
                    hint("r", "rename", HomeKey::Rename);
                    if self.adoptable().is_some() {
                        hint("A", "adopt", HomeKey::Adopt);
                    }
                }
                None => hint("n", "new session", HomeKey::Compose),
            }
        }
        hints.truncate(5);
        let mut spans: Vec<Span> = Vec::new();
        let mut chips = Vec::new();
        let at = |spans: &[Span]| spans.iter().map(|s| s.width() as u16).sum::<u16>();
        for (key, what, home) in hints {
            spans.push(" ".into());
            let from = at(&spans);
            spans.push(format!(" {key} ").fg(t.accent).bold());
            spans.push(format!("{what} ").fg(t.dim()));
            chips.push((from, at(&spans), home));
        }
        // Getting around, faint, as much as there is room for; `? keys` last
        // to go, since it leads to the rest.
        let left = at(&spans) as usize;
        let around: &[&str] = if typing {
            &["shift-tab agent "]
        } else if self.row() == Some(InboxRow::Compose) {
            // `n type` is already the first hint.
            &[
                "j/k move · g sections · ? keys ",
                "j/k move · ? keys ",
                "? keys ",
            ]
        } else {
            &[
                "n new · j/k move · g sections · ? keys ",
                "j/k move · ? keys ",
                "? keys ",
            ]
        };
        let around = around
            .iter()
            .find(|a| left + a.chars().count() + 2 <= width as usize);
        if let Some(around) = around {
            let pad = (width as usize).saturating_sub(left + around.chars().count());
            spans.push(" ".repeat(pad).into());
            spans.push(around.fg(t.muted));
        }
        (Line::from(spans), chips)
    }

    /// Under 100 columns the composer is in the list, not a pane of its own.
    fn compose_inline(&self) -> bool {
        self.home_cols.get() < 100
    }

    /// The composer's lines in the Inbox list: "+ New session" beside a pane,
    /// else the band itself, its defaults and any `#` matches.
    fn compose_row_lines(&self, width: usize) -> Vec<Line<'static>> {
        let t = self.settings.theme;
        if !self.compose_inline() {
            let name = " + New session";
            let pad = width.saturating_sub(name.chars().count() + 4);
            // The first cell is the cursor's (`▌`), as on every row.
            return vec![Line::from(vec![
                " ".into(),
                "+ ".fg(t.accent).bold(),
                "New session".fg(t.fg).bold(),
                " ".repeat(pad).into(),
                "n".fg(t.muted),
            ])];
        }
        let selected = self.row() == Some(InboxRow::Compose);
        let mut lines = compose::band(&self.composer, width, 3, selected, t);
        lines.extend(self.compose_under(width));
        lines
    }

    /// Under the band: the defaults and what changes them, then the `#` matches.
    fn compose_under(&self, width: usize) -> Vec<Line<'static>> {
        let t = self.settings.theme;
        if self.composer.item_query().is_some() && self.composing() {
            return compose::match_lines(&self.compose_found(), self.composer.pick, width, t);
        }
        let (_, d) = self.compose_defaults();
        let mut spans = vec!["   ".into()];
        spans.extend(compose::defaults_line(&d, t));
        spans.push(if self.composing() {
            "   tab repo · shift-tab agent".fg(t.muted)
        } else {
            "   tab change".fg(t.muted)
        });
        vec![Line::from(spans)]
    }

    /// The pane beside the list on the composer: what's next, the band, the
    /// ready items `s` starts, and the supervisor on the last line.
    fn draw_composer(&self, frame: &mut ratatui::Frame, area: Rect) {
        let t = self.settings.theme;
        let now = now_ms();
        let area = area.inner(Margin::new(2, 0));
        let width = area.width as usize;
        let need = self.need_you();
        let mut lines = vec![
            Line::default(),
            Line::default(),
            Line::from("What's next?".fg(t.fg).bold()),
            Line::from(
                if need == 0 {
                    "Nothing needs you. Start an agent, or pick up the backlog.".to_owned()
                } else {
                    format!("{need} wait on you: tab goes to them. Or start something new.")
                }
                .fg(t.muted),
            ),
            Line::default(),
        ];
        lines.extend(compose::band(&self.composer, width, 4, true, t));
        lines.extend(self.compose_under(width));
        let ready = compose::ready(&self.items, &self.sessions, self.boot());
        let open = self
            .items
            .iter()
            .filter(|i| i.status == valkyrie_proto::ItemStatus::Open)
            .count();
        lines.extend([Line::default(), Line::default()]);
        let head = "Ready in the backlog";
        let count = format!("{open} in Backlog");
        lines.push(Line::from(vec![
            head.fg(t.fg).bold(),
            " ".repeat(width.saturating_sub(head.len() + count.len()))
                .into(),
            count.fg(t.muted),
        ]));
        lines.push(Line::default());
        if ready.is_empty() {
            lines.push(Line::from("nothing ready: b opens the backlog".fg(t.muted)));
        }
        for (i, item) in ready.iter().take(5).enumerate() {
            let key = if i == 0 { "s  " } else { "   " };
            let label = pad(&fit(&compose::label(item), 9), 10);
            let facts = format!("{:<8}{}", item.kind.as_str(), item.priority.as_str());
            let room = width.saturating_sub(3 + 10 + facts.len() + 2).max(8);
            lines.push(Line::from(vec![
                key.fg(t.accent).bold(),
                label.fg(t.muted),
                pad(&fit(&item.title, room), room).fg(t.fg),
                "  ".into(),
                facts.fg(t.muted),
            ]));
        }
        let [body, foot] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(2)]).areas(area);
        frame.render_widget(Paragraph::new(lines), body);
        if let Some(g) = &self.glance {
            let line = if !g.on {
                "◦ supervisor off".to_owned()
            } else {
                let mut said = format!("◦ supervisor watching {}", g.watching);
                if let Some(last) = &g.last {
                    said.push_str(&format!(" · {} {}", last.said, age(last.at * 1000, now)));
                }
                said
            };
            frame.render_widget(
                Paragraph::new(Line::from(fit(&line, width).fg(t.muted))),
                foot,
            );
        }
    }

    /// The Inbox: Needs you, Running, Quiet, each session once (spec: Inbox). A
    /// group shows only while it has rows; its state's mark, name and count head
    /// it, then a hairline to the frame (backlog #286). `area` is inside the frame.
    fn draw_inbox(&self, frame: &mut ratatui::Frame, area: Rect, now: u64, spin: usize) {
        let t = self.settings.theme;
        let others = self.others();
        let running = others.iter().take_while(|s| is_running(s)).count();
        let inner = area;

        if self.need_you() == 0 && others.is_empty() {
            let calm = if t.rain.is_some() {
                Line::from(vec![
                    "> ".fg(t.accent).bold(),
                    "all clear".fg(t.fg).bold(),
                    " · nothing needs you ".fg(t.muted),
                    rain::cursor(now).fg(t.accent),
                ])
            } else {
                Line::from(vec![
                    "✓ ".fg(t.done).bold(),
                    "all clear".fg(t.fg).bold(),
                    "  ·  nothing needs you".fg(t.muted),
                ])
            }
            .centered();
            // Nothing running at all: most likely someone new, so say where to learn.
            let lines = vec![
                calm,
                Line::default(),
                Line::from(vec![
                    "new here? ".fg(t.muted),
                    " ? ".fg(t.accent).bold(),
                    " starts the tutor".fg(t.muted),
                ])
                .centered(),
            ];
            let top = self.compose_row_lines(inner.width as usize);
            let rows = top.len() as u16;
            let [head, _, mid, _] = Layout::vertical([
                Constraint::Length(rows),
                Constraint::Fill(1),
                Constraint::Length(lines.len() as u16),
                Constraint::Fill(1),
            ])
            .areas(inner);
            self.home_rows.borrow_mut().push((head, Hit::Row(0)));
            frame.render_widget(Paragraph::new(top), head);
            frame.render_widget(Paragraph::new(lines), mid);
            return;
        }

        // Each drawn line, and the selection row it is (none for labels).
        let width = inner.width as usize;
        let rows = self.inbox_rows();
        let mut lines: Vec<(Option<usize>, Line<'static>)> = Vec::new();
        let label = |lines: &mut Vec<(Option<usize>, Line<'static>)>, name: &str, n: usize| {
            if !lines.is_empty() {
                lines.push((None, Line::default()));
            }
            // Color means state (rule 4): the group's, on its mark and name.
            let (mark, color) = match name {
                "Needs you" => ("●", t.needs),
                "Running" => ("◐", t.working),
                _ => ("○", t.muted),
            };
            let head = format!(" {mark} {name} {n} ");
            let rule = width.saturating_sub(head.chars().count() + 1);
            lines.push((
                None,
                Line::from(vec![
                    format!(" {mark} {name} ").fg(color).bold(),
                    n.to_string().fg(t.dim()),
                    " ".into(),
                    "─".repeat(rule).fg(t.border),
                ]),
            ));
        };
        lines.extend(
            self.compose_row_lines(width)
                .into_iter()
                .map(|line| (Some(0), line)),
        );
        if self.need_you() == 0 {
            lines.push((None, Line::default()));
            lines.push((
                None,
                Line::from(vec![
                    " ✓ ".fg(t.done).bold(),
                    "nothing needs you".fg(t.muted),
                ]),
            ));
        } else {
            label(&mut lines, "Needs you", self.need_you());
        }
        for (i, row) in rows.iter().enumerate() {
            let line = match *row {
                // Drawn above, its own lines.
                InboxRow::Compose => continue,
                InboxRow::Queue(q) => inbox_queue_row(&self.queue[q], now, spin, width, t),
                InboxRow::Decide => self.decide_row(now, width),
                InboxRow::Filed => self.filed_row(now, width),
                InboxRow::Disk => self.disk_row(width),
                InboxRow::Session(id) => {
                    let s = others.iter().find(|s| s.id == id).expect("listed");
                    // A group's label before its first row.
                    let first = i == 0 || !matches!(rows[i - 1], InboxRow::Session(_));
                    if first && is_running(s) {
                        label(&mut lines, "Running", running);
                    } else if !is_running(s)
                        && (first || {
                            let InboxRow::Session(before) = rows[i - 1] else {
                                unreachable!()
                            };
                            others.iter().any(|o| o.id == before && is_running(o))
                        })
                    {
                        label(&mut lines, "Quiet", others.len() - running);
                    }
                    inbox_session_row(s, now, spin, width, t)
                }
            };
            lines.push((Some(i), line));
        }

        // Keep the cursor's row in sight.
        let height = inner.height as usize;
        let at = lines
            .iter()
            .rposition(|(row, _)| *row == Some(self.selected));
        let offset = match at {
            Some(at) if at + 1 > height => at + 1 - height,
            _ => 0,
        };
        let mut hits = self.home_rows.borrow_mut();
        let shown: Vec<Line> = lines
            .into_iter()
            .skip(offset)
            .take(height)
            .enumerate()
            .map(|(y, (row, line))| {
                let Some(row) = row else { return line };
                let rect = Rect::new(inner.x, inner.y + y as u16, inner.width, 1);
                hits.push((rect, Hit::Row(row)));
                // The composer in the list draws its own selection: a band.
                if row == 0 && self.compose_inline() {
                    return line;
                }
                if row == self.selected {
                    let mut spans = vec!["▌".fg(t.accent)];
                    spans.extend(line.spans.into_iter().skip(1));
                    t.lift_faint(Line::from(spans))
                        .style(style::Style::new().bg(t.selection))
                } else {
                    line
                }
            })
            .collect();
        frame.render_widget(Paragraph::new(shown), inner);
    }

    /// The Decide row: `■ 3 decisions`, then what they are.
    fn decide_row(&self, now: u64, width: usize) -> Line<'static> {
        let t = self.settings.theme;
        let n = self.to_decide();
        let mut what: Vec<String> = self.proposals.iter().map(|d| d.title.clone()).collect();
        what.extend(self.launches.iter().map(|l| format!("start {}", l.title)));
        let since = self
            .proposals
            .iter()
            .map(|d| d.created)
            .chain(self.launches.iter().map(|l| l.made))
            .min()
            .unwrap_or_default();
        record_row(
            "■".fg(t.accent),
            &format!("{n} decision{}", if n == 1 { "" } else { "s" }),
            vec![what.join(" · ").fg(t.secondary())],
            age(since * 1000, now),
            width,
            t,
        )
    }

    /// The low-disk row: `■ disk low`, then what's free on each.
    fn disk_row(&self, width: usize) -> Line<'static> {
        let t = self.settings.theme;
        let text = self
            .disk
            .iter()
            .map(|d| {
                format!(
                    "{} free · {}",
                    valkyrie_proto::disk::bytes(d.free),
                    short_path(&d.path)
                )
            })
            .collect::<Vec<_>>()
            .join(" · ");
        record_row(
            "■".fg(t.blocked),
            "disk low",
            vec![text.fg(t.secondary())],
            String::new(),
            width,
            t,
        )
    }

    /// Deletes the idle build output of every worktree, off the draw loop: it
    /// can take a while with tens of gigabytes. Says what it freed.
    fn free_disk(&mut self) {
        let Some(said) = self.said.clone() else {
            return;
        };
        let client = self.client.clone();
        self.say("deleting idle build output…".into());
        tokio::spawn(async move {
            let line = match client.free_disk().await {
                Ok(text) => text,
                Err(e) => format!("couldn't free space: {e:#}"),
            };
            let _ = said.send(line);
        });
    }

    /// The Filed row: `■ 8 filed`, then how many in each repo.
    fn filed_row(&self, now: u64, width: usize) -> Line<'static> {
        let t = self.settings.theme;
        let filed = self.filed();
        let mut repos: Vec<(String, usize)> = Vec::new();
        for item in &filed {
            let repo = item.project.as_deref().map_or("no repo".into(), base_name);
            match repos.iter_mut().find(|(r, _)| *r == repo) {
                Some((_, n)) => *n += 1,
                None => repos.push((repo, 1)),
            }
        }
        let text = repos
            .iter()
            .map(|(r, n)| format!("{r} {n}"))
            .collect::<Vec<_>>()
            .join(" · ");
        let since = filed.first().map_or(0, |i| i.created);
        record_row(
            "■".fg(t.accent_dim()),
            &format!("{} filed", filed.len()),
            vec![text.fg(t.secondary())],
            age(since * 1000, now),
            width,
            t,
        )
    }

    /// The selected session: its full summary, then the bottom of its screen.
    fn draw_preview(&self, frame: &mut ratatui::Frame, area: Rect) {
        let t = self.settings.theme;
        if self.row() == Some(InboxRow::Compose) {
            let block = frame_block(
                Line::from(vec![
                    " + ".fg(t.accent).bold(),
                    "New session ".fg(t.fg).bold(),
                ]),
                t,
            );
            let inner = block.inner(area);
            frame.render_widget(block, area);
            return self.draw_composer(frame, inner);
        }
        if let Some(d) = self.proposal() {
            let title = Line::from(vec![
                " ■ ".fg(t.accent).bold(),
                d.title.clone().fg(t.fg).bold(),
                format!(" #{} ", d.id).fg(t.muted),
            ]);
            let lines = vec![Line::from(
                format!(
                    "{}  ·  {} · {}{}",
                    d.kind.as_str(),
                    proposer(d),
                    scope_name(&d.project, short_path),
                    d.supersedes
                        .map(|n| format!(" · replaces #{n}"))
                        .unwrap_or_default()
                )
                .fg(t.muted),
            )];
            let mut lines = lines;
            // The row counts the supervisor's offers too; Enter opens the
            // proposals, so say where the rest are.
            let offers = self.launches.len();
            if offers > 0 {
                lines.push(Line::from(
                    format!("and {offers} for the supervisor: s opens them").fg(t.muted),
                ));
            }
            let block = frame_block(title, t);
            let inner = block.inner(area);
            frame.render_widget(block, area);
            self.draw_proposal_text(frame, inner, d, lines);
            return;
        }
        if let Some(row @ (InboxRow::Decide | InboxRow::Filed | InboxRow::Disk)) = self.row() {
            let (title, lines): (&str, Vec<Line>) = match row {
                InboxRow::Disk => (
                    "disk low",
                    self.disk
                        .iter()
                        .map(|d| {
                            Line::from(vec![
                                format!(
                                    "{} free of {}  ",
                                    valkyrie_proto::disk::bytes(d.free),
                                    valkyrie_proto::disk::bytes(d.total)
                                )
                                .fg(t.fg),
                                short_path(&d.path).fg(t.muted),
                            ])
                        })
                        .chain([
                            Line::default(),
                            "Agents' worktrees build their own target/, up to 24 GB each."
                                .fg(t.secondary())
                                .into(),
                            Line::default(),
                            "↩ delete every worktree's target/ nothing built in for an hour"
                                .fg(t.muted)
                                .into(),
                        ])
                        .collect(),
                ),
                InboxRow::Filed => (
                    "filed by agents, not triaged",
                    self.filed()
                        .iter()
                        .map(|i| {
                            let repo = i.project.as_deref().map_or("no repo".into(), base_name);
                            Line::from(vec![
                                format!("{repo}#{}  ", i.number).fg(t.muted),
                                i.title.clone().fg(t.fg),
                            ])
                        })
                        .chain([
                            Line::default(),
                            "↩ triage in the backlog".fg(t.muted).into(),
                        ])
                        .collect(),
                ),
                _ => (
                    "for the supervisor",
                    self.launches
                        .iter()
                        .map(|l| Line::from(vec!["start  ".fg(t.muted), l.title.clone().fg(t.fg)]))
                        .chain([Line::default(), "↩ open the supervisor".fg(t.muted).into()])
                        .collect(),
                ),
            };
            let block = frame_block(Line::from(format!(" {title} ").fg(t.fg).bold()), t);
            let inner = block.inner(area);
            frame.render_widget(block, area);
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
            return;
        }
        let (selected, _) = self.selection();
        let session = selected.and_then(|id| self.sessions.iter().find(|s| s.id == id));
        let Some(session) = session else {
            return;
        };
        self.draw_session_preview(frame, area, session, false);
    }

    /// A session's state, full summary, then the bottom of its screen, in a panel.
    fn draw_session_preview(
        &self,
        frame: &mut ratatui::Frame,
        area: Rect,
        session: &SessionInfo,
        focused: bool,
    ) {
        let t = self.settings.theme;
        let name = session.name.clone().fg(t.fg).bold();
        let name = if session.auto_name {
            name.italic()
        } else {
            name
        };
        let title = Line::from(vec![
            " ▸ ".fg(t.accent).bold(),
            name,
            format!(" #{} ", session.id).fg(t.muted),
        ]);
        // The accent panel over a session, where it's a dialog; at home, a quiet frame.
        let block = if focused {
            panel(title, t, true)
        } else {
            frame_block(title, t)
        };
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let status = &session.status;
        let color = t.state(status.state);
        let mut head = vec![Line::from(vec![
            format!(
                "{} {}",
                state_icon(status.state, 0),
                state_word(status.state)
            )
            .fg(color)
            .bold(),
            format!("  {}  ·  {}", where_from(session), short_path(&session.cwd)).fg(t.muted),
        ])];
        if let Some(summary) = &status.summary {
            head.push(Line::from(summary.clone().fg(t.fg)));
        }
        let head_height = head.len() as u16 + 1 + u16::from(status.summary.is_some()); // a gap, room to wrap
        let [top, screen] =
            Layout::vertical([Constraint::Length(head_height), Constraint::Fill(1)]).areas(inner);
        frame.render_widget(Paragraph::new(head).wrap(Wrap { trim: true }), top);

        // The screen sits in a recessed inset, out to the frame's padding, so an
        // agent's own prompt reads as its screen and not as more of ours.
        let surface = if focused { t.panel } else { t.bg };
        let inset = Rect {
            x: screen.x.saturating_sub(1).max(area.x + 1),
            width: (screen.width + 2).min(area.width.saturating_sub(2)),
            ..screen
        };
        if inset.height == 0 {
            return;
        }
        let bg = t.recessed(surface);
        frame.render_widget(Block::new().style(style::Style::new().bg(bg)), inset);
        let [label, screen] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(Rect {
                x: inset.x + 1,
                width: inset.width.saturating_sub(2),
                ..inset
            });

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
        let shown = end - start;
        let mut left = vec![
            "screen".fg(t.dim()).bold(),
            if session.exited.is_some() {
                " · ended".fg(t.muted)
            } else {
                " · live".fg(t.muted)
            },
        ];
        if shown > 0 {
            left.push(
                format!(" · last {shown} line{}", if shown == 1 { "" } else { "s" }).fg(t.muted),
            );
        }
        frame.render_widget(Paragraph::new(Line::from(left)), label);
        // At home Enter attaches; the peek says what a click does itself.
        if !focused {
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    "↩".fg(t.accent).bold(),
                    " attach".fg(t.muted),
                ]))
                .alignment(ratatui::layout::Alignment::Right),
                label,
            );
        }
        let body: Vec<Line> = lines[start..end]
            .iter()
            .map(|l| Line::from(l.to_string().fg(t.muted)))
            .collect();
        frame.render_widget(Paragraph::new(body), screen);
    }
}

impl App {
    /// A look at the tab the mouse rests on, over the attached session, next to
    /// that tab, saying what a click does.
    fn draw_peek(
        &self,
        frame: &mut ratatui::Frame,
        main: Rect,
        strip: Option<Rect>,
        session: &SessionInfo,
    ) {
        let width = (main.width * 3 / 5).max(PEEK_MIN_WIDTH).min(main.width);
        let height = (main.height / 2).max(PEEK_MIN_HEIGHT).min(main.height);
        let tab = strip.and_then(|strip| {
            let layout = self.tab_layout(strip);
            let tab = layout
                .tabs
                .iter()
                .find(|&&(i, _)| Some(i) == self.peek_tab());
            tab.map(|&(_, r)| r)
        });
        let (x, y) = match (self.settings.tab_side, tab) {
            (TabSide::Top, Some(r)) => (r.x.min(main.right() - width), main.y),
            // Beside the strip, which an open rail lays over the session.
            (TabSide::Left, Some(r)) => (r.right() + 1, r.y.min(main.bottom() - height)),
            (TabSide::Right, Some(r)) => (
                r.x.saturating_sub(width + 1).min(main.right() - width),
                r.y.min(main.bottom() - height),
            ),
            _ => (main.x, main.y),
        };
        let rect = Rect::new(x.max(main.x), y.max(main.y), width, height).intersection(main);
        frame.render_widget(ratatui::widgets::Clear, rect);
        self.draw_session_preview(frame, rect, session, true);
        if rect.height < 3 {
            return;
        }
        let t = self.settings.theme;
        let keys = [("click", "switch"), ("right-click", "rename · close")];
        let hints = Line::from(
            keys.iter()
                .flat_map(|(key, what)| {
                    [
                        format!(" {key} ").fg(t.accent).bold(),
                        format!("{what} ").fg(t.muted),
                    ]
                })
                .collect::<Vec<_>>(),
        );
        let bottom = Rect::new(
            rect.x + 1,
            rect.bottom() - 1,
            rect.width.saturating_sub(2),
            1,
        );
        frame.render_widget(Paragraph::new(hints.centered()), bottom);
    }

    /// The decisions to review, over the attached session: the list, then the one
    /// under the cursor in full.
    fn draw_reviews(&self, frame: &mut ratatui::Frame, area: Rect, cursor: usize) {
        let t = self.settings.theme;
        let Some(d) = self.proposals.get(cursor) else {
            return;
        };
        let rows = self.proposals.len().min(PROPOSAL_ROWS) as u16;
        let width = area.width.min(REVIEWS_MAX_WIDTH);
        let height = (rows + REVIEWS_BODY_ROWS + 3).min(area.height);
        let rect = Rect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        );
        let title = Line::from(vec![
            " ■ ".fg(t.accent).bold(),
            "decisions to review ".fg(t.fg).bold(),
            format!("{} ", self.proposals.len()).fg(t.muted),
        ]);
        let flagged = d.fresh.review.is_some();
        let keys: &[(&str, &str)] = if flagged {
            &[
                ("j/k", "move"),
                ("a", "still holds"),
                ("d", "retire"),
                ("esc", "back"),
            ]
        } else if scoping(d).is_some() {
            &[
                ("j/k", "move"),
                ("a", "scope it"),
                ("d", "keep everywhere"),
                ("esc", "back"),
            ]
        } else {
            &[
                ("j/k", "move"),
                ("pgup/dn", "scroll"),
                ("a", "accept"),
                ("d", "reject"),
                ("esc", "back"),
            ]
        };
        let hints = Line::from(
            keys.iter()
                .flat_map(|(key, what)| {
                    [
                        format!(" {key} ").fg(t.accent).bold(),
                        format!("{what} ").fg(t.muted),
                    ]
                })
                .collect::<Vec<_>>(),
        );
        let block = panel(title, t, true).title_bottom(hints.centered());
        let inner = block.inner(rect);
        frame.render_widget(ratatui::widgets::Clear, rect);
        frame.render_widget(block, rect);

        let [list, rule, body] = Layout::vertical([
            Constraint::Length(rows),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(inner);
        let mut state = TableState::default().with_selected(Some(cursor));
        frame.render_stateful_widget(
            proposal_table(self.proposals.iter().map(|d| proposal_row(d, t)), t),
            list,
            &mut state,
        );
        frame.render_widget(
            Paragraph::new("─".repeat(rule.width as usize).fg(t.border)),
            rule,
        );
        let mut lines = vec![Line::from(
            format!(
                "{}  ·  {} · {}",
                d.kind.as_str(),
                proposer(d),
                scope_name(&d.project, short_path)
            )
            .fg(t.muted),
        )];
        if let Some(why) = &d.fresh.review {
            lines.push(Line::from(
                format!("may be out of date: {why}").fg(t.accent),
            ));
        } else if let Some(scope) = scoping(d) {
            lines.push(Line::from(
                format!(
                    "scope it to {}? agents get it when they open these",
                    scope.join(", ")
                )
                .fg(t.accent),
            ));
        } else if !d.scope.is_empty() {
            // Accepting it takes its scope too, so that's shown before you do.
            lines.push(Line::from(
                format!("applies to {}", d.scope.join(", ")).fg(t.muted),
            ));
        }
        self.draw_proposal_text(frame, body, d, lines);
    }
}

/// Where a proposal's text is scrolled to (`App::proposal_scroll`).
#[derive(Clone, Copy, Default)]
struct ProposalScroll {
    /// Its project (hashed, to stay `Copy`), id and version; another resets it.
    whose: Option<(u64, u32, u64)>,
    top: u16,
    rows: u16,
    /// Where the text was drawn, so the wheel knows when it is over it.
    area: Rect,
}

fn fnv_path(path: &std::path::Path) -> u64 {
    path.as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
        })
}

/// A peek's smallest size, when the session leaves room for it.
const PEEK_MIN_WIDTH: u16 = 50;
const PEEK_MIN_HEIGHT: u16 = 12;
/// The decisions list over a session: its widest, and the rows for the body.
const REVIEWS_MAX_WIDTH: u16 = 100;
const REVIEWS_BODY_ROWS: u16 = 8;
/// Shorter terminals leave the session every row.
const TABS_MIN_ROWS: u16 = 12;
/// The column of tabs beside the session: the widest tab, its rule and a margin.
const SIDE_WIDTH: u16 = TAB_MAX + 2;
/// The column of minimal tabs: a short name, its icon and number. Beside the
/// session only on top; at the side it is the rail, open.
const MINIMAL_SIDE_WIDTH: u16 = 18;
/// The rail, shut: each tab's number and icon, and the rule.
const RAIL_WIDTH: u16 = 6;
/// Narrower terminals leave the session every column.
const SIDE_MIN_COLS: u16 = 70;
/// A tab's width bounds, padding included; a minimal tab can be narrower.
const TAB_MIN: u16 = 10;
const MINIMAL_TAB_MIN: u16 = 6;
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
                "Zoom / unzoom",
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
        let (main, tabs) = self.split_tabs(body);
        (self.beside_split(main).0, tabs)
    }

    /// The session's area and the docs column beside it (backlog #311), while
    /// one is open on the attached session. On a narrow screen the docs take it
    /// all, and the panes keep their size under them.
    fn beside_split(&self, main: Rect) -> (Rect, Option<Rect>) {
        if !self.beside_shown() {
            return (main, None);
        }
        if main.width < BESIDE_MIN_COLS {
            return (main, Some(main));
        }
        let [panes, docs] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(main);
        (panes, Some(docs))
    }

    /// The docs beside a session: a hairline to the panes when they share the
    /// screen (a frame, not a dialog: nothing behind it dims), then the page.
    fn draw_beside(&self, frame: &mut ratatui::Frame, area: Rect, beside: bool, page: &docs::Page) {
        let t = self.settings.theme;
        let area = match beside {
            true => {
                let rule = Rect { width: 1, ..area };
                frame.render_widget(ratatui::widgets::Clear, rule);
                let line: Vec<Line> = (0..area.height).map(|_| "│".fg(t.dim()).into()).collect();
                frame.render_widget(Paragraph::new(line), rule);
                Rect {
                    x: area.x + 1,
                    width: area.width.saturating_sub(1),
                    ..area
                }
            }
            false => area,
        };
        page.draw_in(frame, area, t);
    }

    /// The docs column on screen now, if one is open.
    fn beside_area(&self) -> Option<Rect> {
        let full = self.screen().ok()?;
        let (main, _) = self.split_tabs(Rect::new(0, 0, full.cols, full.rows));
        self.beside_split(main).1
    }

    /// The body split into the session's area and, when shown, the tab strip's.
    fn split_tabs(&self, body: Rect) -> (Rect, Option<Rect>) {
        let side = |at| Layout::horizontal(at).areas::<2>(body);
        let width = match (self.rail(), self.settings.minimal_tabs) {
            (true, _) => RAIL_WIDTH,
            (false, true) => MINIMAL_SIDE_WIDTH,
            (false, false) => SIDE_WIDTH,
        };
        // An open rail lies over the session, which keeps its size.
        let open = |tabs: Rect| {
            if !self.rail_open() {
                return tabs;
            }
            let width = MINIMAL_SIDE_WIDTH.min(body.width);
            match self.settings.tab_side {
                TabSide::Right => Rect {
                    x: tabs.right() - width,
                    width,
                    ..tabs
                },
                _ => Rect { width, ..tabs },
            }
        };
        match self.settings.tab_side {
            TabSide::Top if body.height >= TABS_MIN_ROWS => {
                let rows = self.settings.tabs.rows(self.settings.minimal_tabs);
                let [tabs, main] =
                    Layout::vertical([Constraint::Length(rows), Constraint::Fill(1)]).areas(body);
                (main, Some(tabs))
            }
            TabSide::Left if body.width >= SIDE_MIN_COLS && body.height >= TABS_MIN_ROWS => {
                let [tabs, main] = side([Constraint::Length(width), Constraint::Fill(1)]);
                (main, Some(open(tabs)))
            }
            TabSide::Right if body.width >= SIDE_MIN_COLS && body.height >= TABS_MIN_ROWS => {
                let [main, tabs] = side([Constraint::Fill(1), Constraint::Length(width)]);
                (main, Some(open(tabs)))
            }
            _ => (body, None),
        }
    }

    /// Minimal tabs in a column are a rail of icons, as a browser's vertical tabs
    /// are when collapsed.
    pub(crate) fn rail(&self) -> bool {
        self.settings.minimal_tabs && self.settings.tab_side != TabSide::Top
    }

    /// The rail opens to names over the session while the mouse is on it, and
    /// while a drag or a rename works on the tabs.
    fn rail_open(&self) -> bool {
        self.rail()
            && (self.rail_hover
                || self.tab_drag.is_some()
                || (self.renaming.is_some() && self.view.is_some()))
    }

    /// The rail shut: icons only.
    fn rail_shut(&self) -> bool {
        self.rail() && !self.rail_open()
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
            .map(|tab| {
                tab_width(
                    &self.sessions[self.face(tab)],
                    tab.len(),
                    self.settings.minimal_tabs,
                )
            })
            .collect();
        let focus = self.attached_index().unwrap_or(0);
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
    /// session with a row between (minimal: one row each, back to back), then "+".
    /// The column's edge toward the session is left for underlined tabs' rule.
    fn column_layout(&self, strip: Rect) -> TabLayout {
        let minimal = self.settings.minimal_tabs;
        let (height, step) = if minimal { (1, 1) } else { (2, 3) };
        let x = match self.settings.tab_side {
            TabSide::Right => strip.x + 1,
            _ => strip.x,
        };
        let width = strip.width.saturating_sub(1);
        let row = |y: u16, height: u16| Rect::new(x, y, width, height).intersection(strip);
        let home = row(strip.y, height);
        let first = home.bottom() + 1;
        let n = self.tabs().len();
        let focus = self.attached_index().unwrap_or(0);
        // Each tab and the row after it; the marks and "+" a row each.
        let room = strip.bottom().saturating_sub(first);
        let fits = |start: usize, end: usize| {
            let marks = u16::from(start > 0) + u16::from(end < n);
            (end - start) as u16 * step + marks < room
        };
        let (start, end) = window(n, focus, fits);
        let mut y = first;
        let more_before = (start > 0).then(|| {
            y += 1;
            row(first, 1)
        });
        let mut tabs = Vec::new();
        for i in start..end {
            tabs.push((i, row(y, height)));
            y += step;
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
        if self.rail_open() {
            frame.render_widget(ratatui::widgets::Clear, area);
        }
        match self.settings.tabs {
            TabStyle::Cards => self.draw_cards(frame, area),
            TabStyle::Underline | TabStyle::Folder => self.draw_underlined(frame, area),
        }
    }

    /// Marks for tabs that did not fit: before the first shown and after the last.
    fn draw_more(&self, frame: &mut ratatui::Frame, layout: &TabLayout) {
        let (before, after) = match self.settings.tab_side {
            TabSide::Top => ("‹", "›"),
            _ if self.rail_shut() => ("  ▲", "  ▼"),
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
        // Minimal tabs in a column sit back to back: the row under is the next one.
        if self.settings.tab_side == TabSide::Top
            || self.settings.minimal_tabs
            || y >= area.bottom()
        {
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
        let minimal = self.settings.minimal_tabs;
        let shut = self.rail_shut();
        // Every clickable thing is a card on the darker strip; the cursor's is lit.
        let lit = style::Style::new().bg(t.accent).fg(t.bg);
        let attached = self.attached_index();
        let tabs = self.tabs();
        for &(i, rect) in &layout.tabs {
            let tab = &tabs[i];
            let s = &self.sessions[self.face(tab)];
            let here = Some(i) == attached;
            let cursor = self.tab_mode && here;
            let (state, label) = match s.exited {
                None => (s.status.state, state_word(s.status.state)),
                Some(_) => (AgentState::Exited, "exited"),
            };
            let color = t.state(state);
            let queued = self.queue.iter().any(|q| q.session == s.id);
            // A split tab says how many panes it has, and `⊡` once zoomed here.
            let panes = if tab.len() > 1 {
                let mark = if here && self.is_zoomed() {
                    '⊡'
                } else {
                    '⊞'
                };
                format!("{mark}{} ", tab.len())
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
                Some((id, typed)) if *id == s.id => Line::from(
                    [mark.fg(t.accent).bold(), "✎ ".fg(t.accent)]
                        .into_iter()
                        .chain(typed.spans(width.saturating_sub(3), t))
                        .collect::<Vec<_>>(),
                ),
                _ => {
                    let name_color = if queued { color } else { t.fg };
                    let icon = if minimal {
                        format!("{} ", state_icon(state, spin))
                    } else {
                        String::new()
                    };
                    let panes = if minimal && !shut {
                        panes.trim_end()
                    } else {
                        ""
                    };
                    let room =
                        width.saturating_sub(1 + number.width() + icon.width() + panes.width());
                    let (name, tail) = name_and_report(s, room, minimal && !shut);
                    let mut name = name.fg(name_color);
                    if here || queued {
                        name = name.bold();
                    }
                    if s.auto_name {
                        name = name.italic();
                    }
                    Line::from(vec![
                        mark.fg(t.accent).bold(),
                        number.fg(t.muted),
                        icon.fg(color),
                        name,
                        tail.fg(t.secondary()),
                        format!(" {panes}").trim_end().to_string().fg(t.muted),
                    ])
                }
            };
            let mut bottom = vec![mark.fg(t.accent).bold()];
            bottom.extend(detail_spans(
                &format!("{panes}{}", s.report.clone().unwrap_or_else(|| program(s))),
                s.report.is_some(),
                label,
                state_icon(state, spin),
                color,
                width - 1,
                t,
            ));
            let lines = if minimal {
                vec![top]
            } else {
                vec![top, Line::from(bottom)]
            };
            frame.render_widget(
                Paragraph::new(lines).style(style::Style::new().bg(bg)),
                rect,
            );
            if cursor {
                frame.buffer_mut().set_style(rect, lit);
            }
            self.card_edge(frame, area, rect, if cursor { t.accent } else { bg });
        }
        // Home: the way back to the whole picture, with how many need you.
        let need_you = self.need_you();
        let mut needs = if need_you == 0 {
            Line::default()
        } else {
            Line::from(format!(" ● {need_you}").fg(t.needs).bold())
        };
        // On top the home card ends a column early, like the gap after a tab.
        let home = match self.settings.tab_side {
            TabSide::Top => Rect {
                width: layout.home.width.saturating_sub(1),
                ..layout.home
            },
            _ => layout.home,
        };
        let home_lines = if !minimal {
            vec![
                Line::from(vec![" ⌂ ".fg(t.accent).bold(), "home".fg(t.fg).bold()]),
                needs,
            ]
        } else if shut {
            vec![rail_home(need_you, t)]
        } else if need_you == 0 {
            vec![Line::from(vec![
                " ⌂ ".fg(t.accent).bold(),
                "home".fg(t.fg).bold(),
            ])]
        } else {
            needs.spans.insert(0, " ⌂".fg(t.accent).bold());
            vec![needs]
        };
        frame.render_widget(
            Paragraph::new(home_lines).style(style::Style::new().bg(t.card)),
            home,
        );
        self.card_edge(frame, area, home, t.card);
        self.draw_more(frame, &layout);
        let new = if shut { "" } else { "new" };
        let plus = Paragraph::new(Line::from(vec![" + ".fg(t.accent).bold(), new.fg(t.fg)]))
            .style(style::Style::new().bg(t.card));
        frame.render_widget(plus, layout.plus);
    }

    /// Flat tabs on the page over a thin rule; the attached tab is bright and
    /// underlined in the accent, the rest muted. The tab cursor lifts its tab and
    /// underlines it in the second accent. Folder tabs on top mark the attached tab
    /// with a bar over it instead, and break the rule under it.
    /// Behind a session: the theme's background when sessions take its colors,
    /// else the terminal's own.
    fn page_bg(&self) -> style::Color {
        if self.settings.themed_sessions {
            self.settings.theme.bg
        } else {
            style::Color::Reset
        }
    }

    fn draw_underlined(&self, frame: &mut ratatui::Frame, area: Rect) {
        let t = self.settings.theme;
        let spin = (now_ms() / SPIN_EVERY.as_millis() as u64) as usize;
        let layout = self.tab_layout(area);
        // The session's own background, so the tabs sit on the same page.
        let page = style::Style::new().bg(self.page_bg());
        // The rule runs along the strip's edge toward the session: under a strip on
        // top, beside a column. A tab is underlined where it meets the rule.
        let folder = self.settings.tabs == TabStyle::Folder;
        let minimal = self.settings.minimal_tabs;
        let shut = self.rail_shut();
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
            // A split tab says how many panes it has, and `⊡` once zoomed here.
            let panes = if tab.len() > 1 {
                let mark = if here && self.is_zoomed() {
                    '⊡'
                } else {
                    '⊞'
                };
                format!("{mark}{} ", tab.len())
            } else {
                String::new()
            };
            let (state, label) = match s.exited {
                None => (s.status.state, state_word(s.status.state)),
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
                Some((id, typed)) if *id == s.id => Line::from(
                    std::iter::once(" ✎ ".fg(t.accent))
                        .chain(typed.spans(width.saturating_sub(3), t))
                        .collect::<Vec<_>>(),
                ),
                _ => {
                    let icon = if minimal {
                        format!("{} ", state_icon(state, spin))
                    } else {
                        String::new()
                    };
                    let panes = if minimal && !shut {
                        panes.trim_end()
                    } else {
                        ""
                    };
                    let room =
                        width.saturating_sub(1 + number.width() + icon.width() + panes.width());
                    // Bright when attached, the state's color when it needs you.
                    let (name, tail) = name_and_report(s, room, minimal && !shut);
                    let name = match (here, queued) {
                        (_, true) => name.fg(color).bold(),
                        (true, false) => name.fg(t.fg).bold(),
                        (false, false) => name.fg(t.muted),
                    };
                    let name = if s.auto_name { name.italic() } else { name };
                    let number = if here {
                        number.fg(t.accent)
                    } else {
                        number.fg(t.border)
                    };
                    Line::from(vec![
                        " ".into(),
                        number,
                        icon.fg(color),
                        name,
                        tail.fg(t.secondary()),
                        format!(" {panes}").trim_end().to_string().fg(t.muted),
                    ])
                }
            };
            let mut bottom = vec![Span::raw(" ")];
            bottom.extend(detail_spans(
                &format!("{panes}{}", s.report.clone().unwrap_or_else(|| program(s))),
                s.report.is_some(),
                label,
                state_icon(state, spin),
                color,
                width - 1,
                t,
            ));
            let lines = if minimal {
                vec![top]
            } else {
                vec![top, Line::from(bottom)]
            };
            frame.render_widget(Paragraph::new(lines), rect.intersection(text));
            if self.tab_mode && here {
                lift(frame, rect);
                underline(frame, rect, t.accent);
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
            // Minimal tabs sit back to back: only home has a rule under it.
            let tabs_below = layout.tabs.iter().map(|&(_, r)| r).filter(|_| !minimal);
            let below = std::iter::once(layout.home).chain(tabs_below);
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
        let need_you = self.need_you();
        let mut needs = if need_you == 0 {
            Line::default()
        } else {
            Line::from(format!(" ● {need_you}").fg(t.needs).bold())
        };
        let home_lines = if !minimal {
            vec![
                Line::from(vec![" ⌂ ".fg(t.accent), "home".fg(t.muted)]),
                needs,
            ]
        } else if shut {
            vec![rail_home(need_you, t)]
        } else if need_you == 0 {
            vec![Line::from(vec![" ⌂ ".fg(t.accent), "home".fg(t.muted)])]
        } else {
            needs.spans.insert(0, " ⌂".fg(t.accent));
            vec![needs]
        };
        frame.render_widget(Paragraph::new(home_lines), layout.home.intersection(text));
        self.draw_more(frame, &layout);
        let new = if shut { "" } else { "new" };
        frame.render_widget(
            Paragraph::new(Line::from(vec![" + ".fg(t.accent), new.fg(t.muted)])),
            layout.plus.intersection(text),
        );
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
        let file = short_path(file);
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
            " . ".fg(t.accent).bold(),
            "keys ".fg(t.muted),
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

impl App {
    /// The keys page, in the middle of the screen: one row per action with its key,
    /// and the default beside a key that was changed.
    fn draw_keys_page(&self, frame: &mut ratatui::Frame, page: &KeysPage) {
        let t = self.settings.theme;
        let map = &self.settings.keys;
        let width = 52;
        let area = frame.area();
        // Rows for the actions: what fits, sliding to keep the cursor in view.
        let room = area.height.saturating_sub(7).max(1) as usize;
        let shown = Action::ALL.len().min(room);
        let start = page.cursor.saturating_sub(shown - 1);
        let height = shown as u16 + 6;
        let rect = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        )
        .intersection(area);
        let mut lines = vec![Line::default()];
        for (i, &action) in Action::ALL.iter().enumerate().skip(start).take(shown) {
            let here = i == page.cursor;
            let key = if here && page.capturing {
                "press a key…".fg(t.accent).bold()
            } else {
                match map.get(action) {
                    Some(k) if here => k.label().fg(t.accent).bold(),
                    Some(k) => k.label().fg(t.fg),
                    None => "—".fg(t.muted),
                }
            };
            let mut line = Line::from(vec![
                format!("  {:<20}", action.label()).fg(if here { t.fg } else { t.muted }),
                key,
            ]);
            if !map.is_default(action) {
                let default = Keymap::default()
                    .get(action)
                    .map_or("none".into(), Key::label);
                line.spans
                    .push(format!("  (default {default})").fg(t.muted));
            }
            let pad = (width as usize).saturating_sub(2 + line.width());
            line.spans.push(" ".repeat(pad).into());
            lines.push(if here {
                line.style(style::Style::new().bg(t.selection))
            } else {
                line
            });
        }
        lines.push(Line::default());
        lines.push(Line::from("  ^]  home, always".fg(t.muted)));
        let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
        let hints: Vec<_> = if page.capturing {
            hint(" esc ", "cancel ").to_vec()
        } else {
            [
                hint(" ↩ ", "change "),
                hint(" ⌫ ", "clear "),
                hint(" r ", "reset "),
                hint(" R ", "all "),
                hint(" esc ", "done "),
            ]
            .concat()
        };
        frame.render_widget(ratatui::widgets::Clear, rect);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style::Style::new().fg(t.accent))
            .title(" Keys ".fg(t.fg).bold())
            .title_bottom(Line::from(hints).centered())
            .style(style::Style::new().bg(t.panel).fg(t.fg));
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

/// The keys page: its cursor, whether it waits for a key, and whether `R` was
/// pressed once.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct KeysPage {
    cursor: usize,
    capturing: bool,
    reset_armed: bool,
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

/// Home on the shut rail: how many need you, else the house.
fn rail_home(needing: usize, t: &Theme) -> Line<'static> {
    if needing == 0 {
        Line::from(" ⌂".fg(t.accent))
    } else {
        Line::from(format!(" ●{needing}").fg(t.needs).bold())
    }
}

/// A tab wide enough for its name and its program line (minimal: its name, icon
/// and pane count), within bounds.
fn tab_width(s: &SessionInfo, panes: usize, minimal: bool) -> u16 {
    // Mark and number before the name; mark, " · ", icon and state after the program,
    // and `⊞2 ` before it on a split tab.
    let name = 3 + s.name.width();
    let split = if panes > 1 {
        2 + panes.to_string().len()
    } else {
        0
    };
    if minimal {
        // Mark, number, icon and a space, the name, then ` ⊞2` on a split tab.
        // and ` · tests 41/50` while it shows a status.
        let report = s.report.as_ref().map_or(0, |r| 3 + r.width());
        return ((5 + s.name.width() + report + split) as u16 + 1).clamp(MINIMAL_TAB_MIN, TAB_MAX);
    }
    let lead = s
        .report
        .as_ref()
        .map_or_else(|| program(s).width(), |r| r.width());
    let detail = 1 + split + lead + 5 + state_word(s.status.state).width();
    (name.max(detail) as u16 + 1).clamp(TAB_MIN, TAB_MAX)
}

/// A tab's second line, `claude · ⠋ working` in `width`: the program muted, the
/// icon and state in the state's color.
/// A one-line tab's name cut to `room`, and, when `inline`, the status it
/// published after it (` · tests 41/50`), the name keeping a few columns.
fn name_and_report(s: &SessionInfo, room: usize, inline: bool) -> (String, String) {
    let Some(report) = s.report.as_ref().filter(|_| inline) else {
        return (fit(&s.name, room), String::new());
    };
    let keep = s.name.width().min(6);
    let tail = fit(&format!(" · {report}"), room.saturating_sub(keep));
    (fit(&s.name, room.saturating_sub(tail.width())), tail)
}

/// A tab card's second line: what runs there, or the status it published
/// (`report`, in the second accent), then its state.
fn detail_spans(
    program: &str,
    report: bool,
    label: &str,
    icon: &str,
    color: style::Color,
    width: usize,
    t: &Theme,
) -> Vec<Span<'static>> {
    let detail = fit(&format!("{program} · {icon} {label}"), width);
    // Where the program ends, or where `fit` cut it short.
    let mut split = program.len().min(detail.len());
    while !detail.is_char_boundary(split) {
        split -= 1;
    }
    let lead = if report { t.secondary() } else { t.muted };
    vec![
        detail[..split].to_string().fg(lead),
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

/// A quiet frame around a home pane: rounded, in the border color, its title on
/// the top edge. Not a dialog: nothing behind it dims and the footer stays
/// (decision #86, amending rule 8).
fn frame_block(title: Line<'static>, t: &Theme) -> Block<'static> {
    // `╭─ Inbox ─…`: a step of rule before the title, off the corner.
    let mut spans = vec!["─".fg(t.border)];
    spans.extend(title.spans);
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style::Style::new().fg(t.border))
        .style(style::Style::new().bg(t.bg).fg(t.fg))
        .title(Line::from(spans))
        .padding(Padding::horizontal(1))
}

/// A section page's footer (spec: One key grammar): at most five hints for the
/// selection, primary first, then `around` faint on the right (`j/k move`,
/// `[ ] filter`) when there's room. `hints` is key and label spans in pairs.
pub(crate) fn page_footer(
    hints: Vec<Span<'static>>,
    around: &str,
    width: u16,
    t: &Theme,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = hints.into_iter().take(10).collect();
    let used: usize = spans.iter().map(Span::width).sum();
    let width = width as usize;
    if used > width {
        return Line::from(backlog::fit_hints(spans, width));
    }
    let around = format!("{around} ");
    let room = around.chars().count();
    if !around.trim().is_empty() && used + room + 2 <= width {
        spans.push(" ".repeat(width - used - room).into());
        spans.push(around.fg(t.muted));
    }
    Line::from(spans)
}

/// A section's page: the whole screen under the strip, down to the footer's
/// row (spec: sections are full screens; rule 8, no box for grouping).
pub(crate) fn section_rect(area: Rect) -> Rect {
    Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    )
}

/// A section page's frame: no border, its title on its first row and its keys
/// on its last, where the footer is (they're the page's footer).
pub(crate) fn section_block(t: &Theme) -> Block<'static> {
    // A quiet row over the keys, as home has: where a toast goes, never over
    // the page's own text.
    Block::new()
        .padding(Padding::new(1, 1, 0, 1))
        .style(style::Style::new().bg(t.bg).fg(t.fg))
}

/// The detail beside a list, by the width there is (spec: Layout by width):
/// none under 100 columns (the list alone; a detail is its own screen), a list
/// of 56 columns up to 139, 62 up to 199, and 72 from 200 with the detail at
/// most 110, left-aligned. A single rule between them. The list, and the
/// detail if any.
pub(crate) fn list_and_detail(
    frame: &mut ratatui::Frame,
    area: Rect,
    t: &Theme,
) -> (Rect, Option<Rect>) {
    let (list, split) = list_detail_areas(area);
    let Some((rule, detail)) = split else {
        return (list, None);
    };
    let bar = vec![Line::from("│".fg(t.border)); rule.height as usize];
    frame.render_widget(Paragraph::new(bar), rule);
    (list, Some(detail))
}

/// Where a section page's list rows landed on screen, for the mouse (backlog
/// #281). Each row is kept with its place against the cursor, so a click turns
/// into the j/k and ↩ every list already takes (rule 7), and a page's own
/// handling of a move (facts to load, an armed delete to drop) still runs.
/// Recorded at each draw; a page showing no list records none.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RowHits(std::cell::RefCell<Vec<(Rect, isize)>>);

impl RowHits {
    pub(crate) fn clear(&self) {
        self.0.borrow_mut().clear();
    }

    /// The rows drawn in `area`, scrolled by `skip` lines: each selectable
    /// row's line in the list, and its place against the cursor.
    pub(crate) fn record(&self, area: Rect, skip: usize, rows: &[(usize, isize)]) {
        let mut hits = self.0.borrow_mut();
        hits.clear();
        for &(line, from) in rows {
            let Some(y) = line.checked_sub(skip) else {
                continue;
            };
            if y < area.height as usize {
                hits.push((Rect::new(area.x, area.y + y as u16, area.width, 1), from));
            }
        }
    }

    /// The keys a press at `x`,`y` stands for: ↩ on the cursor's row, else the
    /// j or k that bring the cursor to it.
    pub(crate) fn click(&self, x: u16, y: u16) -> Option<Vec<u8>> {
        let hits = self.0.borrow();
        let &(_, from) = hits.iter().find(|(r, _)| r.contains(Position::new(x, y)))?;
        Some(match from {
            0 => b"\r".to_vec(),
            n if n > 0 => b"j".repeat(n.unsigned_abs()),
            n => b"k".repeat(n.unsigned_abs()),
        })
    }
}

/// `list_and_detail`'s areas without drawing: the list, and the column between
/// and the detail when there's room for one.
fn list_detail_areas(area: Rect) -> (Rect, Option<(Rect, Rect)>) {
    let list = match area.width {
        0..100 => return (area, None),
        100..140 => 56,
        140..200 => 62,
        _ => 72,
    };
    let [list, between, detail] = Layout::horizontal([
        Constraint::Length(list),
        Constraint::Length(1),
        Constraint::Max(110),
    ])
    .areas(area);
    (list, Some((between, detail)))
}

/// Dims what's under a dialog (rule 8: a box is a dialog, and what's behind it
/// steps back).
pub(crate) fn dim(frame: &mut ratatui::Frame, area: Rect, t: &Theme) {
    frame.buffer_mut().set_style(
        area,
        style::Style::new()
            .fg(t.muted)
            .add_modifier(Modifier::DIM)
            .remove_modifier(Modifier::BOLD),
    );
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

/// `#id  kind  title`, then who proposed it and in which project, muted.
/// Who proposed a decision, as the lists say it.
fn proposer(d: &Decision) -> &str {
    match d.provenance.by.as_str() {
        "human" => "you",
        "valkyrie" => "your correction",
        by => by,
    }
}

fn proposal_row(d: &Decision, t: &Theme) -> TableRow<'static> {
    let project = scope_name(&d.project, |p| {
        p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    });
    // An active decision here may be out of date, or have a scope proposed.
    let kind = if d.fresh.review.is_some() {
        "review?".fg(t.accent)
    } else if scoping(d).is_some() {
        "scope?".fg(t.accent)
    } else {
        d.kind.as_str().fg(t.muted)
    };
    TableRow::new([
        Cell::from(format!("#{}", d.id).fg(t.muted)),
        Cell::from(kind),
        Cell::from(d.title.clone().fg(t.fg).bold()),
        Cell::from(format!("{} · {project}", proposer(d)).fg(t.muted)),
    ])
}

fn proposal_table<'a>(rows: impl IntoIterator<Item = TableRow<'a>>, t: &Theme) -> Table<'a> {
    Table::new(
        rows,
        [
            Constraint::Length(5),
            Constraint::Length(10),
            Constraint::Fill(3),
            Constraint::Fill(1),
        ],
    )
    .column_spacing(1)
    .row_highlight_style(style::Style::new().bg(t.selection))
    .highlight_symbol(Line::from("▌".fg(t.accent)))
    .highlight_spacing(HighlightSpacing::Always)
}

/// The scope proposed for `d`, when that's what it waits on (ADR-0014 §2): an
/// active decision not also flagged as out of date, which comes first.
fn scoping(d: &Decision) -> Option<&[String]> {
    if d.status != valkyrie_proto::DecisionStatus::Active || d.fresh.review.is_some() {
        return None;
    }
    d.proposed_scope.as_deref()
}

/// The detail's word for what runs there: the agent, and the program when it
/// isn't the agent (an Inbox row shows neither).
fn where_from(s: &SessionInfo) -> String {
    let program = program(s);
    match s.status.agent.as_str() {
        agent if agent == program || !is_agent(s) => program,
        agent => format!("{agent} · {program}"),
    }
}

/// Whether a digit answers this queued session: it asks with a menu of choices
/// (a permission, a question, a prompt seen on screen). Not a bell from a plain
/// program, nor a prompt waiting for text: a digit there would be typed in.
fn answerable(q: &QueueItem) -> bool {
    use valkyrie_proto::AskKind;
    q.status.state == AgentState::NeedsInput
        && matches!(
            q.status.ask,
            Some(AskKind::Permission | AskKind::Question | AskKind::Screen)
        )
}

/// Working and still running: the Inbox's Running group.
fn is_running(s: &SessionInfo) -> bool {
    s.exited.is_none() && s.status.state == AgentState::Working
}

/// `text` padded with spaces to `cells` columns wide (wide characters count two).
fn pad(text: &str, cells: usize) -> String {
    format!("{text}{}", " ".repeat(cells.saturating_sub(text.width())))
}

/// A record's row in Needs you (a batched decisions or filed row): its square
/// marker, a count for a name, no repo, what's in it, and how long it's waited.
fn record_row(
    mark: Span<'static>,
    name: &str,
    text: Vec<Span<'static>>,
    age: String,
    width: usize,
    t: &Theme,
) -> Line<'static> {
    let mut line = inbox_row(
        (AgentState::Idle, 0),
        (name, false),
        Path::new(""),
        text,
        age,
        width,
        t,
    );
    line.spans[1] = Span::styled(format!("{} ", mark.content), mark.style);
    line
}

/// A repository's folder name: `api` for `~/repos/api`.
fn base_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Inbox name and repo columns, in cells.
const INBOX_NAME: usize = 14;
const INBOX_REPO: usize = 7;

/// An Inbox row: a gutter for the cursor, the marker, the name (bold, italic when
/// it's the agent's topic), the repo (faint), one line, and the age right-aligned.
/// The path, command and adapter are the detail's, not the row's.
fn inbox_row(
    (state, spin): (AgentState, usize),
    (name, auto): (&str, bool),
    cwd: &Path,
    text: Vec<Span<'static>>,
    age: String,
    width: usize,
    t: &Theme,
) -> Line<'static> {
    let name = pad(&fit(name, INBOX_NAME), INBOX_NAME).fg(t.fg).bold();
    let name = if auto { name.italic() } else { name };
    let repo = cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut spans = vec![
        " ".into(),
        format!("{} ", state_icon(state, spin)).fg(t.state(state)),
        name,
        " ".into(),
        pad(&fit(&repo, INBOX_REPO), INBOX_REPO).fg(t.muted),
        " ".into(),
    ];
    let used: usize = spans.iter().map(Span::width).sum();
    let age = format!(" {age:>4} ");
    let room = width.saturating_sub(used + age.width());
    let mut left = room;
    for span in text {
        if left == 0 {
            break;
        }
        let cut = fit(&span.content, left);
        left -= cut.width();
        spans.push(Span::styled(cut, span.style));
    }
    spans.push(" ".repeat(left).into());
    spans.push(age.fg(t.muted));
    Line::from(spans)
}

fn inbox_queue_row(q: &QueueItem, now: u64, spin: usize, width: usize, t: &Theme) -> Line<'static> {
    let s = &q.status;
    let mut text = vec![s.summary.clone().unwrap_or_default().fg(t.secondary())];
    // The supervisor's offer on a flag (ADR-0019 §2): `a` takes it.
    if let Some(step) = q.flag.as_ref().and_then(|f| f.step.as_ref()) {
        text.push("  → ".fg(t.muted));
        text.push(step.action.label().fg(t.accent).bold());
    }
    if let Some(report) = &q.report {
        text.push("  ▸ ".fg(t.muted));
        text.push(report.clone().fg(t.secondary()));
    }
    inbox_row(
        (s.state, spin),
        (&q.name, q.auto_name),
        &q.cwd,
        text,
        age(s.since_ms, now),
        width,
        t,
    )
}

fn inbox_session_row(
    s: &SessionInfo,
    now: u64,
    spin: usize,
    width: usize,
    t: &Theme,
) -> Line<'static> {
    let mut text = Vec::new();
    let state = match s.exited {
        None => s.status.state,
        Some(code) => {
            let word = code.map_or("exited".into(), |c| format!("exited {c}"));
            text.push(format!("{word} ").fg(t.muted));
            AgentState::Exited
        }
    };
    if s.clients > 0 {
        text.push("◉ ".fg(t.muted));
    }
    if let Some(summary) = &s.status.summary {
        if !text.is_empty() && s.exited.is_some() {
            text.push("  ".into());
        }
        text.push(summary.clone().fg(t.secondary()));
    }
    // What it published on its tab with `valk status`.
    if let Some(report) = &s.report {
        text.push("  ▸ ".fg(t.muted));
        text.push(report.clone().fg(t.secondary()));
    }
    if s.status.agent != "generic" && !s.status.hooked {
        text.push("  no hooks yet".fg(t.secondary()).italic());
    }
    // Another live agent changed the same files (ADR-0019 §1).
    if let Some(overlap) = &s.overlap {
        // A warning, not a state: the glyph says so, the stuck color would lie.
        text.push("  ⚠ ".fg(t.secondary()));
        text.push(overlap.clone().fg(t.secondary()));
    }
    inbox_row(
        (state, spin),
        (&s.name, s.auto_name),
        &s.cwd,
        text,
        age(s.status.since_ms, now),
        width,
        t,
    )
}

/// The home screen's rename prompt.
fn rename_line(id: SessionId, typed: &edit::LineEdit, t: &Theme) -> Line<'static> {
    let mut spans = vec![format!(" rename #{id} ").fg(t.accent).bold(), " ".into()];
    spans.extend(typed.spans(NAME_MAX + 1, t));
    for (key, what) in [("↩", "save"), ("esc", "cancel"), ("blank", "folder name")] {
        spans.push(" ".into());
        spans.push(format!(" {key} ").fg(t.accent).bold());
        spans.push(format!(" {what}").fg(t.dim()));
    }
    Line::from(spans).style(style::Style::new().bg(t.panel))
}

/// What the attached bar's key hints are for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarMode {
    Session,
    Tabs,
    Menu,
    Renaming,
    Reviewing,
}

/// What a click on the bar under a session does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarClick {
    Action(Action),
    /// The `keys` chip, while hints are off: the `?` sheet.
    Help,
}

/// The bar under a session, and the columns (from, to) its chips are clicked in.
struct Bar {
    line: Line<'static>,
    clicks: Vec<(u16, u16, BarClick)>,
}

/// The bar under an attached session. Without `hints` it shows no keys until
/// you're in tab mode, a menu or a rename.
/// The attached pane's place in a split tab, for the bar.
#[derive(Debug, Clone, Copy)]
struct BarPanes {
    at: usize,
    of: usize,
    zoomed: bool,
}

#[allow(clippy::too_many_arguments)]
fn attached_bar(
    view: &Attached,
    place: &str,
    panes: Option<BarPanes>,
    mode: BarMode,
    need_you: usize,
    hints: bool,
    cols: u16,
    t: &Theme,
    map: &Keymap,
) -> Bar {
    let width = |spans: &[Span]| spans.iter().map(|s| s.width() as u16).sum::<u16>();
    let mut clicks = Vec::new();
    // Left: the name, its state, and where it works (spec: Attached).
    let name = format!(" {} ", view.name).fg(t.fg).bold();
    let mut spans = vec![if view.auto_name { name.italic() } else { name }];
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
                format!(" {} {} ", state_icon(s.state, 0), state_word(s.state))
                    .fg(color)
                    .bold(),
            );
        }
        (None, None) => {}
    }
    // Where it works gives way first when the bar is short of room.
    let place_at = (!place.is_empty()).then(|| {
        spans.push(format!(" {place} ").fg(t.muted));
        spans.len() - 1
    });
    // Zoomed, the bar says so, and which pane it is; a click shows the split.
    if let Some(p) = panes.filter(|p| p.zoomed) {
        let from = width(&spans);
        spans.push(format!(" ⊡ zoomed {}/{} ", p.at, p.of).fg(t.accent).bold());
        clicks.push((from, width(&spans), BarClick::Action(Action::Zoom)));
        spans.push(" ".into());
    }
    if let Some(page) = &view.scroll {
        let up = page.history - page.from_top;
        spans.push(
            format!(" ↑ {up}/{} lines back ", page.history)
                .fg(t.accent)
                .bold(),
        );
        spans.push(" q live ".fg(t.muted));
    }
    if let Some((notice, until)) = &view.notice
        && Instant::now() < *until
    {
        spans.push(format!(" {notice} ").fg(t.done).bold());
    }
    // Right: the one count and how to answer it, then the way to everything
    // else and the way home, as plain hints: no chips (spec: Attached). A
    // prompt's keys (rename, the pane menu) take their place while it's open.
    let label = |action: Action| map.get(action).map(|k| k.label());
    let mut right: Vec<(Span, Option<BarClick>)> = Vec::new();
    let key = |k: String| format!("{k} ").fg(t.accent).bold();
    let what = |w: &str| format!("{w}   ").fg(t.dim());
    match mode {
        BarMode::Session | BarMode::Tabs => {
            if need_you > 0 {
                right.push((
                    format!("● {need_you} need you ").fg(t.needs).bold(),
                    Some(BarClick::Action(Action::Home)),
                ));
                if hints && let Some(k) = label(Action::Needing) {
                    right.push((key(k), Some(BarClick::Action(Action::Needing))));
                }
                right.push(("  ".into(), None));
            }
            if mode == BarMode::Tabs {
                right.push((key("esc".into()), None));
                right.push((what("back"), None));
            } else if hints {
                if let Some(k) = label(Action::Tabs) {
                    right.push((key(k), Some(BarClick::Action(Action::Tabs))));
                    right.push((what("actions"), Some(BarClick::Action(Action::Tabs))));
                }
                if let Some(k) = label(Action::Home) {
                    right.push((key(k), Some(BarClick::Action(Action::Home))));
                    right.push((what("home"), Some(BarClick::Action(Action::Home))));
                }
            } else if let Some(k) = label(Action::Tabs) {
                // Hints off (the default): only where every key is, tab mode's
                // key then `?` (a bare `?` is the session's). A click opens it.
                right.push((key(format!("{k} ?")), Some(BarClick::Help)));
                right.push((what("keys"), Some(BarClick::Help)));
            }
        }
        BarMode::Menu | BarMode::Renaming => {
            let keys: &[(&str, &str)] = if mode == BarMode::Menu {
                &[("↑/↓", "move"), ("↩", "pick"), ("esc", "close menu")]
            } else {
                &[("↩", "save"), ("esc", "cancel"), ("blank", "folder name")]
            };
            for (k, w) in keys {
                right.push((key(k.to_string()), None));
                right.push((what(w), None));
            }
        }
        // The list's own border has its keys.
        BarMode::Reviewing => {}
    }
    let right_width: u16 = right.iter().map(|(s, _)| s.width() as u16).sum();
    // Short of room, the place is cut so the right side (the count and the way
    // out) stays on the bar (backlog #284).
    let over = (width(&spans) + right_width).saturating_sub(cols);
    if over > 0
        && let Some(at) = place_at
    {
        let room = (spans[at].width() as u16).saturating_sub(over + 2) as usize;
        spans[at] = match room {
            0 => "".into(),
            room => format!(" {} ", fit(place, room)).fg(t.muted),
        };
    }
    let pad = cols.saturating_sub(width(&spans) + right_width);
    spans.push(" ".repeat(pad as usize).into());
    for (span, click) in right {
        let from = width(&spans);
        spans.push(span);
        if let Some(click) = click {
            clicks.push((from, width(&spans), click));
        }
    }
    Bar {
        line: Line::from(spans).style(style::Style::new().bg(t.panel)),
        clicks,
    }
}

/// `~/repos/x` for paths under `$HOME`.
/// The user scope's root (ADR-0016 §1): habits that hold in every project, not a
/// directory. Matches `valkyrie_context::project::USER`.
const USER_SCOPE: &str = "~user";

/// How a decision's scope is named in a review: "your habits" for the user's,
/// else `path` named by `project`.
fn scope_name(path: &std::path::Path, project: fn(&std::path::Path) -> String) -> String {
    match path.as_os_str() == USER_SCOPE {
        true => "your habits".into(),
        false => project(path),
    }
}

fn short_path(path: &std::path::Path) -> String {
    let home = valkyrie_proto::home_dir();
    if path.starts_with(&home) && home.as_os_str().len() > 1 {
        let rest = path.strip_prefix(&home).unwrap();
        if rest.as_os_str().is_empty() {
            return "~".into();
        }
        return format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display());
    }
    path.display().to_string()
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

/// What `sync_paste` writes for bracketed paste (`h` or `l`), if anything: it only
/// acts when a prompt opens or closes, and leaves paste on when the program wants it.
fn paste_change(want: bool, forced: bool, program: bool) -> Option<char> {
    (want != forced).then_some(if want || program { 'h' } else { 'l' })
}

/// `m` with bracketed paste on when a backlog prompt forces it.
fn forced_modes(mut m: Modes, forced: bool) -> Modes {
    m.bracketed_paste |= forced;
    m
}

/// Mirrors the program's input modes.
fn write_modes(m: Modes, shape: Option<CursorShape>) -> Result<()> {
    let flag = |on: bool| if on { 'h' } else { 'l' };
    let mut out = term::outer();
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

/// The mouse off, for leaving. Valkyrie takes it on the home screen
/// (`App::capture_home`) and, while attached, first (`App::update_capture`).
fn release_mouse() -> Result<()> {
    let mut out = term::outer();
    write!(out, "{}", mouse::CAPTURE_OFF)?;
    out.flush()?;
    Ok(())
}

/// Back to what a plain shell expects: no mirrored input modes, the user's cursor shape.
fn reset_terminal_modes() -> Result<()> {
    write_modes(Modes::default(), None)?;
    release_mouse()?;
    // The session's images must not stay over the home screen (or the shell).
    write!(term::outer(), "{CLEAR_IMAGES}")?;
    execute!(term::outer(), SetCursorStyle::DefaultUserShape)?;
    Ok(())
}

/// The first key in `bytes`: a CSI or SS3 sequence, a lone Esc, or one byte.
fn next_key(bytes: &[u8]) -> &[u8] {
    match bytes {
        [0x1b, b'[', rest @ ..] => {
            // A final byte ends it, an embedded Esc cuts it short (that Esc is the
            // next key), and nothing else does: a stray `ESC [` can't swallow what follows.
            let end = rest
                .iter()
                .position(|b| (0x40..=0x7e).contains(b) || *b == 0x1b)
                .map_or(bytes.len(), |i| if rest[i] == 0x1b { i + 2 } else { i + 3 });
            &bytes[..end]
        }
        [0x1b, b'O', _, ..] => &bytes[..3],
        [] => bytes,
        _ => &bytes[..1],
    }
}

/// Window size changes: SIGWINCH on unix. Windows sends none to a program reading
/// VT input, so there the size is looked at a few times a second.
struct Resizes {
    #[cfg(unix)]
    signal: tokio::signal::unix::Signal,
    #[cfg(windows)]
    every: tokio::time::Interval,
    #[cfg(windows)]
    last: Option<(u16, u16)>,
}

impl Resizes {
    fn new() -> Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                signal: signal(SignalKind::window_change())?,
            })
        }
        #[cfg(windows)]
        {
            let mut every = tokio::time::interval(Duration::from_millis(200));
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            Ok(Self {
                every,
                last: ratatui::crossterm::terminal::size().ok(),
            })
        }
    }

    async fn recv(&mut self) -> Option<()> {
        #[cfg(unix)]
        return self.signal.recv().await;
        #[cfg(windows)]
        loop {
            self.every.tick().await;
            let now = ratatui::crossterm::terminal::size().ok();
            if now != self.last {
                self.last = now;
                return Some(());
            }
        }
    }
}

/// Keys as the VT sequences a unix terminal sends (Windows Terminal and conhost
/// translate them), so input passes through to sessions the same way. Raw mode
/// is crossterm's; this adds VT input, and VT output for the escapes written
/// directly.
#[cfg(windows)]
fn vt_input() {
    use windows_sys::Win32::System::Console::{
        DISABLE_NEWLINE_AUTO_RETURN, ENABLE_VIRTUAL_TERMINAL_INPUT,
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetConsoleMode,
    };
    // SAFETY: console mode calls on our own standard handles.
    unsafe {
        for (handle, add) in [
            (STD_INPUT_HANDLE, ENABLE_VIRTUAL_TERMINAL_INPUT),
            (
                STD_OUTPUT_HANDLE,
                ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN,
            ),
        ] {
            let handle = GetStdHandle(handle);
            let mut mode = 0;
            if GetConsoleMode(handle, &mut mode) != 0 {
                SetConsoleMode(handle, mode | add);
            }
        }
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

/// Where the home screen's cursor is, by item rather than row (`App::anchor`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Anchor {
    /// The Decide or Filed row.
    Record(InboxRow),
    /// A session, in Needs you or below it.
    Session(Option<SessionId>),
}

/// A row of the Inbox (spec: Inbox).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InboxRow {
    /// "+ New session": the composer (backlog #276), always the top row.
    Compose,
    /// A disk running low on space (backlog #300), every such disk as one row.
    Disk,
    /// A session waiting on you, by its place in the queue.
    Queue(usize),
    /// Proposals and ready items, as one row.
    Decide,
    /// Items agents filed, not triaged, as one row.
    Filed,
    /// A session not waiting on you.
    Session(SessionId),
}

/// A row that waits on you: Needs you's rows, not the composer or a session
/// below them.
fn waits(row: &InboxRow) -> bool {
    !matches!(row, InboxRow::Session(_) | InboxRow::Compose)
}

/// The supervisor at a glance (`App::glance`).
#[derive(Debug, Clone, PartialEq)]
struct Glance {
    on: bool,
    watching: usize,
    last: Option<valkyrie_proto::SupervisorEntry>,
}

/// What a place on the home screen is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hit {
    /// A row of the selection (proposals, queue, sessions).
    Row(usize),
    /// A chip or panel that does what this key does.
    Key(HomeKey),
    /// The preview panel: the wheel scrolls a proposal's text there.
    Preview,
    /// A section's name on the strip.
    Section(Section),
}

/// The four sections of home (spec: Four sections), each a page; the Inbox is
/// home itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Inbox,
    Backlog,
    Memory,
    History,
}

impl Section {
    const ALL: [Section; 4] = [
        Section::Inbox,
        Section::Backlog,
        Section::Memory,
        Section::History,
    ];

    fn name(self) -> &'static str {
        match self {
            Section::Inbox => "Inbox",
            Section::Backlog => "Backlog",
            Section::Memory => "Memory",
            Section::History => "History",
        }
    }

    /// The key after `g` that goes there, as on the web.
    fn key(self) -> &'static [u8] {
        match self {
            Section::Inbox => b"i",
            Section::Backlog => b"b",
            Section::Memory => b"m",
            Section::History => b"h",
        }
    }
}

/// One of Memory's tabs: Decisions, Skills and Notes are the memory page's;
/// Docs and Wiki are pages of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryTab {
    Page(memory::Tab),
    Docs,
    Wiki,
}

/// Memory's tabs in the order `]` goes through them (spec: Memory).
const MEMORY_TABS: [MemoryTab; 5] = [
    MemoryTab::Page(memory::Tab::Decisions),
    MemoryTab::Page(memory::Tab::Skills),
    MemoryTab::Docs,
    MemoryTab::Wiki,
    MemoryTab::Page(memory::Tab::Notes),
];

/// A section page set aside while you're in a session you opened from it. Each
/// is boxed: the pages differ a lot in size, more so on some platforms.
enum Parked {
    Backlog(Box<backlog::Page>),
    Wiki(Box<wiki::Page>),
    Digest(Box<digest::Page>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HomeKey {
    Up,
    Down,
    Enter,
    /// `n` or `/`: the composer, typing (backlog #276).
    Compose,
    /// `#`: the composer, finding a backlog item to start.
    ComposeItem,
    Kill,
    /// Open the settings panel.
    Settings,
    Rename,
    /// A key that works the same attached (`keys::Action`).
    Direct(Action),
    /// Open the keys page.
    Keys,
    /// Accept the selected proposal.
    Accept,
    /// Reject the selected proposal.
    Reject,
    /// Scroll the shown proposal's text.
    PageUp,
    PageDown,
    /// A short code that pairs another device with the web app.
    AddDevice,
    /// Open the backlog page.
    Backlog,
    /// The backlog proposal's toast: opens the backlog on it.
    Toast,
    /// Open the away digest.
    Digest,
    /// Open the supervisor page.
    Supervisor,
    /// Open past sessions, to resume one.
    Past,
    /// Open the Docs page.
    Docs,
    /// Open the repository wiki.
    Wiki,
    /// Write the selected agent session down as a backlog item.
    Adopt,
    /// Take the other step offered on the selected flagged session.
    Other,
    /// `tab`: the next row that waits on you.
    NextWaiting,
    /// `1`-`9`: answer the selected ask with that choice.
    Answer(u8),
    /// `?`: every key, the tutor at its foot.
    Help,
    /// Esc: back from a session's detail screen.
    Back,
    Quit,
}

#[cfg(test)]
fn home_keys(bytes: &[u8], map: &Keymap) -> Vec<HomeKey> {
    home_keys_at(bytes, map)
        .into_iter()
        .map(|(k, _)| k)
        .collect()
}

/// The home screen's keys in `bytes`, each with where it ends.
fn home_keys_at(bytes: &[u8], map: &Keymap) -> Vec<(HomeKey, usize)> {
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
            let len = if key == b"\x1b[M" { 6 } else { key.len() };
            match key {
                b"\x1b[A" | b"\x1bOA" => keys.push((HomeKey::Up, i + len)),
                b"\x1b[B" | b"\x1bOB" => keys.push((HomeKey::Down, i + len)),
                b"\x1b[6~" => keys.push((HomeKey::PageDown, i + len)),
                b"\x1b[5~" => keys.push((HomeKey::PageUp, i + len)),
                _ => {}
            }
            i += len;
            continue;
        }
        if let Some((action, len)) = map.at(rest) {
            keys.push((HomeKey::Direct(action), i + len));
            i += len;
            continue;
        }
        let key = match rest[0] {
            b'k' => Some(HomeKey::Up),
            b'j' => Some(HomeKey::Down),
            b'\r' | b'\n' => Some(HomeKey::Enter),
            b'n' | b'/' => Some(HomeKey::Compose),
            b'#' => Some(HomeKey::ComposeItem),
            b'x' => Some(HomeKey::Kill),
            b',' => Some(HomeKey::Settings),
            b'r' => Some(HomeKey::Rename),
            b'.' => Some(HomeKey::Keys),
            b'a' => Some(HomeKey::Accept),
            b'd' => Some(HomeKey::Reject),
            b'p' => Some(HomeKey::AddDevice),
            b'b' => Some(HomeKey::Backlog),
            b'w' => Some(HomeKey::Digest),
            b's' => Some(HomeKey::Supervisor),
            b'h' => Some(HomeKey::Past),
            b'D' => Some(HomeKey::Docs),
            b'W' => Some(HomeKey::Wiki),
            b'?' => Some(HomeKey::Help),
            b'\t' => Some(HomeKey::NextWaiting),
            d @ b'1'..=b'9' => Some(HomeKey::Answer(d - b'0')),
            b'A' => Some(HomeKey::Adopt),
            b'o' => Some(HomeKey::Other),
            b'q' | 0x03 => Some(HomeKey::Quit),
            // A lone Esc: sequences and Alt keys were taken above.
            0x1b if rest.len() == 1 => Some(HomeKey::Back),
            _ => None,
        };
        if let Some(key) = key {
            keys.push((key, i + 1));
        }
        i += 1;
    }
    keys
}

/// A doc or a proposal open in your editor (`App::start_edit`).
#[derive(Debug)]
struct Editing {
    session: SessionId,
    what: Edited,
    path: PathBuf,
    /// The body as it went in, to tell whether you changed it.
    body: String,
    /// Its session has been seen running.
    seen: bool,
}

/// The docs page beside a session (backlog #311): the docs it wrote, in a column
/// of its own beside the panes. It has the keyboard while open; esc on its list
/// closes it.
struct Beside {
    session: SessionId,
    page: docs::Page,
}

/// Which docs page an action is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocsOn {
    Home,
    Beside,
}

/// Narrower than this, the docs beside a session take its whole area.
const BESIDE_MIN_COLS: u16 = 100;

/// What's open in your editor, and where what you write goes.
#[derive(Debug)]
enum Edited {
    /// A doc, saved over the version `base` (its hash) names.
    Doc { key: String, base: String },
    /// A proposal, accepted as you leave it unless it changed since `seen`.
    Proposal {
        project: PathBuf,
        id: u32,
        seen: u64,
        habit: bool,
        /// Its words before, which `z` puts back.
        was: memory::Wording,
    },
}

/// Your editor as a command, the file to come after it: `$VISUAL`, else
/// `$EDITOR`, else `vi`. Either may carry arguments or quotes (`code -w`), so on
/// Unix it runs through the shell, as git runs it.
fn editor_command() -> Vec<String> {
    let editor = ["VISUAL", "EDITOR"]
        .into_iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "vi".into());
    editor_command_for(&editor)
}

fn editor_command_for(editor: &str) -> Vec<String> {
    if cfg!(unix) {
        vec![
            "sh".into(),
            "-c".into(),
            format!("{editor} \"$@\""),
            "sh".into(),
        ]
    } else {
        editor.split_whitespace().map(String::from).collect()
    }
}

/// A new file holding `body`, readable only by you, for your editor.
fn edit_file(body: &str) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let path = std::env::temp_dir().join(format!("valk-doc-{}-{nanos}.md", std::process::id()));
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    open.open(&path)?.write_all(body.as_bytes())?;
    Ok(path)
}

/// Whether `dir` is in a git repository (as the CLI and daemon see it).
fn in_repository(dir: &Path) -> bool {
    valkyrie_context::project::in_git(dir)
}

/// Which repository adding `typed` gave (backlog #240): the daemon keeps a
/// worktree's main checkout, resolved, so it may not read as typed. The one added
/// by hand that wasn't before, else the one that reads as typed.
fn added_repo(
    before: &valkyrie_proto::RepoList,
    after: &valkyrie_proto::RepoList,
    typed: &str,
) -> Option<PathBuf> {
    let was = |p: &PathBuf| before.repos.iter().any(|r| r.added && r.path == *p);
    after
        .repos
        .iter()
        .find(|r| r.added && !was(&r.path))
        .or_else(|| {
            after
                .repos
                .iter()
                .find(|r| r.path == std::path::Path::new(typed) || r.label == typed)
        })
        .map(|r| r.path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::{Cursor, Span};

    /// `$VISUAL` may quote a path with spaces and carry arguments: it runs
    /// through the shell, the file after it.
    #[cfg(unix)]
    #[test]
    fn the_editor_runs_through_the_shell() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("valk tui editor {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("my ed");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s:%s' \"$1\" \"$(basename \"$2\")\" > \"$2\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = dir.join("doc file.md");
        std::fs::write(&file, "").unwrap();
        let mut cmd = editor_command_for(&format!("'{}' -w", script.display()));
        cmd.push(file.display().to_string());
        let ok = std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .status()
            .unwrap();
        assert!(ok.success());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "-w:doc file.md");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A stray empty `.git` above the cwd isn't a repository, so `n` mustn't file
    /// the item under it (backlog #13).
    #[test]
    fn a_stray_empty_git_dir_is_not_a_repository() {
        let base = std::env::temp_dir().join(format!("valk-tui-stray-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join(".git")).unwrap();
        std::fs::create_dir_all(base.join("y")).unwrap();
        let real = base.join("repo");
        std::fs::create_dir_all(real.join(".git")).unwrap();
        std::fs::write(real.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(real.join("src")).unwrap();
        let strays_above = base.ancestors().skip(1).any(in_repository);
        let (stray, filed) = (
            in_repository(&base.join("y")),
            in_repository(&real.join("src")),
        );
        std::fs::remove_dir_all(&base).unwrap();
        assert!(filed, "a real repository still counts");
        if !strays_above {
            assert!(!stray, "an empty .git is not a repository");
        }
    }

    /// The default map's keys go past the program; Esc before `[` or `O` is a
    /// sequence, and an Alt key no one here claims is the program's.
    #[test]
    fn finds_direct_keys() {
        let map = Keymap::default();
        let at = |b: &[u8]| map.at(b);
        assert_eq!(at(b"\x1b3"), Some((Action::Tab(2), 2)));
        assert_eq!(at(b"\x1bh"), Some((Action::Home, 2)));
        assert_eq!(at(b"\x1bt"), Some((Action::NewTab, 2)));
        assert_eq!(at(&[0x1e]), None, "Ctrl-^ is the program's again");
        assert_eq!(at(&[DETACH_KEY]), Some((Action::Home, 1)));
        assert_eq!(at(&[TABS_KEY]), Some((Action::Tabs, 1)));
        assert_eq!(at(b"\x1bk"), Some((Action::PrevTab, 2)));
        assert_eq!(at(b"\x1bp"), Some((Action::Last, 2)));
        assert_eq!(at(b"\x1b[A"), None);
        assert_eq!(at(b"\x1bOA"), None);
        assert_eq!(at(b"\x1bl"), None, "fish's ls");
        assert_eq!(at(b"\x1b"), None, "a lone Esc");
        assert_eq!(at(b"n"), None);
        assert_eq!(
            find_direct(b"ls\x1b[D\x1bnx", &map),
            Some((5, Action::Needing, 2))
        );
        assert_eq!(find_direct(b"\x1b[1;3A plain", &map), None);
        // Home takes them too.
        assert_eq!(
            home_keys(b"\x1b2j\x1bn", &map),
            vec![
                HomeKey::Direct(Action::Tab(1)),
                HomeKey::Down,
                HomeKey::Direct(Action::Needing)
            ]
        );
    }

    fn home_keys_default(bytes: &[u8]) -> Vec<HomeKey> {
        home_keys(bytes, &Keymap::default())
    }

    #[test]
    fn parses_home_keys() {
        assert_eq!(
            home_keys_default(b"j\x1b[Ak\x1bOB\r.q"),
            vec![
                HomeKey::Down,
                HomeKey::Up,
                HomeKey::Up,
                HomeKey::Down,
                HomeKey::Enter,
                HomeKey::Keys,
                HomeKey::Quit
            ]
        );
        // Mouse reports arriving after a detach are not keys (an X10 report's raw
        // bytes could be `q` or `x`).
        assert_eq!(home_keys_default(b"\x1b[<0;5;3m\x1b[Mqxj"), []);
        assert_eq!(home_keys_default(b"\x1b[<0;5;3mj"), [HomeKey::Down]);
        // Nor is the terminal's answer to an image command.
        assert_eq!(
            home_keys_default(b"\x1b_Gi=1;EINVAL:q\x1b\\j"),
            [HomeKey::Down]
        );
        assert_eq!(home_keys_default(b"ad"), [HomeKey::Accept, HomeKey::Reject]);
        assert_eq!(home_keys_default(b"p"), [HomeKey::AddDevice]);
        // Shifted: `a` accepts a proposal, `A` adopts a session.
        assert_eq!(home_keys_default(b"aA"), [HomeKey::Accept, HomeKey::Adopt]);
        // `?` is every key; tab the next waiting row; a digit answers an ask.
        assert_eq!(home_keys_default(b"?"), [HomeKey::Help]);
        assert_eq!(
            home_keys_default(b"\t3"),
            [HomeKey::NextWaiting, HomeKey::Answer(3)]
        );
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
            auto_name: false,
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
            overlap: None,
            report: None,
            supervisor: false,
        }
    }

    fn queued(id: SessionId) -> QueueItem {
        QueueItem {
            session: id,
            name: String::new(),
            auto_name: false,
            cwd: "/".into(),
            status: AgentStatus::default(),
            flag: None,
            report: None,
        }
    }

    #[test]
    fn the_users_habits_are_named_as_such_in_a_review() {
        assert_eq!(
            scope_name(std::path::Path::new("~user"), short_path),
            "your habits"
        );
        assert_eq!(
            scope_name(std::path::Path::new("/r/app"), short_path),
            "/r/app"
        );
    }

    fn proposed(project: &str, id: u32, title: &str) -> Decision {
        Decision {
            id,
            project: project.into(),
            title: title.into(),
            body: String::new(),
            kind: valkyrie_proto::DecisionKind::Decision,
            status: valkyrie_proto::DecisionStatus::Proposed,
            created: 0,
            updated: 0,
            supersedes: None,
            superseded_by: None,
            pinned: false,
            scope: Vec::new(),
            proposed_scope: None,
            scope_asked: false,
            provenance: valkyrie_proto::Provenance {
                by: "claude".into(),
                ..Default::default()
            },
            fresh: Default::default(),
        }
    }

    /// Memory's `z` takes back the last answer given there: an accept goes back
    /// to review, then there's nothing left to undo. One rewritten on the way in
    /// (`e`) goes back in its own words, and a scope you left alone is never sent.
    #[tokio::test]
    async fn memory_undoes_its_last_answer_and_an_edit_too() {
        use std::sync::{Arc, Mutex};
        use valkyrie_proto::codec::{read_frame, write_frame};
        use valkyrie_proto::{ClientMsg, DecisionStatus, Reply};
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-undo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        #[cfg_attr(unix, allow(unused_mut))]
        let mut listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        // A daemon with one decision, reviewed as the real one would, refusing
        // what was read before it changed.
        let mut first = proposed("/r/app", 4, "Keep it short");
        first.body = "Why.".into();
        first.scope = vec!["src/**".into()];
        first.updated = 1;
        let held = Arc::new(Mutex::new(first.clone()));
        let sent: Arc<Mutex<Vec<ReviewAction>>> = Arc::default();
        let (d, kept) = (held.clone(), sent.clone());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut rd, mut wr) = tokio::io::split(stream);
            while let Ok(Some(msg)) = read_frame::<_, ClientMsg>(&mut rd).await {
                let answer = match msg {
                    ClientMsg::Review {
                        req, action, seen, ..
                    } => {
                        kept.lock().unwrap().push(action.clone());
                        let mut d = d.lock().unwrap();
                        if seen != Some(d.updated) {
                            ServerMsg::Err {
                                req,
                                message: "changed since you read it".into(),
                            }
                        } else {
                            match action {
                                ReviewAction::Accept => d.status = DecisionStatus::Active,
                                ReviewAction::Reopen => d.status = DecisionStatus::Proposed,
                                ReviewAction::Revise {
                                    title, body, kind, ..
                                } => {
                                    (d.title, d.body, d.kind) = (title, body, kind);
                                    d.status = DecisionStatus::Active;
                                }
                                ReviewAction::Edit {
                                    title, body, kind, ..
                                } => (d.title, d.body, d.kind) = (title, body, kind),
                                _ => {}
                            }
                            d.updated += 1;
                            let reply = Reply::Decision {
                                decision: Box::new(d.clone()),
                            };
                            ServerMsg::Ok { req, reply }
                        }
                    }
                    ClientMsg::Decisions { req, .. } => ServerMsg::Ok {
                        req,
                        reply: Reply::Decisions {
                            decisions: vec![d.lock().unwrap().clone()],
                        },
                    },
                    _ => continue,
                };
                if write_frame(&mut wr, &answer).await.is_err() {
                    break;
                }
            }
        });
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        app.memory = Some(memory::Page::new(memory::Tab::Decisions));
        app.proposals = vec![first.clone()];

        app.memory_keys(b"a").await;
        assert_eq!(app.status, "accepted #4 Keep it short");
        app.proposals.clear();
        app.memory_keys(b"z").await;
        assert_eq!(app.status, "#4 Keep it short is up for review again");
        app.memory_keys(b"z").await;
        assert_eq!(app.status, "nothing to undo");
        assert_eq!(
            *sent.lock().unwrap(),
            [ReviewAction::Accept, ReviewAction::Reopen]
        );

        // Rewritten in your editor, scope untouched: accepted as written.
        let now = held.lock().unwrap().clone();
        app.proposals = vec![now.clone()];
        let was = memory::Wording::of(&now);
        let path = dir.join("edit.md");
        std::fs::write(&path, "x").unwrap();
        let text = was.text().replace("# Keep it short", "# Keep it shorter");
        let what = Edited::Proposal {
            project: now.project.clone(),
            id: 4,
            seen: now.updated,
            habit: false,
            was,
        };
        app.accept_edited(what, text, path.clone()).await;
        assert_eq!(app.status, "accepted #4 Keep it shorter, as you wrote it");
        assert!(!path.exists(), "accepted, your file goes");
        app.memory_keys(b"z").await;
        assert_eq!(
            app.status,
            "#4 Keep it short is up for review again, as it was proposed"
        );
        let sent = sent.lock().unwrap();
        let kind = valkyrie_proto::DecisionKind::Decision;
        assert_eq!(
            sent[2..],
            [
                ReviewAction::Revise {
                    title: "Keep it shorter".into(),
                    body: "Why.".into(),
                    kind,
                    scope: None,
                },
                ReviewAction::Reopen,
                ReviewAction::Edit {
                    title: "Keep it short".into(),
                    body: "Why.".into(),
                    kind,
                    scope: None,
                },
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A page's footer: five hints at most, primary first, then what gets you
    /// around, faint on the right, when there's room for it (spec: One key
    /// grammar).
    #[test]
    fn a_page_footer_keeps_five_hints_and_puts_moving_on_the_right() {
        let t = crate::theme::DRACULA;
        let hint = |k: &str| vec![format!(" {k} ").into(), format!("{k}-label ").into()];
        let hints: Vec<ratatui::text::Span<'static>> = ["a", "b", "c", "d", "e", "f", "g"]
            .into_iter()
            .flat_map(hint)
            .collect();
        let text = |line: Line| {
            line.spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        let wide = text(page_footer(hints.clone(), "j/k move", 120, &t));
        assert!(
            wide.contains(" e e-label ") && !wide.contains(" f "),
            "{wide}"
        );
        assert!(wide.trim_end().ends_with("j/k move"), "{wide}");
        assert_eq!(wide.chars().count(), 120, "{wide}");
        // Narrow: the hints first; what gets you around goes, then hints do.
        let narrow = text(page_footer(hints.clone(), "j/k move", 60, &t));
        assert!(
            !narrow.contains("j/k move") && narrow.contains(" a a-label "),
            "{narrow}"
        );
        let tiny = text(page_footer(hints, "j/k move", 24, &t));
        assert!(tiny.chars().count() <= 24, "{tiny}");
    }

    /// An App over a socket nobody answers: enough for state and drawing.
    async fn idle_app(dir: &std::path::Path) -> (App, valkyrie_proto::ipc::Listener) {
        let _ = std::fs::remove_dir_all(dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(dir).unwrap();
        let listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        // Not whatever this machine's settings.toml says.
        app.settings = Settings::default();
        (app, listener)
    }

    /// What a fake daemon was sent, in order.
    type Sent = std::sync::Arc<std::sync::Mutex<Vec<valkyrie_proto::ClientMsg>>>;

    /// An App over a fake daemon that answers every request, so a test can go
    /// through attach and kill: `List` gets `sessions` (less the ones killed
    /// since), everything else `Done`. The App starts with those sessions and a
    /// fixed 120x30 terminal. `idle_app`'s listener never answers, and a request
    /// to it waits forever.
    async fn answering_app(dir: &std::path::Path, sessions: Vec<SessionInfo>) -> (App, Sent) {
        use valkyrie_proto::codec::{read_frame, write_frame};
        use valkyrie_proto::{ClientMsg, Reply};
        let _ = std::fs::remove_dir_all(dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(dir).unwrap();
        // Windows' accept takes `&mut`.
        #[cfg_attr(unix, allow(unused_mut))]
        let mut listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let sent: Sent = Default::default();
        let (kept, mut listed) = (sent.clone(), sessions.clone());
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut rd, mut wr) = tokio::io::split(stream);
            while let Ok(Some(msg)) = read_frame::<_, ClientMsg>(&mut rd).await {
                kept.lock().unwrap().push(msg.clone());
                // Every request carries a `req`; input and resizes don't.
                let Some(req) = serde_json::to_value(&msg).unwrap()["req"].as_u64() else {
                    continue;
                };
                let reply = match msg {
                    ClientMsg::List { .. } => Reply::Sessions {
                        sessions: listed.clone(),
                        layouts: Vec::new(),
                    },
                    ClientMsg::Kill { session, .. } => {
                        listed.retain(|s| s.id != session);
                        Reply::Done
                    }
                    // Session 1 wrote doc 8; the rest wrote none.
                    ClientMsg::Docs {
                        session: Some(id), ..
                    } => Reply::Docs {
                        docs: (id == 1).then(|| fake_doc(false)).into_iter().collect(),
                        root: "/docs".into(),
                        unread: 0,
                        at: 1,
                    },
                    ClientMsg::OpenDoc { key, .. } if key == "8" => Reply::Doc {
                        doc: Box::new(fake_doc(true)),
                    },
                    _ => Reply::Done,
                };
                if write_frame(&mut wr, &ServerMsg::Ok { req, reply })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        app.fixed_size = Some(Size {
            cols: 120,
            rows: 29,
        });
        app.sessions = sessions;
        (app, sent)
    }

    /// Doc 8, as `answering_app` has it: with its text when read.
    fn fake_doc(text: bool) -> valkyrie_proto::Doc {
        valkyrie_proto::Doc {
            id: 8,
            kind: valkyrie_proto::DocKind::Plan,
            title: "Rollout plan".into(),
            path: "repos/app/2026-11-26-rollout-plan.md".into(),
            key: "8".into(),
            unread: false,
            by: "claude".into(),
            created: "2026-11-26T10:00:00Z".into(),
            session: Some("app".into()),
            commit: None,
            item: None,
            repo: None,
            edited: vec![],
            hash: text.then(|| "abc".into()),
            text: text
                .then(|| "---\nvalk-id: 8\n---\n# Rollout\n\nShip the canary first.\n".into()),
        }
    }

    /// The docs a session wrote open beside its panes (backlog #311): `D` in tab
    /// mode, or a click on a mention of one. They take the keyboard, read a doc
    /// with ↩, and esc goes back to the list, then closes them.
    #[tokio::test]
    async fn a_sessions_docs_open_beside_it() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-beside-{}", std::process::id()));
        let (mut app, sent) = answering_app(&dir, vec![info(1), info(2)]).await;
        let mut view = Attached::new(1, "app".into(), None);
        view.apply(update(true, vec![row(0, "doc 8 written: /docs/x.md")]));
        app.view = Some(view);
        let full = Rect::new(0, 0, 120, 29);
        let whole = app.split_body(full).0;

        app.on_keys(vec![TABS_KEY, b'D']).await;
        assert!(app.beside_shown() && !app.tab_mode);
        let (panes, _) = app.split_body(full);
        let docs = app.beside_area().unwrap();
        assert!(
            panes.width < whole.width && docs.x >= panes.right(),
            "{panes:?} {docs:?}"
        );
        assert!(
            sent.lock().unwrap().iter().any(|m| matches!(
                m,
                valkyrie_proto::ClientMsg::Docs {
                    session: Some(1),
                    ..
                }
            )),
            "asks for this session's docs only"
        );

        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let draw = |app: &App, terminal: &mut Terminal<TestBackend>| {
            terminal
                .draw(|frame| {
                    let b = app.beside.as_ref().unwrap();
                    app.draw_beside(frame, app.beside_area().unwrap(), true, &b.page);
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let screen = draw(&app, &mut terminal);
        assert!(screen.contains("1 doc from this session"), "{screen}");
        assert!(screen.contains("Rollout plan"), "{screen}");

        // Keys are the docs', not the program's: ↩ reads it.
        app.on_keys(b"\r".to_vec()).await;
        let screen = draw(&app, &mut terminal);
        assert!(screen.contains("Ship the canary first."), "{screen}");
        assert!(
            !sent
                .lock()
                .unwrap()
                .iter()
                .any(|m| matches!(m, valkyrie_proto::ClientMsg::Input { .. })),
            "nothing typed into the session"
        );
        // Esc back to the list, and again closes it; the panes get their room back.
        app.on_keys(b"\x1b".to_vec()).await;
        assert!(app.beside_shown());
        app.on_keys(b"\x1b".to_vec()).await;
        assert!(app.beside.is_none());
        assert_eq!(app.split_body(full).0, whole);

        // A mention of it in the session reads it there at once.
        app.open_beside(Some("8".into())).await;
        let screen = draw(&app, &mut terminal);
        assert!(screen.contains("Ship the canary first."), "{screen}");
        // Another session's tab doesn't show them.
        app.view = Some(Attached::new(2, "other".into(), None));
        assert!(!app.beside_shown());
        assert_eq!(app.split_body(full).0, whole);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An agent's new backlog proposal pops up as a toast that names how to answer
    /// it; `b` in tab mode opens the backlog on it. The setting turns it off.
    #[tokio::test]
    async fn a_new_backlog_proposal_pops_up() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-pop-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.settings.sound = false;
        app.settings.desktop = false;
        let item = |id: u32, status: &str, by: &str| -> Item {
            serde_json::from_value(serde_json::json!({
                "id": id, "number": id, "title": format!("item {id}"), "status": status, "by": by,
                "project": "/r/app", "created": 0, "updated": 0,
            }))
            .unwrap()
        };
        let mut items = vec![item(1, "open", "human"), item(2, "proposed", "claude")];
        let push = |items: &[Item]| ServerMsg::Items {
            items: items.to_vec(),
            launches: vec![],
        };
        app.on_push(push(&items)).unwrap();
        assert!(
            app.toast().is_none(),
            "what was there on connecting isn't news"
        );

        items.push(item(3, "proposed", "claude"));
        app.on_push(push(&items)).unwrap();
        let toast = app.toast().expect("the new proposal shows as a toast");
        assert_eq!(toast.proposal, Some(3));
        let line: String = app
            .toast_line(toast)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(line.contains("■ claude proposed #3 item 3"), "{line}");
        // The home screen's `a` is the decisions': there `b` opens it.
        assert!(line.contains("b open"), "{line}");

        app.view = Some(Attached::new(1, "proj1".into(), None));
        let toast = app.toast().unwrap();
        let hint = app.toast_hint(toast).unwrap();
        assert!(hint.ends_with("a accept #3 · x decline"), "{hint}");
        app.on_keys(vec![TABS_KEY, b'b']).await;
        let rows = app.backlog_rows();
        let cursor = app.backlog.as_ref().expect("the backlog opened").cursor;
        assert_eq!(rows[cursor].1.id, 3, "on the proposal");
        assert!(app.toast().is_none(), "opening it answers the toast");

        // Off: no toast.
        app.backlog = None;
        app.settings.proposals = false;
        items.push(item(4, "proposed", "codex"));
        app.on_push(push(&items)).unwrap();
        assert!(app.toast().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// One selection runs over the Inbox: the queue, then one row for every
    /// proposal, then the sessions not in the queue (each session once). It stays
    /// on its row as the rows above it come and go.
    #[tokio::test]
    async fn selection_runs_over_queue_decisions_and_sessions() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-prop-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.sessions = vec![info(1), info(2)];
        app.queue = vec![queued(2)];
        app.cursor_moved = true;
        app.selected = 2; // session 1, under the composer and the queue's row
        assert!(
            app.on_push(ServerMsg::Proposals {
                items: vec![proposed("/r/a", 1, "one"), proposed("/r/b", 1, "two")],
            })
            .unwrap()
        );
        // The decisions' row above moved the cursor down with its session.
        assert_eq!((app.selected, app.selection().0), (3, Some(1)));
        assert_eq!(app.need_you(), 3, "the queue's one and both proposals");

        let walk: Vec<_> = (0..app.rows())
            .map(|i| {
                app.selected = i;
                (
                    app.proposal().map(|d| d.title.clone()),
                    app.queue_index(),
                    app.selection().0,
                )
            })
            .collect();
        assert_eq!(
            walk,
            [
                // The composer, on top.
                (None, None, None),
                (None, Some(0), Some(2)),
                // a/d answer the oldest proposal, the one the detail shows.
                (Some("one".into()), None, None),
                (None, None, Some(1)),
            ]
        );

        // On the decisions' row: it stays there as they're answered...
        app.selected = 2;
        app.on_push(ServerMsg::Proposals {
            items: vec![proposed("/r/b", 1, "two")],
        })
        .unwrap();
        assert_eq!(app.proposal().unwrap().title, "two");
        // ...and once none are left, the cursor stays where it was: on the next row.
        app.on_push(ServerMsg::Proposals { items: vec![] }).unwrap();
        assert_eq!((app.selected, app.selection().0), (2, Some(1)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The Inbox is one list, Needs you then Running then Quiet, and draws each
    /// session once: a waiting one isn't listed again below. Rows carry no path,
    /// command or adapter column.
    #[tokio::test]
    async fn inbox_draws_each_session_once_in_three_groups() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-inbox-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        let session = |id, name: &str, state, cwd: &str| {
            let mut s = info(id);
            s.name = name.into();
            s.cwd = cwd.into();
            s.command = vec!["/usr/bin/fish".into()];
            s.status.state = state;
            s.status.summary = Some(format!("{name} says hi"));
            s
        };
        // Tab order puts the quiet shell first; Running still comes before Quiet.
        app.sessions = vec![
            session(3, "shell", AgentState::Idle, "/home/u/repos/web"),
            session(1, "builder", AgentState::Working, "/home/u/repos/api"),
            session(2, "asker", AgentState::NeedsInput, "/home/u/repos/api"),
        ];
        let mut q = queued(2);
        q.name = "asker".into();
        q.cwd = "/home/u/repos/api".into();
        q.status.state = AgentState::NeedsInput;
        q.status.summary = Some("asker says hi".into());
        app.queue = vec![q];

        let ids: Vec<_> = (0..app.rows())
            .map(|i| {
                app.selected = i;
                app.selection().0
            })
            .collect();
        assert_eq!(
            ids,
            [None, Some(2), Some(1), Some(3)],
            "the composer, then each session once"
        );

        for (width, height) in [(80, 24), (100, 30), (140, 40)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    let [body, bar] =
                        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                            .areas(frame.area());
                    app.draw_home(frame, body, bar);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let rows: Vec<String> = (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect();
            let text = rows.join("\n");
            let find = |what: &str| rows.iter().position(|r| r.contains(what));
            let (needs, running, quiet) = (
                find("Needs you 1").expect("Needs you"),
                find("Running 1").expect("Running"),
                find("Quiet 1").expect("Quiet"),
            );
            assert!(needs < running && running < quiet, "{text}");
            // The list (left of any preview) names each session once.
            let list: String = rows
                .iter()
                .map(|r| r.chars().take(70).collect::<String>())
                .collect();
            for name in ["asker says hi", "builder says hi", "shell says hi"] {
                assert_eq!(list.matches(name).count(), 1, "{name} at {width}: {text}");
            }
            assert!(
                (needs..running).any(|y| rows[y].contains("asker")),
                "{text}"
            );
            assert!(rows[quiet + 1].contains("shell") && rows[quiet + 1].contains("web"));
            assert!(
                !list.contains("fish") && !list.contains("~/repos"),
                "{text}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// One count of what waits on you: the queue, each proposal and each item
    /// agents filed (not one an agent filed and took, nor an open one). The header,
    /// the Inbox and the attached status line all say the same number.
    #[tokio::test]
    async fn one_count_is_shown_the_same_everywhere() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-count-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.sessions = vec![info(1), info(2)];
        app.queue = vec![queued(1)];
        app.on_push(ServerMsg::Proposals {
            items: vec![proposed("/r/a", 1, "one"), proposed("/r/b", 2, "two")],
        })
        .unwrap();
        let item = |id: u32, status: &str, project: &str, taken: &[u32]| -> Item {
            serde_json::from_value(serde_json::json!({
                "id": id, "number": id, "title": format!("item {id}"), "status": status,
                "by": "claude", "project": project, "created": id, "updated": 0,
                "filed_by": [0, 2], "taken": taken,
            }))
            .unwrap()
        };
        app.on_push(ServerMsg::Items {
            items: vec![
                item(1, "proposed", "/r/api", &[]),
                item(2, "proposed", "/r/web", &[]),
                // Its own work, under way: not for triage.
                item(3, "proposed", "/r/web", &[2]),
                item(4, "open", "/r/web", &[]),
            ],
            launches: vec![],
        })
        .unwrap();
        assert_eq!(app.need_you(), 5);
        assert_eq!(
            app.inbox_rows(),
            [
                InboxRow::Compose,
                InboxRow::Queue(0),
                InboxRow::Decide,
                InboxRow::Filed,
                InboxRow::Session(2)
            ]
        );

        let home = home_screen(&app, 100).join("\n");
        for want in [
            "Inbox 5",
            "Needs you 5",
            "2 decisions",
            "2 filed",
            "api 1 · web 1",
        ] {
            assert!(home.contains(want), "lacks {want:?}: {home}");
        }
        assert!(!home.contains("backlog 2 waiting"), "no box of its own");

        let view = Attached::new(2, "two".into(), None);
        let bar = attached_bar(
            &view,
            "",
            None,
            BarMode::Session,
            app.need_you(),
            true,
            100,
            app.settings.theme,
            &app.settings.keys,
        );
        let line: String = bar.line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(line.contains("● 5 need you"), "{line}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The strip names the four sections, the one on screen marked, and `g` then
    /// a letter goes to one from home or any section's list; `g g` is still the
    /// page's own top, and a page taking text keeps `g` as a letter.
    #[tokio::test]
    async fn g_goes_to_a_section_and_the_strip_says_which() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-sect-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.queue = vec![queued(1)];
        app.sessions = vec![info(1)];
        let strip = |app: &App, width: u16| home_screen(app, width)[0].clone();

        assert_eq!(app.section(), Section::Inbox);
        let wide = strip(&app, 120);
        assert!(wide.contains("ᛒ") && wide.contains("Inbox 1"), "{wide}");
        assert!(
            wide.contains("VALKYRIE   Inbox 1    Backlog    Memory    History"),
            "{wide}"
        );
        assert!(
            // The host may not fit (a CI runner's name is long); the rest does.
            wide.contains(" running · "),
            "{wide}"
        );
        // Narrow, the others are a letter; the one on screen keeps its name.
        let narrow = strip(&app, 80);
        assert!(narrow.contains("Inbox 1   B   M   H"), "{narrow}");
        assert!(
            !narrow.contains("VALKYRIE"),
            "no room for the name: {narrow}"
        );

        // `g b` from home: the backlog, which the strip now names in full.
        app.on_input(b"gb".to_vec()).await;
        assert_eq!(app.section(), Section::Backlog);
        assert!(strip(&app, 80).contains("I") && strip(&app, 80).contains("Backlog"));
        // `g g` is the list's top, not a section, with an item's facts loaded.
        app.backlog.as_mut().unwrap().cursor = 3;
        app.backlog.as_mut().unwrap().facts = Some((1, 0, None));
        app.on_input(b"gg".to_vec()).await;
        assert_eq!(
            (app.section(), app.backlog.as_ref().unwrap().cursor),
            (Section::Backlog, 0)
        );
        // Across reads: `g`, then `i`, goes home.
        app.on_input(b"g".to_vec()).await;
        assert!(app.g_pending);
        app.on_input(b"i".to_vec()).await;
        assert_eq!((app.section(), app.g_pending), (Section::Inbox, false));

        // A `g` left pending is dropped by a click, not kept for a later key.
        app.on_input(b"g".to_vec()).await;
        app.on_home_mouse(Mouse {
            kind: MouseKind::Press,
            code: 0,
            x: 5,
            y: 20,
            release: false,
        })
        .await;
        assert!(!app.g_pending && app.status.is_empty());

        // Typing in the backlog, `g` is a letter.
        app.on_input(b"gb".to_vec()).await;
        app.on_input(b"n".to_vec()).await;
        assert!(
            app.backlog.as_ref().unwrap().typing.is_some(),
            "n adds an item"
        );
        app.on_input(b"gi".to_vec()).await;
        assert_eq!(app.section(), Section::Backlog, "typed, not a section");

        // History and Memory are their pages' sections.
        app.backlog = None;
        app.digest = Some(digest::Page::default());
        assert_eq!(app.section(), Section::History);
        app.on_input(b"gi".to_vec()).await;
        assert!(app.digest.is_none());
        app.docs = Some(docs::Page::default());
        assert_eq!(app.section(), Section::Memory);
        let wide = strip(&app, 120);
        assert!(
            wide.contains("Decisions · Skills · Docs · Wiki · Notes  [ ]"),
            "{wide}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Attaching from a section's page sets it aside; coming home puts it back
    /// as it was, so home is the section and row you left.
    #[tokio::test]
    async fn home_is_the_section_and_row_you_attached_from() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-park-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        let mut page = backlog::Page::default();
        page.cursor = 2;
        app.backlog = Some(page);
        app.park();
        assert!(app.backlog.is_none(), "the session has the screen");
        app.unpark();
        assert_eq!(app.backlog.as_ref().map(|p| p.cursor), Some(2));
        assert!(app.parked.is_none());

        // Nothing open: parking keeps what's parked.
        app.park();
        app.backlog = None;
        app.park();
        assert!(app.parked.is_some());
        app.unpark();
        app.backlog = None;
        // A resume in flight is over once its session is up: no frozen page.
        let mut history = digest::Page::default();
        history.resuming = true;
        app.digest = Some(history);
        app.park();
        app.unpark();
        assert!(app.digest.as_ref().is_some_and(|p| !p.resuming));
        app.digest = None;

        // A section opened meanwhile isn't replaced.
        app.backlog = Some(backlog::Page::default());
        app.park();
        app.digest = Some(digest::Page::default());
        app.unpark();
        assert!(app.backlog.is_none() && app.digest.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Layout by width (spec): the list alone under 100 columns; a list of 56,
    /// 62 or 72 columns beside a detail of at most 110 above that. Narrow,
    /// Enter shows a session's detail as its own screen, Esc goes back.
    #[tokio::test]
    async fn the_layout_follows_the_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        for (width, list, detail) in [
            (80, 80, None),
            (120, 56, Some(63)),
            (160, 62, Some(97)),
            (240, 72, Some(110)),
        ] {
            let mut terminal = Terminal::new(TestBackend::new(width, 10)).unwrap();
            let mut got = None;
            terminal
                .draw(|f| got = Some(list_and_detail(f, f.area(), &theme::DRACULA)))
                .unwrap();
            let (l, d) = got.unwrap();
            assert_eq!((l.width, d.map(|d| d.width)), (list, detail), "{width}");
        }

        let dir = std::env::temp_dir().join(format!("valkyrie-tui-narrow-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.sessions = vec![info(1)];
        let rows = home_screen(&app, 80);
        assert_eq!(
            rows[1].matches('╭').count(),
            1,
            "no detail at 80: {rows:#?}"
        );
        app.selected = 1;
        app.on_input(b"\r".to_vec()).await;
        assert!(app.narrow_detail(), "Enter shows the detail, not attaches");
        let foot = home_screen(&app, 80).last().cloned().unwrap();
        assert!(
            foot.contains("↩ attach") && foot.contains("esc back"),
            "{foot}"
        );
        app.on_input(b"\x1b".to_vec()).await;
        assert!(!app.narrow_detail());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Sections are full screens (spec: rule 8): a section's page draws no box
    /// and runs edge to edge under the strip, its keys on the screen's last row,
    /// where the footer is.
    #[test]
    fn section_pages_fill_the_screen_with_their_keys_at_the_foot() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let t = &theme::DRACULA;
        type Draw<'a> = Box<dyn Fn(&mut ratatui::Frame) + 'a>;
        let items: Vec<Item> = (1..=3)
            .map(|id| {
                serde_json::from_value(serde_json::json!({
                    "id": id, "number": id, "title": format!("Item number {id}"), "status": "open",
                    "by": "human", "project": "/r/app", "created": 0, "updated": 0,
                }))
                .unwrap()
            })
            .collect();
        let rows = backlog::rows_of(&items, &items, &[], 0);
        let mut ready = backlog::Page::default();
        ready.filter = backlog::Filter::Ready;
        let screens: Vec<(&str, Draw)> = vec![
            (
                "backlog",
                Box::new(|f| backlog::Page::default().draw(f, &[], &[], &[], 0, 0, t)),
            ),
            (
                "backlog, ready",
                Box::new(|f| ready.draw(f, &rows, &items, &[], 0, 0, t)),
            ),
            ("docs", Box::new(|f| docs::Page::default().draw(f, t))),
            (
                "wiki",
                Box::new(|f| wiki::Page::new("/r/app".into()).draw(f, t)),
            ),
            (
                "supervisor",
                Box::new(|f| supervisor::Page::default().draw(f, &[], 0, t)),
            ),
            ("history", Box::new(|f| digest::Page::default().draw(f, t))),
            (
                "memory",
                Box::new(|f| memory::Page::new(memory::Tab::Decisions).draw(f, &[], t)),
            ),
            (
                "pull requests",
                Box::new(|f| prs::Page::default().draw(f, &Default::default(), 0, t)),
            ),
        ];
        // The spec's three sizes (T5b, backlog #273).
        for (name, draw) in &screens {
            for (w, h) in [(80u16, 24u16), (100, 30), (140, 40)] {
                let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
                terminal.draw(|f| draw(f)).unwrap();
                let buffer = terminal.backend().buffer();
                let rows: Vec<String> = (0..h)
                    .map(|y| (0..w).map(|x| buffer[(x, y)].symbol()).collect())
                    .collect();
                let text = rows.join("\n");
                // A single rule between list and detail is no box (spec:
                // Layout by width); corners are.
                assert!(
                    !text
                        .chars()
                        .any(|c| "╭╮╰╯".contains(c) && !rows[0].contains(c)),
                    "{name} draws a box:\n{text}"
                );
                assert!(
                    rows[h as usize - 1].contains("j/k move"),
                    "{name}'s keys at the foot, {w}x{h}:\n{text}"
                );
                if *name == "backlog, ready" {
                    assert!(rows[1].contains("Backlog Ready"), "{text}");
                }
            }
        }
    }

    /// Color is state, in every theme (spec: Marks and color): home frames its
    /// panes quietly but draws no dialog's box and dims nothing, no filled chip
    /// below the strip, and stuck-ish states share the stuck color.
    #[tokio::test]
    async fn home_frames_its_panes_but_never_a_dialog_in_any_theme() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-skin-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        let mut ask = info(1);
        ask.status.state = AgentState::NeedsInput;
        let mut working = info(2);
        working.status.state = AgentState::Working;
        app.sessions = vec![ask, working, info(3)];
        let mut q = queued(1);
        q.status.state = AgentState::NeedsInput;
        app.queue = vec![q];
        app.on_push(ServerMsg::Proposals {
            items: vec![proposed("/r/a", 1, "one")],
        })
        .unwrap();
        for theme in theme::THEMES {
            app.settings.theme = theme;
            assert_eq!(
                theme.state(AgentState::Stale),
                theme.blocked,
                "{}",
                theme.name
            );
            assert_eq!(theme.state(AgentState::Interrupted), theme.blocked);
            assert_eq!(theme.state(AgentState::Idle), theme.muted);
            for width in [100, 140] {
                let mut terminal = Terminal::new(TestBackend::new(width, 20)).unwrap();
                terminal
                    .draw(|frame| {
                        let [body, bar] =
                            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                                .areas(frame.area());
                        app.draw_home(frame, body, bar);
                    })
                    .unwrap();
                let buffer = terminal.backend().buffer();
                for y in 1..20 {
                    for x in 0..width {
                        let cell = &buffer[(x, y)];
                        assert!(
                            !"┌┐└┘".contains(cell.symbol()),
                            "{} {width}: a dialog's box at {x},{y}",
                            theme.name
                        );
                        assert_ne!(
                            cell.bg, theme.accent,
                            "{} {width}: a chip at {x},{y}",
                            theme.name
                        );
                    }
                }
                // From 100 the detail sits beside the list, each in a quiet
                // frame on the border color (decision #86), nothing dimmed.
                let corners: Vec<u16> = (0..width)
                    .filter(|&x| buffer[(x, 1)].symbol() == "╭")
                    .collect();
                assert_eq!(corners.len(), 2, "{} {width}", theme.name);
                for x in corners {
                    assert_eq!(buffer[(x, 1)].fg, theme.border, "{}", theme.name);
                }
                assert!(
                    (0..width).all(|x| !buffer[(x, 3)].modifier.contains(Modifier::DIM)),
                    "{} {width}: frames aren't a dialog",
                    theme.name
                );
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Backlog #286: the strip is a bar with the rune and the open section as
    /// chips, the Inbox frame says how many need you, and each group's head is
    /// its state's mark and color, then a hairline.
    #[tokio::test]
    async fn home_marks_its_groups_and_brands_its_strip() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-brand-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.settings = Settings::default();
        let t = app.settings.theme;
        let mut working = info(2);
        working.status.state = AgentState::Working;
        app.sessions = vec![info(1), working, info(3)];
        let mut q = queued(1);
        q.status.state = AgentState::NeedsInput;
        app.queue = vec![q];
        let mut terminal = Terminal::new(TestBackend::new(140, 24)).unwrap();
        terminal
            .draw(|frame| {
                let [body, bar] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                    .areas(frame.area());
                app.draw_home(frame, body, bar);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let row = |y: u16| -> String { (0..140).map(|x| buffer[(x, y)].symbol()).collect() };
        let at = |y: u16, s: &str| -> u16 {
            let r = row(y);
            r[..r.find(s).unwrap_or_else(|| panic!("{s:?} in {r:?}"))]
                .chars()
                .count() as u16
        };

        // The strip: a bar, the rune an accent chip, the open section a chip.
        assert!(row(0).starts_with(" ᛒ  VALKYRIE"), "{}", row(0));
        assert_eq!(buffer[(1, 0)].bg, t.accent, "the rune's chip");
        assert_eq!(
            buffer[(at(0, "Inbox"), 0)].bg,
            t.selection,
            "the open section"
        );
        assert_eq!(
            buffer[(at(0, "Backlog"), 0)].bg,
            t.panel,
            "the rest on the bar"
        );

        // The Inbox frame says how many need you.
        assert!(row(1).starts_with("╭─ Inbox · 1 needs you"), "{}", row(1));

        // Each group's head: its mark and name in its state's color, a hairline.
        let rows: Vec<String> = (0..24).map(row).collect();
        let head = |name: &str| rows.iter().position(|r| r.contains(name)).unwrap() as u16;
        for (name, mark, color) in [("Needs you", "●", t.needs), ("Running", "◐", t.working)] {
            let y = head(name);
            assert!(row(y).contains(&format!("{mark} {name}")), "{}", row(y));
            assert_eq!(buffer[(at(y, name), y)].fg, color, "{name}");
            assert!(row(y).contains("──────"), "a hairline: {}", row(y));
        }
        assert!(row(head("Quiet")).contains("○ Quiet"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Proposals are one Needs-you row, with their keys in the bar and the oldest
    /// one's body in the preview.
    #[tokio::test]
    async fn home_screen_batches_proposals_into_one_row() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-review-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.sessions = vec![info(1)];
        let mut d = proposed("/home/u/repos/api", 7, "Use sqlx, not diesel");
        d.kind = valkyrie_proto::DecisionKind::Constraint;
        d.body = "Async all the way down.\nDiesel blocks.".into();
        app.on_push(ServerMsg::Proposals {
            items: (0..9)
                .map(|i| proposed("/r/x", 10 + i, "more"))
                .chain([d])
                .collect(),
        })
        .unwrap();
        let render = |app: &App| {
            let mut terminal = Terminal::new(TestBackend::new(160, 24)).unwrap();
            terminal
                .draw(|frame| {
                    let [body, bar] =
                        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                            .areas(frame.area());
                    app.draw_home(frame, body, bar);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            buffer
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };

        // One row for them all, counted; the detail reads the oldest, which a/d
        // answer.
        app.selected = 1;
        let text = render(&app);
        for want in ["Inbox 10", "10 decisions", "#10", "claude · /r/x", "accept"] {
            assert!(text.contains(want), "lacks {want:?}");
        }
        assert!(!text.contains("decisions to review"), "no box of its own");
        assert!(!text.contains("sqlx"));

        // On a session, its own keys again.
        app.selected = 2;
        let text = render(&app);
        assert!(text.contains("open") && !text.contains("accept"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn cursor_follows_its_session_when_the_queue_changes() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-{}", std::process::id()));
        // A client is needed to build an App; this one is never used.
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        // Not whatever this machine's settings.toml says.
        app.settings = Settings::default();
        app.sessions = vec![info(1), info(2), info(3)];
        // Before the cursor is put anywhere, pushes leave it on the first row
        // that waits on you, else on the composer (backlog #276).
        app.selected = 2;
        app.on_push(ServerMsg::Queue {
            items: vec![queued(1)],
        })
        .unwrap();
        assert_eq!(app.selected, 1, "the TUI opens on what waits");
        app.on_push(ServerMsg::Queue { items: vec![] }).unwrap();
        assert_eq!(
            app.row(),
            Some(InboxRow::Compose),
            "inbox zero: the composer"
        );
        // Once it's been put somewhere, it follows its session.
        app.cursor_moved = true;
        app.selected = 3; // session 3
        app.on_push(ServerMsg::Queue {
            items: vec![queued(1)],
        })
        .unwrap();
        assert_eq!(app.selection().0, Some(3));
        // Session 1 moved up into the queue, so it's no longer listed below it.
        assert_eq!(app.selected, 3);
        app.selected = 1; // queue row for session 1
        app.on_push(ServerMsg::Queue {
            items: vec![queued(2), queued(1)],
        })
        .unwrap();
        assert_eq!(app.selection(), (Some(1), app.queue.get(1)));
        // A session below that starts asking moves up into the queue; the cursor
        // goes with it, and back down when it's answered.
        app.selected = 3; // session 3, under the queue's two rows
        assert_eq!(app.selection(), (Some(3), None));
        app.on_push(ServerMsg::Queue {
            items: vec![queued(3), queued(2), queued(1)],
        })
        .unwrap();
        assert_eq!((app.selected, app.selection().0), (1, Some(3)));
        app.on_push(ServerMsg::Queue {
            items: vec![queued(2), queued(1)],
        })
        .unwrap();
        assert_eq!((app.selected, app.selection()), (3, (Some(3), None)));
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
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
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
                    auto_name: false,
                    cwd: s.cwd.clone(),
                    status: s.status.clone(),
                    flag: None,
                    report: None,
                }
            })
            .collect();
        app.selected = 1;
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
                "Backlog",
                "Inbox ",
                "cargo test -p auth",
                "backfill",
                "web-ui",
                "Do you want to proceed?",
                "? keys",
            ] {
                // The logos of themes with rain glitch a letter now and then.
                if theme.rain.is_some() && want == "ᛒ" {
                    continue;
                }
                assert!(text.contains(want), "{} theme lacks {want:?}", theme.name);
            }
            // The screen is a recessed inset with its own label, not a rule.
            let row_of = |want: &str| {
                (0..buffer.area.height).find_map(|y| {
                    let row: String = (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect();
                    row.find(want).map(|i| (row[..i].chars().count() as u16, y))
                })
            };
            assert!(!text.contains("── screen"), "{}", theme.name);
            let (x, y) = row_of("screen · live · last 6 lines").expect(theme.name);
            assert!(
                row_of("↩ attach").is_some_and(|(_, ay)| ay == y),
                "{}",
                theme.name
            );
            let (px, py) = row_of("Do you want to proceed?").unwrap();
            assert!(py > y, "{}", theme.name);
            let well = theme.recessed(theme.bg);
            assert_ne!(well, theme.bg, "{}", theme.name);
            for (cx, cy) in [(x, y), (px, py), (x - 1, py)] {
                assert_eq!(buffer[(cx, cy)].bg, well, "{} at {cx},{cy}", theme.name);
            }
            snapshot(buffer, theme.name);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A flagged session's row says what the supervisor offers, and with it
    /// selected the footer names the keys that take it (ADR-0019 §2).
    #[tokio::test]
    async fn a_flagged_row_offers_its_step_on_a_key() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use valkyrie_proto::{Flag, Step, StepAction};
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-flag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        let mut s = info(4);
        s.name = "item-98".into();
        s.status = AgentStatus {
            agent: "claude".into(),
            state: AgentState::Blocked,
            summary: Some("CI red on PR #98: windows".into()),
            seq: 3,
            ..AgentStatus::default()
        };
        let mut row = queued(4);
        row.name = s.name.clone();
        row.status = s.status.clone();
        row.flag = Some(Flag {
            reason: "CI red on PR #98: windows".into(),
            pr: "https://github.com/o/r/pull/98".into(),
            seq: 3,
            step: Some(Step {
                action: StepAction::Rerun,
                why: "ci / windows matches #17".into(),
                alt: Some(StepAction::CiFix),
            }),
            working: false,
        });
        app.sessions = vec![s];
        app.queue = vec![row];
        app.selected = 1;
        let (session, seq, step) = app.offered().unwrap();
        assert_eq!((session, seq, step.action), (4, 3, StepAction::Rerun));
        let mut terminal = Terminal::new(TestBackend::new(160, 14)).unwrap();
        terminal
            .draw(|frame| {
                let [body, bar] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                    .areas(frame.area());
                app.draw_home(frame, body, bar);
            })
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        // The row's own "→ rerun" may be cut at the list's width; the keys say it.
        for want in [
            " a rerun failed jobs",
            " o ask its agent to fix CI",
            " d leave it",
            "ci / windows matches #17",
        ] {
            assert!(text.contains(want), "lacks {want:?}");
        }
        // Without a step yet, nothing to take.
        app.queue[0].flag.as_mut().unwrap().step = None;
        assert!(app.offered().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The README's demo: an agent stops on a permission prompt, rises to the top of
    /// the queue, is answered and leaves it, and another finishes. With
    /// `VALK_SNAPSHOT_DIR` set, each step is written as `demo-<n>.json` for
    /// docs/demo/frames.py.
    #[tokio::test]
    async fn demo_walks_a_prompt_through_the_queue() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-demo-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        let now = now_ms();
        let home = std::env::var("HOME").unwrap_or_default();
        let session = |id, name: &str, cwd: &str| {
            let mut s = info(id);
            s.name = name.into();
            s.command = vec!["claude".into()];
            s.cwd = format!("{home}/repos/{cwd}").into();
            s.status = AgentStatus {
                agent: "claude".into(),
                state: AgentState::Working,
                since_ms: now - 300_000,
                hooked: true,
                ..AgentStatus::default()
            };
            s
        };
        app.sessions = vec![
            session(1, "fix-login", "shop"),
            session(2, "checkout-ui", "shop"),
            session(3, "docs-pass", "shop"),
            session(4, "db-migrate", "api"),
        ];
        let set = |app: &mut App, id: SessionId, state, summary: Option<&str>, ago: u64| {
            let s = app.sessions.iter_mut().find(|s| s.id == id).unwrap();
            s.status.state = state;
            s.status.summary = summary.map(Into::into);
            s.status.since_ms = now - ago * 1000;
        };
        let queue = |app: &mut App, ids: &[SessionId]| {
            app.queue = ids
                .iter()
                .map(|&id| {
                    let s = app.sessions.iter().find(|s| s.id == id).unwrap();
                    QueueItem {
                        session: s.id,
                        name: s.name.clone(),
                        auto_name: false,
                        cwd: s.cwd.clone(),
                        status: s.status.clone(),
                        flag: None,
                        report: None,
                    }
                })
                .collect();
        };
        let render = |app: &App, name: &str| {
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
            snapshot(buffer, name);
            buffer
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        let working =
            "● Adding a test for the login redirect\n  ⎿  Wrote tests/login.test.js (+18)";
        let docs = "● Fixed 4 typos in README.md\n\n  Nothing else to change.";

        // 1. Everyone working; one finished earlier waits in the queue.
        set(
            &mut app,
            3,
            AgentState::ReviewReady,
            Some("Done: fixed 4 typos in README.md"),
            240,
        );
        queue(&mut app, &[3]);
        app.selected = 1;
        app.preview = Some((3, docs.into()));
        let text = render(&app, "demo-1");
        assert!(text.contains("docs-pass") && text.contains("fixed 4 typos"));
        assert!(text.contains("Inbox 1") && !text.contains("asks"));

        // 2. fix-login stops on a permission prompt and goes to the top.
        set(
            &mut app,
            1,
            AgentState::NeedsInput,
            Some("Permission: Bash npm test"),
            2,
        );
        queue(&mut app, &[1, 3]);
        app.selected = 1;
        app.preview = Some((
            1,
            format!(
                "{working}\n\n● Bash(npm test)\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. No"
            ),
        ));
        let text = render(&app, "demo-2");
        assert!(text.contains("Inbox 2") && text.contains("npm test"));
        assert!(text.contains("Do you want to proceed?"));

        // 3. Answered: back to work, out of the queue.
        set(&mut app, 1, AgentState::Working, None, 1);
        queue(&mut app, &[3]);
        app.selected = 0;
        app.preview = Some((3, docs.into()));
        let text = render(&app, "demo-3");
        assert!(text.contains("Inbox 1") && !text.contains("Permission"));

        // 4. checkout-ui finishes and joins the queue.
        set(
            &mut app,
            2,
            AgentState::ReviewReady,
            Some("Done: cart totals update live"),
            1,
        );
        queue(&mut app, &[2, 3]);
        app.selected = 0;
        app.preview = Some((
            2,
            "● Cart totals now update as items change\n  ⎿  2 files changed, 64 insertions, 9 deletions".into(),
        ));
        let text = render(&app, "demo-4");
        assert!(text.contains("cart totals update") && text.contains("Inbox 2"));
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
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
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
        view.title = Some("user@host:~/repos/proj2".into());
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
                        Paragraph::new(
                            attached_bar(
                                view,
                                "",
                                app.bar_panes(view.id),
                                BarMode::Session,
                                0,
                                app.settings.hints,
                                bar.width,
                                app.settings.theme,
                                &app.settings.keys,
                            )
                            .line,
                        ),
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
            app.tab_mode = true;
            draw(&app, &mut terminal);
            snapshot(
                terminal.backend().buffer(),
                &format!("tabs-cursor-{}", theme.name),
            );
            app.tab_mode = false;
        }
        app.settings.theme = theme::THEMES[0];
        let lines = draw(&app, &mut terminal);
        assert!(
            lines[0].contains("1 proj1") && lines[0].contains("2 proj2"),
            "{}",
            lines[0]
        );
        assert!(lines[1].contains("fish · ○ quiet"), "{}", lines[1]);
        assert!(lines[1].contains("claude · ") && lines[1].contains("working"));
        assert!(lines[1].contains("codex · ● asks"));
        assert!(lines[0].trim_end().ends_with("+ new"), "{}", lines[0]);
        assert!(
            lines[2].starts_with("inside session 2"),
            "the session sits below"
        );
        // The attached tab is marked; the window title is gone from the bar.
        let layout = app.tab_layout(tabs);
        let attached = layout.tabs[1].1;
        assert_eq!(lines[0].chars().nth(attached.x as usize), Some('▌'));
        assert!(!lines[23].contains("user@host"), "{}", lines[23]);

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

        // ^\ is tab mode; getting around is the direct keys', so h/l, ↩ and the
        // digits do nothing there, and Esc hands the keyboard back.
        app.on_keys(vec![TABS_KEY]).await;
        assert!(app.tab_mode);
        app.on_keys(b"l2\r".to_vec()).await;
        assert!(app.tab_mode);
        assert_eq!(app.view.as_ref().map(|v| v.id), Some(2));
        app.on_keys(b"\x1b".to_vec()).await;
        assert!(!app.tab_mode);

        // r renames the attached tab; typing edits, Esc cancels.
        app.on_keys(vec![TABS_KEY, b'r']).await;
        assert_eq!(renaming(&app), Some((2, "proj2".into())));
        app.on_keys(b"\x7f\x7f\x7f\x7f\x7fapi \xc3\xa9".to_vec())
            .await;
        assert_eq!(renaming(&app), Some((2, "api é".into())));
        let lines = draw(&app, &mut terminal);
        assert!(lines[0].contains("✎ api é▏"), "{}", lines[0]);
        // The cursor moves: ← twice, then typing goes in before the é.
        app.on_keys(b"\x1b[D\x1b[Dx\x1b[F!".to_vec()).await;
        assert_eq!(renaming(&app), Some((2, "apix é!".into())));
        app.on_keys(b"\x1b[H\x1b[3~\x1b[F".to_vec()).await;
        assert_eq!(renaming(&app), Some((2, "pix é!".into())));
        app.on_keys(b"\x15".to_vec()).await;
        assert_eq!(renaming(&app), Some((2, String::new())));
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
        assert_eq!(renaming(&app), Some((3, "proj3".into())));
        app.on_keys(b"\x1b".to_vec()).await;
        // Close on an agent's tab asks first; Esc keeps the session.
        app.tab_click(tabs, right).await;
        app.on_keys(b"[B".to_vec()).await;
        let menu = app.menu.expect("the menu, on Close");
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
        // A double click on a tab starts the prompt too; one too slow doesn't. The
        // attached tab, since switching would wait on a daemon this test lacks.
        let left = Mouse::at(MouseKind::Press, attached.x + 1, 0);
        app.tab_click(tabs, left).await;
        app.tab_press = app.tab_press.map(|(id, at)| (id, at - DOUBLE_CLICK));
        app.tab_click(tabs, left).await;
        assert_eq!(app.renaming, None);
        app.tab_click(tabs, left).await;
        assert_eq!(renaming(&app), Some((2, "proj2".into())));
        app.on_keys(b"\x1b".to_vec()).await;
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Option-h from a Mac terminal that doesn't send Option as Meta says how to fix
    /// it; once an Alt key has come through, it no longer does.
    #[tokio::test]
    async fn option_typed_as_a_character_says_how_to_fix_it() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-option-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        app.sessions = (1..=2).map(info).collect();
        app.view = Some(Attached::new(2, "two".into(), None));
        let notice = |app: &App| app.view.as_ref().unwrap().notice.clone().map(|n| n.0);

        // `ø` is a letter, not a key: no hint.
        app.on_keys("ø".as_bytes().to_vec()).await;
        assert_eq!(notice(&app), None);
        app.on_keys("˙".as_bytes().to_vec()).await;
        assert!(notice(&app).unwrap().starts_with("⌥ isn't alt"));

        app.view.as_mut().unwrap().notice = None;
        app.on_keys(b"\x1b9".to_vec()).await;
        app.view.as_mut().unwrap().notice = None;
        app.on_keys("˙".as_bytes().to_vec()).await;
        assert_eq!(notice(&app), None, "Alt works here");

        // On the home screen it goes in the status line.
        app.view = None;
        app.alt_works = false;
        app.on_input("∆".as_bytes().to_vec()).await;
        assert!(app.status.starts_with("⌥ isn't alt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Direct keys that need no switch: Alt-w opens the menu on Close, a missing
    /// tab or no last tab says so, and the bytes before a direct key reach nobody
    /// else's tab.
    #[tokio::test]
    async fn direct_keys_without_a_switch() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-direct-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        app.sessions = (1..=2).map(info).collect();
        app.view = Some(Attached::new(2, "two".into(), None));

        app.on_keys(b"\x1bw".to_vec()).await;
        let menu = app.menu.expect("Alt-w opens the menu");
        assert_eq!((menu.session, menu.cursor), (2, MENU_CLOSE));
        app.on_keys(b"\x1b".to_vec()).await;
        assert_eq!(app.menu, None);

        let notice = |app: &App| app.view.as_ref().unwrap().notice.clone().map(|n| n.0);
        app.on_keys(b"\x1b9".to_vec()).await;
        assert_eq!(notice(&app).as_deref(), Some("no tab 9"));
        app.on_keys(b"\x1bp".to_vec()).await;
        assert_eq!(notice(&app).as_deref(), Some("no tab to go back to"));
        // Nothing needs you and the run started here: stay.
        app.on_keys(b"\x1bn".to_vec()).await;
        assert_eq!(notice(&app).as_deref(), Some("nothing else needs you"));
        assert_eq!(app.view.as_ref().map(|v| v.id), Some(2));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Alt-a opens the backlog page typing a new bug, from a session, tab mode or
    /// home; ↩ adds it and Esc or ^C cancels, and either goes back where it began.
    /// Nothing after the ↩ in the same read runs as a page command, a pasted line
    /// break is a space, and an item the daemon refused stays in the prompt.
    #[tokio::test]
    async fn capture_key_adds_an_item_and_goes_back() {
        use std::sync::{Arc, Mutex};
        use valkyrie_proto::codec::{read_frame, write_frame};
        use valkyrie_proto::{ClientMsg, ItemKind, NewItem, Reply};
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-capture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        // Windows' accept takes `&mut`.
        #[cfg_attr(unix, allow(unused_mut))]
        let mut listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        // A daemon that adds items, refuses one titled "fail" and every change, and
        // keeps what it was sent.
        let seen: Arc<Mutex<Vec<ClientMsg>>> = Arc::default();
        let kept = seen.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut rd, mut wr) = tokio::io::split(stream);
            while let Ok(Some(msg)) = read_frame::<_, ClientMsg>(&mut rd).await {
                kept.lock().unwrap().push(msg.clone());
                let refused = |req, message: &str| ServerMsg::Err {
                    req,
                    message: message.into(),
                };
                let answer = match msg {
                    ClientMsg::AddItem { req, item: new, .. } if new.body == "fail" => {
                        refused(req, "the daemon said no")
                    }
                    ClientMsg::AddItem { req, item: new, .. } => {
                        let id = kept
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|m| matches!(m, ClientMsg::AddItem { item, .. } if item.body != "fail"))
                            .count() as u32;
                        let item = Item {
                            parent: None,
                            pr_merged: false,
                            priority: Default::default(),
                            blocked_by: Vec::new(),
                            id,
                            number: id,
                            title: new.body.clone(),
                            body: String::new(),
                            kind: new.kind,
                            status: valkyrie_proto::ItemStatus::Open,
                            project: None,
                            attachments: Vec::new(),
                            by: "human".into(),
                            sessions: Vec::new(),
                            boot: 0,
                            taken: Vec::new(),
                            branch: None,
                            branch_taken: false,
                            checkout: None,
                            pr: None,
                            filed_by: None,
                            unaccepted: false,
                            pr_closed: false,
                            joined: Default::default(),
                            notes: Vec::new(),
                            launch: None,
                            created: 0,
                            updated: 0,
                        };
                        let reply = Reply::Item {
                            item: Box::new(item),
                            note: None,
                        };
                        ServerMsg::Ok { req, reply }
                    }
                    ClientMsg::ChangeItem { req, .. } => refused(req, "no changes here"),
                    _ => continue,
                };
                if write_frame(&mut wr, &answer).await.is_err() {
                    break;
                }
            }
        });
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        app.sessions = (1..=2).map(info).collect();
        app.view = Some(Attached::new(2, "two".into(), None));
        // An open item, selected when the page opens.
        app.on_push(ServerMsg::Items {
            launches: Vec::new(),
            items: vec![Item {
                parent: None,
                pr_merged: false,
                priority: Default::default(),
                blocked_by: Vec::new(),
                id: 9,
                number: 9,
                title: "Already here".into(),
                body: String::new(),
                kind: ItemKind::Bug,
                status: valkyrie_proto::ItemStatus::Open,
                project: Some("/r/app".into()),
                attachments: Vec::new(),
                by: "human".into(),
                sessions: Vec::new(),
                boot: 0,
                taken: Vec::new(),
                branch: None,
                branch_taken: false,
                checkout: None,
                pr: None,
                filed_by: None,
                unaccepted: false,
                pr_closed: false,
                joined: Default::default(),
                notes: Vec::new(),
                launch: None,
                created: 0,
                updated: 1,
            }],
        })
        .unwrap();
        let typing = |app: &App| match &app.backlog {
            Some(backlog::Page {
                typing: Some(backlog::Typing::Add { kind, text }),
                ..
            }) => Some((*kind, text.text().to_string())),
            _ => None,
        };
        let capturing = |app: &App| app.backlog.as_ref().is_some_and(|p| p.capture);
        let notice = |app: &App| app.view.as_ref().unwrap().notice.clone().map(|n| n.0);
        let added = |seen: &Arc<Mutex<Vec<ClientMsg>>>| -> Vec<NewItem> {
            let seen = seen.lock().unwrap();
            let added = seen.iter().filter_map(|m| match m {
                ClientMsg::AddItem { item, .. } if item.body != "fail" => Some(item.clone()),
                _ => None,
            });
            added.collect()
        };
        let changed = |seen: &Arc<Mutex<Vec<ClientMsg>>>| {
            let seen = seen.lock().unwrap();
            seen.iter()
                .any(|m| matches!(m, ClientMsg::ChangeItem { .. }))
        };

        // A Mac's Option-a types `å`, a letter in its own right: no hint, and it
        // goes to the program.
        app.on_keys("å".as_bytes().to_vec()).await;
        assert_eq!(notice(&app), None);
        assert!(app.backlog.is_none());

        // From a session: what follows Alt-a in the same read is the item's text.
        app.on_keys(b"\x1batab eats".to_vec()).await;
        assert!(capturing(&app));
        assert_eq!(typing(&app), Some((ItemKind::Bug, "tab eats".into())));
        // Keys after ↩ in the same read are dropped: `s` doesn't ship #9.
        app.on_input(b" a letter\rs".to_vec()).await;
        assert!(app.backlog.is_none(), "back to the session");
        assert!(!changed(&seen), "nothing shipped");
        assert_eq!(app.view.as_ref().map(|v| v.id), Some(2));
        assert_eq!(notice(&app).as_deref(), Some("added #1 tab eats a letter"));
        let first = added(&seen)[0].clone();
        assert_eq!(
            (first.kind, first.body.as_str()),
            (ItemKind::Bug, "tab eats a letter")
        );

        // A bracketed paste across reads is one item, its line breaks spaces.
        app.on_keys(b"\x1ba\x1b[200~error: boom\r\n".to_vec()).await;
        app.on_input(b"  at main.rs:3\n\x1b[201~\r".to_vec()).await;
        assert!(app.backlog.is_none());
        assert_eq!(added(&seen)[1].body, "error: boom at main.rs:3");

        // A paste's end marker split across reads is still seen.
        app.on_keys(b"\x1ba\x1b[200~abc\x1b[20".to_vec()).await;
        app.on_input(b"1~\r".to_vec()).await;
        assert!(app.backlog.is_none());
        assert_eq!(added(&seen)[2].body, "abc");
        // A paste that never ends: Esc still cancels.
        app.on_keys(b"\x1ba\x1b[200~lost\r".to_vec()).await;
        assert_eq!(typing(&app), Some((ItemKind::Bug, "lost ".into())));
        app.on_input(b"\x1b".to_vec()).await;
        assert!(app.backlog.is_none());
        assert_eq!(added(&seen).len(), 3);

        // Esc and ^C cancel, back to the session, nothing added.
        for cancel in [b"\x1b", b"\x03"] {
            app.on_keys(b"\x1ba".to_vec()).await;
            assert_eq!(typing(&app), Some((ItemKind::Bug, String::new())));
            app.on_input(b"half a thought".to_vec()).await;
            app.on_input(cancel.to_vec()).await;
            assert!(app.backlog.is_none());
            assert_eq!(app.view.as_ref().map(|v| v.id), Some(2));
        }
        assert_eq!(added(&seen).len(), 3);

        // Refused: the prompt comes back as it was, the reason in the bar.
        app.on_keys(b"\x1ba\tfail\r".to_vec()).await;
        assert_eq!(typing(&app), Some((ItemKind::Feature, "fail".into())));
        assert!(capturing(&app));
        assert_eq!(notice(&app).as_deref(), Some("the daemon said no"));
        app.on_input(b"\x1b".to_vec()).await;
        assert!(app.backlog.is_none());

        // From tab mode too, which it leaves; Tab cycles the kind.
        app.tab_mode = true;
        app.on_keys(b"\x1ba\t".to_vec()).await;
        assert!(!app.tab_mode);
        assert_eq!(typing(&app), Some((ItemKind::Feature, String::new())));
        app.on_input(b"\x1b".to_vec()).await;
        assert!(app.backlog.is_none());

        // From home, in one read: the page closes and home stays.
        app.view = None;
        app.on_input(b"\x1baidea\r".to_vec()).await;
        assert!(app.backlog.is_none());
        assert_eq!(app.status, "added #4 idea");

        // `n` on a page opened with `b` keeps the page open after adding, and so
        // does Alt-a there.
        app.on_input(b"bnx\r".to_vec()).await;
        assert!(app.backlog.as_ref().is_some_and(|p| p.typing.is_none()));
        app.on_input(b"\x1bay\r".to_vec()).await;
        assert!(app.backlog.is_some() && !capturing(&app));
        assert_eq!(added(&seen).len(), 6);
        app.on_input(b"q".to_vec()).await;

        // Unbound, Alt-a goes to the program.
        app.settings.keys.bind(Action::Capture, None);
        app.view = Some(Attached::new(2, "two".into(), None));
        app.on_keys(b"\x1ba".to_vec()).await;
        assert!(app.backlog.is_none());
        let sent = || {
            seen.lock()
                .unwrap()
                .iter()
                .any(|m| matches!(m, ClientMsg::Input { session: 2, data } if data == b"\x1ba"))
        };
        for _ in 0..200 {
            if sent() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(sent(), "Alt-a reached the program");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `.` on the home screen opens the keys page. Enter waits for a key and maps
    /// it, taking it from whoever had it; the map is kept in the settings file, and
    /// `r`, Backspace and `R R` reset, clear and reset everything.
    #[tokio::test]
    async fn backlog_opens_with_b_and_shows_what_waits_on_the_home_screen() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-backlog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        let item = |id: u32, title: &str, status: valkyrie_proto::ItemStatus| Item {
            parent: None,
            pr_merged: false,
            priority: Default::default(),
            blocked_by: Vec::new(),
            id,
            number: id,
            title: title.into(),
            body: String::new(),
            kind: valkyrie_proto::ItemKind::Bug,
            status,
            project: Some("/r/app".into()),
            attachments: Vec::new(),
            by: "claude".into(),
            sessions: Vec::new(),
            boot: 0,
            taken: Vec::new(),
            branch: None,
            branch_taken: false,
            checkout: None,
            pr: None,
            filed_by: None,
            unaccepted: false,
            pr_closed: false,
            joined: Default::default(),
            notes: Vec::new(),
            launch: None,
            created: 0,
            updated: u64::from(id),
        };
        use valkyrie_proto::ItemStatus::{Open, Proposed};
        app.on_push(ServerMsg::Items {
            launches: Vec::new(),
            items: vec![
                item(1, "Tab rename eats a letter", Open),
                item(2, "Flaky exit test", Proposed),
            ],
        })
        .unwrap();
        let text = |terminal: &Terminal<TestBackend>| -> String {
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect()
        };

        // Home: only the filed item waits on the user, as one row, and the
        // cursor rests on it.
        app.restore(Anchor::Session(None));
        assert_eq!(app.row(), Some(InboxRow::Filed));
        let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
        terminal
            .draw(|f| {
                let [body, bar] =
                    Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());
                app.draw_home(f, body, bar)
            })
            .unwrap();
        let home = text(&terminal);
        assert!(
            home.contains("Inbox 1") && home.contains("1 filed"),
            "{home}"
        );
        assert!(!home.contains("Tab rename"), "{home}");

        // Keys after `b` in the same read go to the page, not the home screen.
        app.on_input(b"bj".to_vec()).await;
        let page = app.backlog.clone().unwrap();
        assert_eq!(page.cursor, 1);
        terminal
            .draw(|f| {
                page.draw(
                    f,
                    &app.backlog_rows(),
                    &app.items,
                    &app.sessions,
                    0,
                    0,
                    app.settings.theme,
                )
            })
            .unwrap();
        let shown = text(&terminal);
        for want in [
            "Backlog",
            "2 open",
            "proposed",
            "backlog",
            "Tab rename eats a letter",
            "↩ launch",
        ] {
            assert!(shown.contains(want), "{want}: {shown}");
        }

        // `/` narrows it to one repository's items, the search over the list; a
        // check then offers only those to ship.
        app.on_push(ServerMsg::Items {
            launches: Vec::new(),
            items: vec![
                item(1, "Tab rename eats a letter", Open),
                item(2, "Flaky exit test", Proposed),
                Item {
                    project: Some("/r/lib".into()),
                    ..item(3, "Lib leaks a socket", Open)
                },
            ],
        })
        .unwrap();
        assert_eq!(app.backlog_rows().len(), 3);
        app.on_input(b"/app\r".to_vec()).await;
        let page = app.backlog.clone().unwrap();
        assert_eq!(page.search.query, "app");
        assert!(!page.search.typing, "↩ keeps it and goes back to the list");
        let rows: Vec<u32> = app.backlog_rows().iter().map(|(_, i)| i.id).collect();
        assert_eq!(rows.len(), 2);
        assert!(!rows.contains(&3));
        terminal
            .draw(|f| {
                page.draw(
                    f,
                    &app.backlog_rows(),
                    &app.items,
                    &app.sessions,
                    0,
                    0,
                    app.settings.theme,
                )
            })
            .unwrap();
        assert!(
            text(&terminal).contains(" / app  esc clears"),
            "{}",
            text(&terminal)
        );
        let merged = |id: u32| valkyrie_proto::ShippedItem {
            id,
            number: id,
            title: String::new(),
            branch: format!("fix/{id}"),
            branch_taken: false,
            filed: false,
            against: "origin/main".into(),
            note: None,
        };
        app.on_shipping(Shipping {
            asking: true,
            unfetched: 0,
            result: Ok((vec![merged(1), merged(3)], Vec::new())),
        });
        assert_eq!(app.backlog.as_ref().unwrap().ship_armed, Some(vec![1]));

        app.on_input(b"q".to_vec()).await;
        assert!(app.backlog.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn keys_page_remaps_and_resets() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-keys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        app.settings_file = dir.join("settings.toml");

        app.on_input(b".".to_vec()).await;
        assert_eq!(app.keys_page, Some(KeysPage::default()));
        // Home is the second row: give it Alt-p, which Last had.
        app.on_input(b"j\r".to_vec()).await;
        assert!(app.keys_page.unwrap().capturing);
        app.on_input(b"\x1bp".to_vec()).await;
        assert_eq!(app.settings.keys.get(Action::Home), Some(Key::Alt(b'p')));
        assert_eq!(app.settings.keys.get(Action::Last), None);
        assert!(app.status.contains("taken from last tab"), "{}", app.status);
        assert_eq!(Settings::kept(&app.settings_file).keys, app.settings.keys);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|f| app.draw_keys_page(f, app.keys_page.as_ref().unwrap()))
            .unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("alt p  (default alt h)"), "{text}");
        assert!(text.contains("Last tab"));

        // A key the program needs is refused; an arrow while waiting is ignored.
        app.on_input(b"\r\x1b[A".to_vec()).await;
        assert!(app.keys_page.unwrap().capturing, "still waiting");
        app.on_input(b"\x03".to_vec()).await;
        assert!(app.status.contains("stays the program's"));
        assert_eq!(app.settings.keys.get(Action::Home), Some(Key::Alt(b'p')));
        // Ctrl and a letter is fine.
        app.on_input(b"\r".to_vec()).await;
        app.on_input(b"\x07".to_vec()).await;
        assert_eq!(app.settings.keys.get(Action::Home), Some(Key::Ctrl(0x07)));

        app.on_input(b"\x7f".to_vec()).await;
        assert_eq!(app.settings.keys.get(Action::Home), None);
        app.on_input(b"r".to_vec()).await;
        assert_eq!(app.settings.keys.get(Action::Home), Some(Key::Alt(b'h')));
        app.on_input(b"kR".to_vec()).await;
        assert_eq!(
            app.settings.keys.get(Action::Last),
            None,
            "one R does nothing"
        );
        app.on_input(b"R".to_vec()).await;
        assert_eq!(app.settings.keys, Keymap::default());
        assert_eq!(Settings::kept(&app.settings_file).keys, Keymap::default());
        app.on_input(b"\x1b".to_vec()).await;
        assert_eq!(app.keys_page, None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The tab being renamed, and its name so far.
    fn renaming(app: &App) -> Option<(SessionId, String)> {
        app.renaming
            .as_ref()
            .map(|(id, name)| (*id, name.text().to_owned()))
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
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
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
        assert!(lines[1].contains("codex · ● asks"), "{}", lines[1]);
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

        // The strip sits on the session's background: the theme's, or with
        // session_colors = terminal, the terminal's.
        for (themed, bg) in [(true, app.settings.theme.bg), (false, style::Color::Reset)] {
            app.settings.themed_sessions = themed;
            let mut strip = Terminal::new(TestBackend::new(100, 20)).unwrap();
            strip
                .draw(|frame| {
                    let tabs = app.split_body(frame.area()).1.unwrap();
                    app.draw_tabs(frame, tabs);
                })
                .unwrap();
            assert_eq!(strip.backend().buffer()[(99, 2)].bg, bg, "themed: {themed}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A click on a link opens it; a drag across one only selects. Ctrl+click opens
    /// it over a program that takes the mouse.
    #[tokio::test]
    async fn clicking_a_link_opens_it() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        // Match the layout used below without depending on a host console.
        app.fixed_size = Some(Size {
            cols: 100,
            rows: 20,
        });
        app.settings = Settings::default();
        app.sessions = vec![info(1)];
        let mut view = Attached::new(1, "proj1".into(), None);
        let mut screen = update(true, vec![row(0, "see https://example.com/x now")]);
        screen.size = Size { cols: 80, rows: 10 };
        view.apply(screen);
        app.view = Some(view);
        let main = app.split_body(Rect::new(0, 0, 100, 20)).0;
        let (x, y) = (main.x + 8, main.y);
        let opened = || links::OPENED.with(|o| std::mem::take(&mut *o.borrow_mut()));

        app.on_mouse(Mouse::at(MouseKind::Press, x, y)).await;
        app.on_mouse(Mouse::at(MouseKind::Release, x, y)).await;
        assert_eq!(opened(), ["https://example.com/x"]);
        assert!(app.view.as_ref().unwrap().selection.is_none());

        app.on_mouse(Mouse::at(MouseKind::Press, x, y)).await;
        app.on_mouse(Mouse::at(MouseKind::Drag, x + 3, y)).await;
        app.on_mouse(Mouse::at(MouseKind::Release, x + 3, y)).await;
        assert!(opened().is_empty(), "a drag selects");

        app.on_mouse(Mouse::at(MouseKind::Press, main.x + 1, y))
            .await;
        app.on_mouse(Mouse::at(MouseKind::Release, main.x + 1, y))
            .await;
        assert!(opened().is_empty(), "not on the link");

        app.view.as_mut().unwrap().modes.mouse_click = true;
        app.on_mouse(Mouse::at(MouseKind::Press, x, y)).await;
        app.on_mouse(Mouse::at(MouseKind::Release, x, y)).await;
        assert!(opened().is_empty(), "the program's click");
        let ctrl = Mouse {
            code: 16,
            ..Mouse::at(MouseKind::Press, x, y)
        };
        app.on_mouse(ctrl).await;
        assert_eq!(opened(), ["https://example.com/x"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The bar under a session (spec: Attached): its name, state and where it
    /// works on the left; the one count and the ways out on the right, plain
    /// words, no chips. Hints are off by default: that leaves the count and
    /// where every key is, which a click opens. Tab mode says esc.
    #[test]
    fn the_session_bar_is_name_state_place_then_count_and_ways_out() {
        assert!(
            !Settings::default().hints,
            "keys are off the bar by default"
        );
        let view = Attached::new(2, "proj2".into(), None);
        let settings = Settings::default();
        let bar = |mode, hints, need| {
            let b = attached_bar(
                &view,
                "api · fix/login · #4",
                None,
                mode,
                need,
                hints,
                100,
                settings.theme,
                &settings.keys,
            );
            let text = b
                .line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            (text, b.clicks)
        };
        let (on, clicks) = bar(BarMode::Session, true, 5);
        assert!(on.starts_with(" proj2  api · fix/login · #4"), "{on}");
        assert!(!on.contains("#2") && !on.contains("alt 1-9"), "{on}");
        assert!(
            on.trim_end()
                .ends_with("● 5 need you alt n   ^\\ actions   alt h home"),
            "{on}"
        );
        assert_eq!(on.chars().count(), 100, "right-aligned to the width");
        assert!(clicks.iter().any(|c| c.2 == BarClick::Action(Action::Tabs)));
        assert!(!on.contains(" ? "), "with hints, no keys hint: {on}");
        let (off, clicks) = bar(BarMode::Session, false, 5);
        assert!(
            off.trim_end().ends_with("● 5 need you   ^\\ ? keys"),
            "{off}"
        );
        assert!(!off.contains("actions"), "{off}");
        assert!(clicks.iter().any(|c| c.2 == BarClick::Help));
        let (quiet, _) = bar(BarMode::Session, false, 0);
        assert!(quiet.trim_end().ends_with("^\\ ? keys"), "{quiet}");
        let (tabs, _) = bar(BarMode::Tabs, true, 0);
        assert!(tabs.trim_end().ends_with("esc back"), "{tabs}");
    }

    /// At 80 columns a long place (a long item branch) gives way, not the right
    /// side: the bar fits the width and still ends with its way out (backlog #284).
    #[test]
    fn a_long_place_never_pushes_the_way_out_off_the_bar() {
        let view = Attached::new(1, "work".into(), None);
        let settings = Settings::default();
        let place = "valkyrie · feat/294-keybinds-in-valk-are-not-displayed-by-default · #283";
        for (mode, need, end) in [
            (BarMode::Tabs, 0, "esc back"),
            (BarMode::Session, 3, "alt h home"),
            (BarMode::Renaming, 0, "folder name"),
        ] {
            let text = attached_bar(
                &view,
                place,
                None,
                mode,
                need,
                true,
                80,
                settings.theme,
                &settings.keys,
            )
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
            assert!(text.chars().count() <= 80, "{mode:?} overflows: {text}");
            assert!(text.trim_end().ends_with(end), "{mode:?}: {text}");
            assert!(text.starts_with(" work "), "{mode:?}: {text}");
            assert!(text.contains('…'), "{mode:?} cuts the place: {text}");
        }
    }

    /// Minimal tabs, in every style. On top: one row each, the number, the state's
    /// icon and the name, no program or state word. In a column: a rail of numbers
    /// and icons that opens over the session, which keeps its size, under the
    /// mouse. Clicks find the tabs either way; `m` in tab mode switches.
    #[tokio::test]
    async fn minimal_tabs_are_one_row_and_a_rail_at_the_side() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-mini-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings_file = dir.join("settings.toml");
        app.settings = Settings::default();
        app.settings.minimal_tabs = true;
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
        for side in TabSide::ALL.iter().copied() {
            for style in TabStyle::ALL.iter().copied() {
                for open in [false, true] {
                    if open && side == TabSide::Top {
                        continue;
                    }
                    app.settings.tab_side = side;
                    app.settings.tabs = style;
                    app.rail_hover = open;
                    let (main, tabs) = app.split_body(body);
                    let tabs = tabs.expect("room for the strip");
                    match side {
                        TabSide::Top => assert_eq!(tabs.height, style.rows(false) - 1),
                        _ => {
                            let width = if open { MINIMAL_SIDE_WIDTH } else { RAIL_WIDTH };
                            assert_eq!(tabs.width, width);
                            assert_eq!(main.width, 100 - RAIL_WIDTH, "the session keeps its size");
                        }
                    }
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
                    let open_name = if open { "-open" } else { "" };
                    snapshot(
                        &buffer,
                        &format!("minimal-tabs-{}-{}{open_name}", side.name(), style.name()),
                    );
                    let at = |x: u16, y: u16| buffer[(x, y)].symbol().to_string();
                    let text =
                        |r: Rect, y: u16| (r.x..r.right()).map(|x| at(x, y)).collect::<String>();
                    let strip: String = (tabs.y..tabs.bottom())
                        .map(|y| text(tabs, y) + "\n")
                        .collect();
                    let what = format!("{side:?} {style:?} open={open}\n{strip}");
                    let layout = app.tab_layout(tabs);
                    let rects: Vec<Rect> = layout.tabs.iter().map(|&(_, r)| r).collect();
                    assert_eq!(rects.len(), 3, "{what}");
                    if side != TabSide::Top {
                        assert_eq!(rects[1].y, rects[0].y + 1, "back to back: {what}");
                    }
                    let name_row = |r: Rect| {
                        if style == TabStyle::Folder && side == TabSide::Top {
                            r.y + 1
                        } else {
                            r.y
                        }
                    };
                    let third = text(rects[2], name_row(rects[2]));
                    if side == TabSide::Top || open {
                        assert!(third.contains("3 ● proj3"), "{what}");
                        assert!(strip.contains("⌂"), "{what}");
                    } else {
                        assert!(third.contains("3 ●") && !strip.contains("proj"), "{what}");
                        assert!(strip.contains("●1"), "home says one needs you: {what}");
                    }
                    for gone in ["codex", "claude", "needs input", "working", "fish"] {
                        assert!(!strip.contains(gone), "{gone}: {what}");
                    }
                    assert_eq!(
                        app.tab_hit(tabs, rects[2].x + 2, name_row(rects[2])),
                        Some(TabHit::Session(3)),
                        "{what}"
                    );
                }
            }
        }

        // The mouse on the rail opens it, and off it shuts it.
        app.settings.tab_side = TabSide::Left;
        app.rail_hover = false;
        app.fixed_size = Some(Size {
            cols: 100,
            rows: 20,
        });
        let motion = |x, y| Mouse {
            kind: MouseKind::Other,
            code: 35,
            x,
            y,
            release: false,
        };
        app.on_mouse(motion(2, 5)).await;
        assert!(app.rail_hover && app.rail_open());
        app.on_mouse(motion(12, 5)).await;
        assert!(app.rail_hover, "still on the open rail");
        app.on_mouse(motion(40, 5)).await;
        assert!(!app.rail_open());
        // Tab mode leaves it shut: it no longer moves along the tabs.
        app.tab_mode = true;
        assert!(!app.rail_open());

        app.tab_keys(b"m").await;
        assert!(!app.settings.minimal_tabs);
        assert!(app.tab_mode, "still in tab mode");
        let saved = std::fs::read_to_string(dir.join("settings.toml")).unwrap();
        assert!(saved.contains("tab_detail = \"full\""), "{saved}");
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
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
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
            assert!(text(rects[2], rects[2].y + 1).contains("codex · ● asks"));
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
        // Short enough that the panel shows the whole settings path: macOS's temp dir
        // alone is about 50 bytes.
        let base = if cfg!(target_os = "macos") {
            PathBuf::from("/tmp")
        } else {
            std::env::temp_dir()
        };
        let dir = base.join(format!("valkyrie-tui-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
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
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        // Not whatever this machine's settings.toml says.
        app.settings = Settings::default();
        app.sessions = (1..=20).map(info).collect();
        let area = Rect::new(0, 0, 80, 2);
        let layout = app.tab_layout(area);
        assert_eq!(layout.tabs[0].0, 0);
        assert!(layout.more_after.is_some() && layout.more_before.is_none());
        app.view = Some(Attached::new(18, "proj18".into(), None));
        let layout = app.tab_layout(area);
        assert!(layout.tabs.iter().any(|(i, _)| *i == 17));
        assert!(layout.more_before.is_some());
        assert!(layout.plus.right() <= area.right());
        for (_, r) in &layout.tabs {
            assert!(r.right() <= layout.plus.x, "tabs overlap the +");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A long skill's text is a preview, not a gate: `a` takes it unread, and
    /// the wheel scrolls it in the review overlay.
    #[tokio::test]
    async fn a_long_skill_scrolls_by_wheel_and_is_not_gated() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir =
            std::env::temp_dir().join(format!("valkyrie-tui-skill-wheel-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        let mut skill = proposed("/r/api", 1, "Cut a release");
        skill.kind = valkyrie_proto::DecisionKind::Skill;
        skill.body = (1..=40).map(|i| format!("{i}. step {i}\n")).collect();
        app.proposals = vec![skill];
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut draw = |app: &App| {
            terminal
                .draw(|frame| app.draw_reviews(frame, frame.area(), 0))
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<String>>()
                .join("\n")
        };
        assert!(draw(&app).contains("1. step 1"));
        // Unread, and `a` is not blocked: the first answer is the daemon's, not a gate's.
        // The fake daemon never answers, so reaching it (timing out) is the proof.
        let asked = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            app.review_at(0, true),
        )
        .await;
        assert!(
            asked.is_err(),
            "refused before asking the daemon: {asked:?}"
        );
        // The wheel over the review overlay's text (the home screen's is tested
        // with the home mouse).
        let over = app.proposal_scroll.get().area;
        let wheel = |y: u16| format!("\x1b[<65;{};{}M", over.x + 2, y + 1).into_bytes();
        app.view = Some(Attached::new(1, "p".into(), None));
        app.reviewing = Some(0);
        for _ in 0..20 {
            app.on_input(wheel(over.y + 1)).await;
            draw(&app);
        }
        assert!(draw(&app).contains("40. step 40"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// What the home screen says, while attached: a peek at the tab under the
    /// strip's cursor, the decisions to review over the session, and what a ping
    /// is about.
    #[tokio::test]
    async fn a_session_shows_what_the_home_screen_would() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-peek-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.sessions = (1..=2)
            .map(|id| {
                let mut s = info(id);
                s.name = format!("proj{id}");
                s
            })
            .collect();
        app.sessions[0].status.agent = "claude".into();
        app.sessions[0].status.state = AgentState::ReviewReady;
        app.sessions[0].status.summary = Some("Fixed the login\nredirect".into());
        app.preview = Some((1, "cargo test\nall 41 passed\n\n".into()));
        app.settings.hints = true;
        app.proposals = vec![
            proposed("/r/api", 1, "Use pnpm"),
            proposed("/r/api", 2, "Never pkill by pattern"),
        ];
        app.proposals[1].body = "It killed tool shells.".into();
        let mut view = Attached::new(2, "proj2".into(), None);
        view.apply(update(true, vec![row(0, "inside session 2")]));
        app.view = Some(view);

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
                    if let Some(session) = app.peeked() {
                        app.draw_peek(frame, main, tabs, session);
                    }
                    if let Some(cursor) = app.reviewing {
                        app.draw_reviews(frame, main, cursor);
                    }
                    let line = attached_bar(
                        view,
                        "",
                        app.bar_panes(view.id),
                        BarMode::Session,
                        app.proposals.len(),
                        app.settings.hints,
                        bar.width,
                        app.settings.theme,
                        &app.settings.keys,
                    )
                    .line;
                    frame.render_widget(Paragraph::new(line), bar);
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<String>>()
        };

        // The bar says decisions wait, though home is out of sight.
        let lines = draw(&app, &mut terminal);
        assert!(lines[23].contains("● 2 need you"), "{}", lines[23]);
        assert!(!lines.join("\n").contains("all 41 passed"));

        // The mouse resting on another tab peeks at it: its state, its summary and
        // the bottom of its screen, and what a click does. Not at once, so a quick
        // click doesn't flash one.
        app.fixed_size = Some(Size {
            cols: 120,
            rows: 23,
        });
        let tabs = app.tabs_area().unwrap();
        let (_, first) = app.tab_layout(tabs).tabs[0];
        let motion = |x, y| Mouse {
            kind: MouseKind::Other,
            code: 35,
            x,
            y,
            release: false,
        };
        let rested = |app: &mut App| {
            app.hover_tab = app.hover_tab.map(|(i, at)| (i, at - HOVER_DELAY));
        };
        app.on_mouse(motion(first.x + 2, first.y)).await;
        assert!(app.peeked().is_none() && app.peek_due().is_some());
        // Moving along the same tab doesn't start the wait over.
        let since = app.hover_tab;
        app.on_mouse(motion(first.x + 3, first.y)).await;
        assert_eq!(app.hover_tab, since);
        rested(&mut app);
        assert_eq!(app.peeked().map(|s| s.id), Some(1));
        assert!(app.peek_due().is_none());
        let screen = draw(&app, &mut terminal).join("\n");
        assert!(screen.contains("proj1 #1"), "{screen}");
        assert!(screen.contains("Fixed the login"), "{screen}");
        assert!(screen.contains("all 41 passed"), "{screen}");
        assert!(screen.contains("click switch"), "{screen}");
        // Its own tab peeks at nothing: you're looking at it.
        let (_, own) = app.tab_layout(tabs).tabs[1];
        app.on_mouse(motion(own.x + 2, own.y)).await;
        rested(&mut app);
        assert!(app.peeked().is_none());
        // Off the strip it closes, and so does a key.
        app.on_mouse(motion(first.x + 2, first.y)).await;
        rested(&mut app);
        app.on_mouse(motion(first.x + 2, 15)).await;
        assert!(app.peeked().is_none());
        app.on_mouse(motion(first.x + 2, first.y)).await;
        rested(&mut app);
        assert!(app.peeked().is_some());
        app.on_input(b"x".to_vec()).await;
        assert!(app.peeked().is_none());

        // The bar's chips click as their keys press.
        let click = |x| Mouse {
            kind: MouseKind::Press,
            code: 0,
            x,
            y: 23,
            release: false,
        };
        let chip = |app: &App, want: BarClick| {
            let bar = app.bar(app.view.as_ref().unwrap());
            let lead = bar
                .line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            assert!(
                lead.contains("need you alt n   ^\\ actions   alt h home"),
                "actions lead: {lead}"
            );
            bar.clicks
                .iter()
                .find(|c| c.2 == want)
                .map(|c| c.0 + 1)
                .unwrap()
        };
        app.on_mouse(click(chip(&app, BarClick::Action(Action::Tabs))))
            .await;
        assert!(app.tab_mode);
        app.on_keys(b"\x1b".to_vec()).await;
        app.on_mouse(click(0)).await;
        assert_eq!(
            (app.reviewing, app.tab_mode),
            (None, false),
            "the name isn't a chip"
        );

        // `d` in tab mode lists the decisions over the session, the first one open.
        app.on_keys(vec![TABS_KEY, b'd']).await;
        assert_eq!((app.tab_mode, app.reviewing), (false, Some(0)));
        let screen = draw(&app, &mut terminal).join("\n");
        assert!(screen.contains("decisions to review"), "{screen}");
        assert!(
            screen.contains("Use pnpm") && screen.contains("Never pkill"),
            "{screen}"
        );
        assert!(!screen.contains("It killed tool shells"), "{screen}");
        app.on_keys(b"jjj".to_vec()).await;
        assert_eq!(app.reviewing, Some(1), "stops at the last");
        let screen = draw(&app, &mut terminal).join("\n");
        assert!(screen.contains("It killed tool shells"), "{screen}");
        // Esc goes back to the session; what follows goes to the program.
        app.on_keys(b"\x1b".to_vec()).await;
        assert_eq!(app.reviewing, None);

        // Reviewed away: the cursor stays in the list, and the last one closes it.
        app.reviewing = Some(1);
        app.on_push(ServerMsg::Proposals {
            items: vec![proposed("/r/api", 1, "Use pnpm")],
        })
        .unwrap();
        assert_eq!(app.reviewing, Some(0));
        app.on_push(ServerMsg::Proposals { items: vec![] }).unwrap();
        assert_eq!(app.reviewing, None);
        // With none, `d` says so rather than opening an empty list.
        app.on_keys(vec![TABS_KEY, b'd']).await;
        assert_eq!(app.reviewing, None);
        assert!(app.view.as_ref().unwrap().notice.is_some());

        // A ping says what the session finished, on one line, and short.
        let ping = Ping {
            kind: Kind::Done,
            session: Some(1),
            text: "proj1 finished".into(),
            sound: false,
            proposal: None,
        };
        let toast: String = toast_spans(&ping, app.toast_summary(&ping), app.settings.theme)
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(toast, " ● proj1 finished  Fixed the login redirect  ");
        app.sessions[0].status.summary = Some("x".repeat(200));
        let toast = toast_spans(&ping, app.toast_summary(&ping), app.settings.theme);
        assert_eq!(toast[1].content.width(), TOAST_SUMMARY_MAX + 2);

        // A `valk notify` is a toast with no session behind it, and no summary.
        app.settings.sound = false;
        app.settings.desktop = false;
        app.on_push(ServerMsg::Notice {
            text: "deploy done".into(),
            sound: Some(valkyrie_proto::NoticeSound::Request),
        })
        .unwrap();
        let notice = app.toast().expect("the notice shows as a toast");
        assert_eq!((notice.kind, notice.session), (Kind::Request, None));
        assert_eq!(app.toast_summary(notice), None);
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
        let _listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        // Not whatever this machine's settings.toml says: the two-row cards.
        app.settings = Settings::default();
        app.settings.hints = true;
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

        // Alt-o cycles focus through the panes (the right click above focused 2).
        app.focus(1);
        app.on_keys(b"\x1bo".to_vec()).await;
        assert_eq!(app.view.as_ref().unwrap().id, 2);
        app.on_keys(b"\x1bo".to_vec()).await;
        assert_eq!(app.view.as_ref().unwrap().id, 1);
        // Tab mode leaves getting around to the direct keys.
        app.on_keys(vec![TABS_KEY, b'o', b'\x1b']).await;
        assert_eq!(app.view.as_ref().unwrap().id, 1);

        // Alt-z zooms the focused pane: drawn alone over the whole area, with all
        // its room; the other keeps its split size and runs on, hidden.
        let bar_text = |app: &App| -> String {
            let view = app.view.as_ref().unwrap();
            app.bar(view)
                .line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        // Zoom is ^\\'s, not a chip on the slim bar.
        assert!(!bar_text(&app).contains("zoom"), "no key chips on the bar");
        let split = app.pane_sizes();
        let hidden = app.pane_mut(2).unwrap().size;
        app.on_keys(b"\x1bz".to_vec()).await;
        assert!(app.is_zoomed());
        let full = Size { cols: 81, rows: 21 };
        assert_eq!(app.pane_mut(1).unwrap().size, full);
        assert_eq!(app.pane_mut(2).unwrap().size, hidden);
        let lines = draw(&app);
        assert!(lines[1].contains("⊡2 fish"), "{}", lines[1]);
        assert!(lines[2].starts_with("left pane"), "{}", lines[2]);
        assert!(
            lines[2..23]
                .iter()
                .all(|l| !l.contains('│') && !l.contains("right pane")),
            "{lines:?}"
        );
        assert!(
            bar_text(&app).contains("⊡ zoomed 1/2"),
            "{}",
            bar_text(&app)
        );
        // Both panes are still watched, at the sizes the daemon gets.
        assert_eq!(app.pane_ids().len(), 2);
        // A click anywhere lands in the zoomed pane.
        app.on_mouse(Mouse::at(MouseKind::Press, 60, 5)).await;
        app.on_mouse(Mouse::at(MouseKind::Release, 60, 5)).await;
        assert_eq!(app.view.as_ref().unwrap().id, 1);

        // The zoom follows Alt-o: the next pane fills the screen, the last one goes
        // back to its split size.
        app.on_keys(b"\x1bo".to_vec()).await;
        assert_eq!(app.view.as_ref().unwrap().id, 2);
        assert!(app.is_zoomed());
        assert_eq!(app.pane_mut(2).unwrap().size, full);
        assert_eq!(
            Some(app.pane_mut(1).unwrap().size),
            split.iter().find(|p| p.0 == 1).map(|p| p.1)
        );
        assert!(draw(&app)[2].starts_with("right pane"));
        assert!(bar_text(&app).contains("⊡ zoomed 2/2"));

        // Alt-z again brings the split back, as it was.
        app.on_keys(b"\x1bz".to_vec()).await;
        assert!(!app.is_zoomed());
        assert_eq!(app.pane_sizes(), split);
        let lines = draw(&app);
        assert!(lines[1].contains("⊞2 fish"), "{}", lines[1]);
        assert!(lines[2].contains("right pane"), "{}", lines[2]);

        // `z` in tab mode and the pane menu's item toggle it too.
        app.on_keys(vec![TABS_KEY, b'z']).await;
        assert!(app.is_zoomed() && !app.tab_mode);
        app.open_focused_pane_menu();
        assert_eq!(app.menu.unwrap().items()[4], "Zoom / unzoom");
        app.pick(4).await;
        assert!(!app.is_zoomed() && app.menu.is_none());

        // A tab of one pane has nothing to zoom.
        app.toggle_zoom();
        app.layout = None;
        assert!(!app.is_zoomed());
        app.zoomed = false;
        app.toggle_zoom();
        assert!(!app.zoomed);
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

    /// Alt-m opens the mini terminal over a session or home and Alt-m again puts it
    /// away: while it is open the shell gets every other key, the daemon watches
    /// it beside the tab's panes, and an exited shell closes the box.
    #[tokio::test]
    async fn mini_terminal_opens_takes_keys_and_hides() {
        use std::sync::{Arc, Mutex};
        use valkyrie_proto::codec::{read_frame, write_frame};
        use valkyrie_proto::{ClientMsg, Reply};
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-minibox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        #[cfg_attr(unix, allow(unused_mut))]
        let mut listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let seen: Arc<Mutex<Vec<ClientMsg>>> = Arc::default();
        let kept = seen.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut rd, mut wr) = tokio::io::split(stream);
            while let Ok(Some(msg)) = read_frame::<_, ClientMsg>(&mut rd).await {
                kept.lock().unwrap().push(msg.clone());
                let answer = match msg {
                    ClientMsg::Spawn { req, .. } => ServerMsg::Ok {
                        req,
                        reply: Reply::Session {
                            info: Box::new(info(7)),
                        },
                    },
                    ClientMsg::AttachPanes { req, .. } => ServerMsg::Ok {
                        req,
                        reply: Reply::Done,
                    },
                    _ => continue,
                };
                if write_frame(&mut wr, &answer).await.is_err() {
                    break;
                }
            }
        });
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        app.fixed_size = Some(Size {
            cols: 100,
            rows: 30,
        });
        app.sessions = vec![info(2)];
        app.view = Some(Attached::new(2, "two".into(), None));
        let sent = || seen.lock().unwrap().clone();
        // The client sends from a task and the fake daemon reads from another.
        let typed = || -> Vec<Vec<u8>> {
            sent()
                .into_iter()
                .filter_map(|m| match m {
                    ClientMsg::Input { session: 7, data } => Some(data),
                    _ => None,
                })
                .collect()
        };

        app.on_keys(b"\x1bm".to_vec()).await;
        assert!(app.mini_open());
        // Hidden: spawned for the mini's size, in the session's directory.
        let spawn = sent()
            .into_iter()
            .find_map(|m| match m {
                ClientMsg::Spawn { spec, .. } => Some(spec),
                _ => None,
            })
            .expect("spawned a shell");
        assert!(spawn.hidden);
        assert_eq!(spawn.cwd, Some(PathBuf::from("/")));
        // Watched beside the attached pane.
        let panes = sent().into_iter().rev().find_map(|m| match m {
            ClientMsg::AttachPanes { panes, .. } => Some(panes),
            _ => None,
        });
        let ids: Vec<_> = panes.unwrap().iter().map(|p| p.0).collect();
        assert_eq!(ids, vec![2, 7]);

        // Keys go to the shell, Alt-m included only as the way out; the rest of the
        // read after it goes back to the session underneath.
        app.on_input(b"ls\r".to_vec()).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(typed(), vec![b"ls\r".to_vec()]);
        app.on_input(b"\x1b[<0;5;5M".to_vec()).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(typed().len(), 1, "the mouse isn't the shell's");
        app.on_input(b"x\x1bm".to_vec()).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!app.mini_open());
        assert_eq!(typed().last(), Some(&b"x".to_vec()));
        let panes = sent().into_iter().rev().find_map(|m| match m {
            ClientMsg::AttachPanes { panes, .. } => Some(panes),
            _ => None,
        });
        assert_eq!(
            panes.unwrap().iter().map(|p| p.0).collect::<Vec<_>>(),
            vec![2]
        );

        // Back again: the same shell, no second spawn.
        app.on_input(b"\x1bm".to_vec()).await;
        assert!(app.mini_open());
        let spawns = sent()
            .iter()
            .filter(|m| matches!(m, ClientMsg::Spawn { .. }))
            .count();
        assert_eq!(spawns, 1);

        // It draws as a box over the screen, with the key to put it away.
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 31)).unwrap();
        terminal.draw(|frame| app.draw_mini(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..31)
            .flat_map(|y| (0..100).map(move |x| (x, y)))
            .map(|(x, y)| buffer[(x, y)].symbol())
            .collect();
        assert!(text.contains("mini terminal"), "{text}");
        assert!(text.contains("alt m puts it away"), "{text}");

        // The shell exiting closes the box and drops the shell.
        app.mini.as_mut().unwrap().pane.exited = Some(Some(0));
        app.mini_reap().await;
        assert!(!app.mini_open() && app.mini.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The box is centered, and never smaller than a usable shell when the
    /// terminal has room.
    #[test]
    fn mini_box_is_centered_and_bounded() {
        let r = mini::mini_rect(Rect::new(0, 0, 100, 30));
        assert_eq!((r.width, r.height), (80, 18));
        assert_eq!((r.x, r.y), (10, 6));
        let small = mini::mini_rect(Rect::new(0, 0, 44, 12));
        assert_eq!((small.width, small.height), (40, 10));
        let tiny = mini::mini_rect(Rect::new(0, 0, 30, 6));
        assert_eq!((tiny.width, tiny.height), (30, 6));
    }

    #[test]
    fn a_csi_ends_at_an_embedded_esc() {
        assert_eq!(next_key(b"\x1b[\x1b"), b"\x1b[");
        assert_eq!(next_key(b"\x1b[1;\x1bx"), b"\x1b[1;");
        assert_eq!(next_key(b"\x1b[Ax"), b"\x1b[A");
        assert_eq!(next_key(b"\x1b[1;"), b"\x1b[1;");
    }

    #[test]
    fn paste_changes_only_when_the_prompt_opens_or_closes() {
        assert_eq!(paste_change(true, false, false), Some('h'));
        assert_eq!(paste_change(false, true, false), Some('l'));
        assert_eq!(
            paste_change(false, true, true),
            Some('h'),
            "the program wants it"
        );
        assert_eq!(paste_change(true, true, false), None);
        assert_eq!(paste_change(false, false, true), None);
    }

    #[test]
    fn forced_modes_turn_paste_on_and_leave_the_rest() {
        let m = Modes {
            app_cursor: true,
            ..Modes::default()
        };
        let f = forced_modes(m, true);
        assert!(f.bracketed_paste && f.app_cursor);
        assert_eq!(forced_modes(m, false), m);
    }

    /// A click on a row of the home screen selects it, and a click on the selected
    /// row does what Enter does. Mouse reports used to be thrown away there (#96).
    #[tokio::test]
    async fn clicking_a_home_row_selects_it_and_a_second_click_opens_it() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-click-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.sessions = ["alpha", "bravo", "charlie"]
            .into_iter()
            .zip(1..)
            .map(|(name, id)| SessionInfo {
                name: name.into(),
                ..info(id)
            })
            .collect();
        app.queue = vec![queued(3)];
        app.proposals = vec![proposed("/r/a", 1, "one")];
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let screen = |app: &App, terminal: &mut Terminal<TestBackend>| -> Vec<String> {
            terminal
                .draw(|frame| {
                    let [body, bar] =
                        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                            .areas(frame.area());
                    app.draw_home(frame, body, bar);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            buffer
                .content()
                .chunks(buffer.area.width as usize)
                .map(|row| row.iter().map(|c| c.symbol()).collect())
                .collect()
        };
        let rows = screen(&app, &mut terminal);
        // The sessions list, not the queue's row for the same session.
        let y = rows.iter().rposition(|r| r.contains("bravo")).unwrap() as u16;
        let x = rows[y as usize].find("bravo").unwrap() as u16;
        let click =
            |x: u16, y: u16| format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", x + 1, y + 1, x + 1, y + 1);

        assert_eq!(app.selected, 0);
        app.on_input(click(x, y).into_bytes()).await;
        assert_eq!(app.selection().0, Some(2), "the click selected bravo");
        assert_eq!(app.status, "");
        // The border and the blank space under the rows select nothing.
        let before = app.selected;
        app.on_input(click(x, 0).into_bytes()).await;
        app.on_input(click(x, 28).into_bytes()).await;
        assert_eq!(app.selected, before);
        // The decisions' one row: the first click selects it, the second is Enter,
        // which opens them to review (a session's attaches: the test after this).
        screen(&app, &mut terminal);
        let y = rows.iter().position(|r| r.contains("decision")).unwrap() as u16;
        let x = rows[y as usize].find("decision").unwrap() as u16;
        app.on_input(click(x, y).into_bytes()).await;
        assert_eq!((app.row(), app.reviewing), (Some(InboxRow::Decide), None));
        screen(&app, &mut terminal);
        app.on_input(click(x, y).into_bytes()).await;
        assert_eq!(app.reviewing, Some(0));
        // Open over home, it takes the keys: `q` closes it rather than quitting,
        // and the rows behind don't move.
        let behind = app.selected;
        app.on_input(b"k".to_vec()).await;
        assert_eq!((app.selected, app.reviewing), (behind, Some(0)));
        app.on_input(b"q".to_vec()).await;
        assert_eq!((app.reviewing, app.quit), (None, false));
        app.on_input(click(x, y).into_bytes()).await;
        assert_eq!(app.reviewing, Some(0));
        app.on_input(b"\x1b".to_vec()).await;
        assert_eq!((app.reviewing, app.quit), (None, false));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A second click on a session's row attaches it, as Enter does; back home,
    /// `x` twice closes the selected one and it leaves the list (backlog #106).
    #[tokio::test]
    async fn a_second_click_on_a_session_attaches_and_x_x_closes_one() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use valkyrie_proto::ClientMsg;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-open-{}", std::process::id()));
        let sessions = ["alpha", "bravo", "charlie"]
            .into_iter()
            // Not 1, 2, 3: attach refuses the session running this test (VALK_SESSION).
            .zip(9001..)
            .map(|(name, id)| SessionInfo {
                name: name.into(),
                ..info(id)
            })
            .collect();
        let (mut app, sent) = answering_app(&dir, sessions).await;
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        let mut screen = |app: &App| -> Vec<String> {
            terminal
                .draw(|frame| {
                    let [body, bar] =
                        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                            .areas(frame.area());
                    app.draw_home(frame, body, bar);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            buffer
                .content()
                .chunks(buffer.area.width as usize)
                .map(|row| row.iter().map(|c| c.symbol()).collect())
                .collect()
        };
        let click =
            |x: u16, y: u16| format!("\x1b[<0;{};{}M\x1b[<0;{};{}m", x + 1, y + 1, x + 1, y + 1);
        let rows = screen(&app);
        let y = rows.iter().rposition(|r| r.contains("bravo")).unwrap() as u16;
        let x = rows[y as usize].find("bravo").unwrap() as u16;
        app.on_input(click(x, y).into_bytes()).await;
        assert_eq!(app.selection().0, Some(9002));
        assert!(app.view.is_none(), "the first click only selects");
        screen(&app);
        app.on_input(click(x, y).into_bytes()).await;
        assert_eq!(
            app.view.as_ref().map(|v| v.id),
            Some(9002),
            "{}",
            app.status
        );
        assert!(
            sent.lock().unwrap().iter().any(
                |m| matches!(m, ClientMsg::AttachPanes { panes, .. } if panes.len() == 1 && panes[0].0 == 9002)
            ),
            "it asked the daemon to watch bravo"
        );
        // Attached, the mouse's motion too; written where a test can read it,
        // not to the terminal running the tests (backlog #303).
        let written = term::take_written();
        assert!(
            written.contains(&format!("{}{}", mouse::CAPTURE_ON, mouse::MOTION_ON)),
            "{written:?}"
        );

        app.detach().await;
        assert!(app.view.is_none());
        screen(&app);
        assert_eq!(app.selection().0, Some(9002), "home, still on bravo");
        app.on_input(b"x".to_vec()).await;
        assert!(
            app.sessions.iter().any(|s| s.id == 9002),
            "one x only arms it"
        );
        app.on_input(b"x".to_vec()).await;
        assert!(
            sent.lock()
                .unwrap()
                .iter()
                .any(|m| matches!(m, ClientMsg::Kill { session: 9002, .. }))
        );
        let left: Vec<_> = app.sessions.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(left, ["alpha", "charlie"], "{}", app.status);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// What a key says on a page shows on the page, in the row over its keys,
    /// and goes with the next key (backlog #277).
    #[tokio::test]
    async fn a_pages_message_shows_over_its_keys() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-said-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        let screen = |app: &App| -> Vec<String> {
            let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
            terminal
                .draw(|f| {
                    app.memory
                        .as_ref()
                        .unwrap()
                        .draw(f, &app.proposals, app.settings.theme);
                    app.draw_said(f);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            buffer
                .content()
                .chunks(100)
                .map(|r| r.iter().map(|c| c.symbol()).collect())
                .collect()
        };
        app.status = "said at home".into();
        app.memory = Some(memory::Page::new(memory::Tab::Decisions));
        // A key on the page: what was said before it is gone.
        app.on_input(b"j".to_vec()).await;
        assert!(!screen(&app).join("\n").contains("said at home"));
        app.say("review failed: that's for the user".into());
        let rows = screen(&app);
        assert!(
            rows[18].contains("review failed: that's for the user"),
            "the row over the keys: {rows:#?}"
        );
        assert!(rows[19].contains("j/k move"), "the keys stay: {rows:#?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Backlog #281: the mouse works on a section page. A click on a row moves
    /// the cursor there, the wheel moves it, and the strip's sections take a
    /// click from the backlog too, which used to read a click as keys.
    #[tokio::test]
    async fn the_mouse_works_on_section_pages() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-pmouse-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        let item = |id: u32| -> Item {
            serde_json::from_value(serde_json::json!({
                "id": id, "number": id, "title": format!("item {id}"), "status": "open",
                "by": "human", "project": "/r/app", "created": 0, "updated": 0,
            }))
            .unwrap()
        };
        app.on_push(ServerMsg::Items {
            items: vec![item(1), item(2), item(3)],
            launches: vec![],
        })
        .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
        // As `App::draw` does at home: the strip and home, the page over them.
        let mut draw = |app: &App| -> Vec<String> {
            terminal
                .draw(|f| {
                    let [body, bar] =
                        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                            .areas(f.area());
                    app.draw_home(f, body, bar);
                    if let Some(page) = &app.backlog {
                        let rows = app.backlog_rows();
                        let t = app.settings.theme;
                        page.draw(f, &rows, &app.items, &app.sessions, 0, 0, t);
                    }
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            (0..30)
                .map(|y| (0..140).map(|x| buffer[(x, y)].symbol()).collect())
                .collect()
        };
        let find = |rows: &[String], s: &str| -> (u16, u16) {
            rows.iter()
                .enumerate()
                .find_map(|(y, r)| r.find(s).map(|i| (r[..i].chars().count() as u16, y as u16)))
                .unwrap_or_else(|| panic!("{s:?} on {rows:#?}"))
        };
        // An SGR press, as the terminal sends it (1-based).
        let click = |x: u16, y: u16| format!("\x1b[<0;{};{}M", x + 1, y + 1).into_bytes();
        let cursor = |app: &App| app.backlog.as_ref().unwrap().cursor;
        let at = |app: &App, id: u32| {
            app.backlog_rows()
                .iter()
                .position(|(_, i)| i.id == id)
                .unwrap()
        };

        app.on_input(b"gb".to_vec()).await;
        let rows = draw(&app);
        let (x, y) = find(&rows, "item 3");
        app.on_input(click(x, y)).await;
        assert_eq!(app.section(), Section::Backlog, "a click isn't keys");
        assert_eq!(cursor(&app), at(&app, 3), "the click's row is selected");

        // The wheel moves the cursor a row a notch.
        draw(&app);
        let before = cursor(&app);
        app.on_input(b"\x1b[<64;10;10M".to_vec()).await;
        assert_eq!(cursor(&app), before - 1);

        // On the selected row a click is ↩; elsewhere, the j/k that get there.
        let rows = draw(&app);
        let hits = &app.backlog.as_ref().unwrap().hits;
        let (x, y) = find(&rows, "item 3");
        assert_eq!(hits.click(x, y), Some(b"j".to_vec()));
        // The selected row, by its number at the list's left edge.
        let id = app.backlog_rows()[cursor(&app)].1.id;
        let y = rows
            .iter()
            .position(|r| r.starts_with(&format!("  #{id} ")))
            .unwrap() as u16;
        assert_eq!(hits.click(4, y), Some(b"\r".to_vec()));
        assert_eq!(hits.click(4, 0), None, "the strip is no row");

        // The strip takes a click from the backlog: no more trapped there.
        // (Inbox: another page would wait on a daemon this one never answers,
        // backlog #106.)
        let rows = draw(&app);
        let x = rows[0]
            .find("Inbox")
            .map(|i| rows[0][..i].chars().count() as u16);
        app.on_input(click(x.expect("Inbox on the strip"), 0)).await;
        assert_eq!(app.section(), Section::Inbox);
        assert!(app.backlog.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Backlog #203: at 80 columns with hints on, the hints fill the session's
    /// bar, so a toast takes the row over it instead of falling off the end.
    #[tokio::test]
    async fn a_toast_over_a_narrow_session_bar_is_seen() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir =
            std::env::temp_dir().join(format!("valkyrie-tui-bar-toast-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.settings = Settings::default();
        app.settings.hints = true;
        app.sessions = vec![info(1)];
        let mut view = Attached::new(1, "proj1".into(), None);
        view.apply(update(true, vec![row(0, "inside session 1")]));
        app.view = Some(view);
        let screen = |app: &App| -> Vec<String> {
            let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
            terminal
                .draw(|f| {
                    let [body, bar] =
                        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                            .areas(f.area());
                    app.draw_session_bar(f, app.view.as_ref().unwrap(), body, bar);
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            buffer
                .content()
                .chunks(80)
                .map(|r| r.iter().map(|c| c.symbol()).collect())
                .collect()
        };
        let before = screen(&app);
        app.show_ping(Ping::notice("hi from another pane", None));
        let rows = screen(&app);
        assert!(
            rows[22].contains("hi from another pane"),
            "the row over the bar: {rows:#?}"
        );
        assert_eq!(rows[23], before[23], "the bar keeps its hints");
        assert!(!rows[23].contains("hi from"), "{rows:#?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The composer (backlog #276): the top row, where the cursor rests at inbox
    /// zero; `n` types, letters never reach the Inbox, ↩ starts the agent in a
    /// worktree of the repository shown, ^f files it, `#` finds an item.
    #[tokio::test]
    async fn the_composer_starts_files_and_finds() {
        use std::sync::{Arc, Mutex};
        use valkyrie_proto::codec::{read_frame, write_frame};
        use valkyrie_proto::{ClientMsg, KnownRepo, Reply, RepoList};
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-compose-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        #[cfg_attr(unix, allow(unused_mut))]
        let mut listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let item = |id: u32, title: &str, project: &str, after: &[u32]| -> Item {
            serde_json::from_value(serde_json::json!({
                "id": id, "number": id, "title": title, "status": "open", "by": "human",
                "project": project, "created": id, "updated": 0, "blocked_by": after,
            }))
            .unwrap()
        };
        let items = vec![
            item(1, "Rate-limit retries in the client", "/r/app", &[]),
            item(2, "Rate-limit the public API", "/r/app", &[3]),
            item(3, "Pin the runner image", "/r/app", &[]),
        ];
        let sent: Arc<Mutex<Vec<ClientMsg>>> = Arc::default();
        let kept = sent.clone();
        let filed = item(7, "Add a --json flag", "/r/web", &[]);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut rd, mut wr) = tokio::io::split(stream);
            while let Ok(Some(msg)) = read_frame::<_, ClientMsg>(&mut rd).await {
                kept.lock().unwrap().push(msg.clone());
                let answer = match msg {
                    ClientMsg::Repos { req } => ServerMsg::Ok {
                        req,
                        reply: Reply::Repos {
                            list: RepoList {
                                repos: ["/r/app", "/r/web"]
                                    .map(|p| KnownRepo {
                                        path: p.into(),
                                        label: p.into(),
                                        added: false,
                                    })
                                    .to_vec(),
                                folders: vec![],
                                hidden: vec![],
                            },
                        },
                    },
                    ClientMsg::AddItem { req, .. } => ServerMsg::Ok {
                        req,
                        reply: Reply::Item {
                            item: Box::new(filed.clone()),
                            note: None,
                        },
                    },
                    // Not started here: attaching needs a real session.
                    ClientMsg::Spawn { req, .. } | ClientMsg::LaunchItem { req, .. } => {
                        ServerMsg::Err {
                            req,
                            message: "not in a test".into(),
                        }
                    }
                    _ => continue,
                };
                if write_frame(&mut wr, &answer).await.is_err() {
                    break;
                }
            }
        });
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        app.sessions = vec![info(1)];
        app.items = items;
        let took = |sent: &Arc<Mutex<Vec<ClientMsg>>>| std::mem::take(&mut *sent.lock().unwrap());

        // Inbox zero: the cursor rests on the composer, and the pane is it.
        app.restore(Anchor::Session(None));
        assert_eq!(app.row(), Some(InboxRow::Compose));
        let wide = home_screen(&app, 140).join("\n");
        for want in [
            "+ New session",
            "What's next?",
            "describe a task, or # for a backlog item",
            "Ready in the backlog",
            "s  app#1",
            "n type",
            "s start app#1",
        ] {
            assert!(wide.contains(want), "lacks {want:?}:\n{wide}");
        }
        assert!(
            !wide.contains("app#2"),
            "a blocked item isn't ready:\n{wide}"
        );
        // Something waiting: the cursor starts there, and k goes back up.
        app.queue = vec![queued(1)];
        app.restore(Anchor::Session(None));
        assert_eq!(app.row(), Some(InboxRow::Queue(0)));
        app.on_input(b"k".to_vec()).await;
        assert_eq!(app.row(), Some(InboxRow::Compose));
        app.queue.clear();

        // Typing: the Inbox's keys are letters now, and the footer is the
        // composer's alone.
        app.on_input(b"n".to_vec()).await;
        assert!(app.composer.typing);
        app.on_input(b"gx Add a --json flag".to_vec()).await;
        assert_eq!(app.section(), Section::Inbox, "g types");
        assert!(app.kill_armed.is_none() && !app.quit);
        app.on_input(vec![0x17, 0x17, 0x17, 0x17, 0x17]).await;
        app.on_input(b"Add a --json flag".to_vec()).await;
        let foot = home_screen(&app, 140).last().cloned().unwrap();
        assert!(
            foot.contains("↩ start")
                && foot.contains("^f file to backlog")
                && !foot.contains("j/k"),
            "{foot}"
        );

        // Nothing named, nothing picked: no repository (backlog #292). tab
        // steps to the first, then the next; ↩ starts the agent there, in a
        // worktree.
        assert!(
            home_screen(&app, 140)
                .join("\n")
                .contains("no repository · claude")
        );
        app.on_input(b"\t\t".to_vec()).await;
        assert!(home_screen(&app, 140).join("\n").contains("web · claude"));
        app.on_input(b"\r".to_vec()).await;
        let spawn = took(&sent)
            .into_iter()
            .find_map(|m| match m {
                ClientMsg::Spawn { spec, .. } => Some(spec),
                _ => None,
            })
            .expect("↩ spawns");
        assert_eq!(spawn.command, ["claude", "Add a --json flag"]);
        assert_eq!(spawn.cwd.as_deref(), Some(Path::new("/r/web")));
        assert_eq!(spawn.worktree.as_deref(), Some(""));
        assert!(
            app.status.starts_with("couldn't start it"),
            "{}",
            app.status
        );
        assert_eq!(
            app.composer.text, "Add a --json flag",
            "a failed start keeps it"
        );

        // ^f files it in that repository instead, a real item.
        app.on_input(vec![compose::FILE_KEY]).await;
        let added = took(&sent)
            .into_iter()
            .find_map(|m| match m {
                ClientMsg::AddItem { item, take, .. } => Some((item, take)),
                _ => None,
            })
            .expect("^f files it");
        assert_eq!(added.0.title, "Add a --json flag");
        assert_eq!(added.0.cwd.as_deref(), Some(Path::new("/r/web")));
        assert!(!added.1);
        assert_eq!(app.status, "filed web#7: Add a --json flag");
        assert!(!app.composer.typing && app.composer.text.is_empty());

        // `#`: the backlog by number or title, the ready one first and the
        // blocked one greyed; ↩ starts the picked item.
        app.on_input(b"#rate".to_vec()).await;
        let found = app.compose_found();
        assert_eq!(found.iter().map(|f| f.id).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(found[1].what, compose::Can::No("after app#3".into()));
        let screen = home_screen(&app, 140).join("\n");
        assert!(
            screen.contains("↩ start app#1") && screen.contains("after app#3"),
            "{screen}"
        );
        app.on_input(b"\r".to_vec()).await;
        assert!(
            took(&sent)
                .iter()
                .any(|m| matches!(m, ClientMsg::LaunchItem { id: 1, .. })),
            "↩ launches #1"
        );
        // Esc goes back to text, then hands the keys back.
        app.on_input(b"\x1b".to_vec()).await;
        assert!(app.composer.typing && app.composer.text.is_empty());
        app.on_input(b"\x1b".to_vec()).await;
        assert!(!app.composer.typing);

        // Under 100 columns, the band is in the list itself.
        app.on_input(b"/".to_vec()).await;
        app.on_input(b"fix the tests".to_vec()).await;
        let narrow = home_screen(&app, 80);
        assert_eq!(narrow[1].matches('╭').count(), 1, "one pane: {narrow:#?}");
        assert!(narrow[2].contains("› fix the tests"), "{narrow:#?}");
        assert!(
            narrow[3].contains("web · claude"),
            "the repository picked stays: {narrow:#?}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Several lines in the composer (backlog #280): ↩ opens Start agents, a
    /// row each with its repository and agent; the footer is the dialog's; ↩
    /// starts every row in its own worktree, and one that fails stays to retry.
    #[tokio::test]
    async fn start_agents_starts_a_session_per_line() {
        use std::sync::{Arc, Mutex};
        use valkyrie_proto::codec::{read_frame, write_frame};
        use valkyrie_proto::{ClientMsg, KnownRepo, Reply, RepoList};
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let socket = dir.join("s.sock");
        valkyrie_proto::ensure_private_dir(&dir).unwrap();
        #[cfg_attr(unix, allow(unused_mut))]
        let mut listener = valkyrie_proto::ipc::Listener::bind(&socket).unwrap();
        let spawned: Arc<Mutex<Vec<SpawnSpec>>> = Arc::default();
        let kept = spawned.clone();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (mut rd, mut wr) = tokio::io::split(stream);
            let mut next = 10;
            while let Ok(Some(msg)) = read_frame::<_, ClientMsg>(&mut rd).await {
                let answer = match msg {
                    ClientMsg::Repos { req } => ServerMsg::Ok {
                        req,
                        reply: Reply::Repos {
                            list: RepoList {
                                repos: ["/r/api", "/r/web"]
                                    .map(|p| KnownRepo {
                                        path: p.into(),
                                        label: p.into(),
                                        added: false,
                                    })
                                    .to_vec(),
                                folders: vec![],
                                hidden: vec![],
                            },
                        },
                    },
                    ClientMsg::List { req } => ServerMsg::Ok {
                        req,
                        reply: Reply::Sessions {
                            sessions: vec![info(1)],
                            layouts: vec![],
                        },
                    },
                    // The web repository refuses: its row stays, with why.
                    ClientMsg::Spawn { req, spec } => {
                        let web = spec.cwd.as_deref() == Some(Path::new("/r/web"));
                        kept.lock().unwrap().push(spec);
                        if web {
                            ServerMsg::Err {
                                req,
                                message: "web is busy".into(),
                            }
                        } else {
                            next += 1;
                            let mut s = info(next);
                            s.name = format!("agent-{next}");
                            ServerMsg::Ok {
                                req,
                                reply: Reply::Session { info: Box::new(s) },
                            }
                        }
                    }
                    _ => continue,
                };
                if write_frame(&mut wr, &answer).await.is_err() {
                    break;
                }
            }
        });
        let (client, _) = Client::connect(&socket).await.unwrap();
        let mut app = App::new(client, PathBuf::new());
        app.settings = Settings::default();
        app.sessions = vec![info(1)];
        app.restore(Anchor::Session(None));

        // A pasted list: the footer offers to review it, ↩ opens the dialog.
        app.on_input(b"n".to_vec()).await;
        app.on_input(
            b"\x1b[200~Add token expiry\nBadge count in web\nCache the release build\x1b[201~"
                .to_vec(),
        )
        .await;
        let foot = home_screen(&app, 120).last().cloned().unwrap();
        assert!(foot.contains("review and start 3"), "{foot}");
        app.on_input(b"\r".to_vec()).await;
        let b = app.batch.as_ref().expect("Start agents is open");
        assert_eq!(b.summary(), "3 tasks · no repository, web");
        let screen = home_screen(&app, 120);
        let text = screen.join("\n");
        assert!(
            text.contains("Start agents") && text.contains("Badge count in web"),
            "{text}"
        );
        let foot = screen.last().unwrap();
        assert!(
            foot.contains("start them all") && foot.contains("esc back") && !foot.contains("j/k"),
            "the dialog's keys alone: {foot}"
        );
        // Letters are the dialog's: `g` isn't a section, `q` doesn't quit.
        app.on_input(b"gq".to_vec()).await;
        assert!(app.batch.is_some() && !app.quit && app.section() == Section::Inbox);

        // Esc: back to the draft, typing, as it was.
        app.on_input(b"\x1b".to_vec()).await;
        assert!(app.batch.is_none() && app.composer.typing);
        assert_eq!(
            app.composer.text,
            "Add token expiry\nBadge count in web\nCache the release build"
        );

        // The third on codex, then ↩ starts all three; web refuses.
        app.on_input(b"\r".to_vec()).await;
        app.on_input(b"jj\x1b[Z\x1b[Z\x1b[Z".to_vec()).await;
        app.on_input(b"\r".to_vec()).await;
        let specs = std::mem::take(&mut *spawned.lock().unwrap());
        let got: Vec<(Vec<String>, Option<PathBuf>, Option<String>)> = specs
            .into_iter()
            .map(|s| (s.command, s.cwd, s.worktree))
            .collect();
        // A line that names no repository starts at home, no worktree (#292).
        let home = || Some(valkyrie_proto::home_dir());
        assert_eq!(
            got,
            [
                (
                    vec!["claude".into(), "Add token expiry".into()],
                    home(),
                    None
                ),
                (
                    vec!["claude".into(), "Badge count in web".into()],
                    Some("/r/web".into()),
                    Some(String::new())
                ),
                (
                    vec!["codex".into(), "Cache the release build".into()],
                    home(),
                    None
                ),
            ]
        );
        assert!(app.view.is_none(), "several: nothing attaches");
        let b = app.batch.as_ref().expect("the refused row stays");
        assert_eq!(b.rows.len(), 1);
        assert_eq!(b.rows[0].failed.as_deref(), Some("web is busy"));
        assert!(
            app.status
                .starts_with("started 2 sessions, 1 couldn't start"),
            "{}",
            app.status
        );
        assert!(
            home_screen(&app, 120)
                .join("\n")
                .contains("couldn't start: web is busy")
        );

        // tab moves it to none, after the last; ↩ starts it and the dialog
        // closes, the draft gone.
        app.on_input(b"\t\r".to_vec()).await;
        assert!(app.batch.is_none());
        assert_eq!(app.status, "started 1 session");
        assert!(app.composer.text.is_empty() && !app.composer.typing);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Renders the home screen to text lines.
    fn home_screen(app: &App, width: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
        terminal
            .draw(|frame| {
                let [body, bar] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)])
                    .areas(frame.area());
                app.draw_home(frame, body, bar);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect())
            .collect()
    }

    fn press(code: u8, x: u16, y: u16) -> Vec<u8> {
        format!("\x1b[<{code};{};{}M", x + 1, y + 1).into_bytes()
    }

    /// The wheel moves the selection on the home screen, and scrolls a proposal's
    /// text when it is over the preview.
    #[tokio::test]
    async fn the_wheel_moves_the_home_selection_and_scrolls_a_proposal() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-wheel-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.sessions = (1..=3).map(info).collect();
        app.on_input(press(65, 5, 5)).await;
        assert_eq!(app.selected, 1, "wheel down moves down");
        app.on_input(press(65, 5, 5)).await;
        app.on_input(press(65, 5, 5)).await;
        app.on_input(press(65, 5, 5)).await;
        assert_eq!(app.selected, 3, "and stops at the last row");
        app.on_input(press(64, 5, 5)).await;
        assert_eq!(app.selected, 2, "wheel up moves up");

        let mut long = proposed("/r/a", 1, "long one");
        long.body = (0..80).map(|i| format!("line {i}\n")).collect();
        app.sessions.clear();
        app.proposals = vec![long];
        app.selected = 1;
        let width = 160;
        home_screen(&app, width);
        // The preview is the right-hand panel, past the lists' 62%.
        let (x, y) = (width - 10, 8);
        assert_eq!(app.proposal_scroll.get().top, 0);
        app.on_input(press(65, x, y)).await;
        assert!(app.proposal_scroll.get().top > 0, "wheel scrolled the text");
        app.on_input(press(64, x, y)).await;
        assert_eq!(app.proposal_scroll.get().top, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A proposal's body shows as markdown, not its source, and a click on a
    /// link in it opens the link.
    #[tokio::test]
    async fn a_proposal_body_is_markdown_and_its_links_open() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-md-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        let mut d = proposed("/r/a", 1, "md one");
        d.body = "## Why\n\n**Bold** words, then [the ADR](https://x.dev/adr).".into();
        app.proposals = vec![d];
        app.selected = 1;
        let width = 160;
        let screen: Vec<String> = home_screen(&app, width);
        let all = screen.join("\n");
        assert!(all.contains("Why") && !all.contains("## Why"), "{all}");
        assert!(all.contains("Bold words") && !all.contains("**"), "{all}");
        let (y, row) = screen
            .iter()
            .enumerate()
            .find(|(_, r)| r.contains("the ADR"))
            .unwrap();
        let x = row
            .chars()
            .collect::<Vec<_>>()
            .windows(7)
            .position(|w| w.iter().collect::<String>() == "the ADR")
            .unwrap() as u16;
        let opened = || links::OPENED.with(|o| std::mem::take(&mut *o.borrow_mut()));
        opened();
        app.on_input(press(0, x + 1, y as u16)).await;
        assert_eq!(opened(), ["https://x.dev/adr"]);
        app.on_input(press(0, x - 3, y as u16)).await;
        assert!(opened().is_empty(), "plain text opens nothing");
        // A session selected now: the old proposal's link is gone with it.
        app.proposals.clear();
        app.sessions = vec![info(1)];
        app.selected = 1;
        app.on_input(press(0, x + 1, y as u16)).await;
        assert!(opened().is_empty(), "no stale link");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `tab` goes to the next row that waits on you, round to the first; a digit
    /// answers only a session that asks; `?` opens every key over a dimmed home,
    /// and Esc closes it.
    #[tokio::test]
    async fn tab_digits_and_the_key_sheet() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-grammar-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.sessions = vec![info(1), info(2), info(3)];
        app.queue = vec![queued(1), queued(3)];
        app.selected = 3; // session 2, below the queue
        app.on_input(b"\t".to_vec()).await;
        assert_eq!(app.selection().0, Some(1), "round to the first waiting");
        app.on_input(b"\t".to_vec()).await;
        assert_eq!(app.selection().0, Some(3));

        // Not an ask (the queue's default state): a digit says so, sends nothing.
        app.on_input(b"2".to_vec()).await;
        assert_eq!(app.status, "1-9 answer a session that asks");
        // Nor a bell from a plain program: the digit would be typed into it.
        let at = app.queue_index().unwrap();
        app.queue[at].status.state = AgentState::NeedsInput;
        app.queue[at].status.ask = Some(valkyrie_proto::AskKind::Bell);
        app.status.clear();
        app.on_input(b"2".to_vec()).await;
        assert_eq!(app.status, "1-9 answer a session that asks");

        app.on_input(b"?".to_vec()).await;
        assert!(app.help);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| app.draw_help(frame)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        for want in [
            "Every key",
            "the next thing that waits on you",
            "starts the tutor",
        ] {
            assert!(text.contains(want), "lacks {want:?}");
        }
        // Keys go to the sheet, not home: `j` doesn't move the cursor.
        let at = app.selected;
        app.on_input(b"j".to_vec()).await;
        assert_eq!((app.help, app.selected), (true, at));
        app.on_input(b"\x1b".to_vec()).await;
        assert!(!app.help);
        // A click over the sheet closes it, and selects nothing behind it.
        app.on_input(b"?".to_vec()).await;
        let at = app.selected;
        app.on_home_mouse(Mouse {
            kind: MouseKind::Press,
            code: 0,
            x: 5,
            y: 3,
            release: false,
        })
        .await;
        assert_eq!((app.help, app.selected), (false, at));

        // An empty home: tab says so.
        app.sessions.clear();
        app.queue.clear();
        app.selected = 0;
        app.on_input(b"\t".to_vec()).await;
        assert_eq!(app.status, "nothing waits on you");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Attached, with hints off (the default), the key sheet is a click on the
    /// bar's keys chip or tab mode's `?` away; while it's open its keys and
    /// clicks are its own, and closing it leaves you in the session.
    #[tokio::test]
    async fn the_key_sheet_opens_over_a_session() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-sheet-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.settings = Settings::default();
        app.sessions = vec![info(1)];
        app.view = Some(Attached::new(1, "proj1".into(), None));
        // No terminal in CI: the bar's row comes from a fixed size.
        app.fixed_size = Some(Size {
            cols: 100,
            rows: 24,
        });

        // Tab mode's key, then `?`.
        app.on_input(b"\x1c?".to_vec()).await;
        assert!(app.help && !app.tab_mode);
        app.on_input(b"j".to_vec()).await;
        assert!(app.help, "the sheet takes the keys");
        app.on_input(b"q".to_vec()).await;
        assert!(!app.help);
        assert_eq!(app.view.as_ref().map(|v| v.id), Some(1));

        // The chip on the bar, then any press closes it.
        let (from, _, _) = app
            .bar(app.view.as_ref().unwrap())
            .clicks
            .into_iter()
            .find(|c| c.2 == BarClick::Help)
            .expect("a keys chip");
        let press = |x, y| Mouse {
            kind: MouseKind::Press,
            code: 0,
            x,
            y,
            release: false,
        };
        app.on_mouse(press(from + 1, 24)).await;
        assert!(app.help);
        // A raw SGR press at (3, 3), through the input path.
        app.on_input(b"\x1b[<0;4;4M".to_vec()).await;
        assert!(!app.help);
        assert_eq!(app.view.as_ref().map(|v| v.id), Some(1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The footer's keys click as they press; `x` asks before it closes.
    #[tokio::test]
    async fn the_home_bars_chips_click_as_their_keys() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-chips-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.sessions = vec![info(1)];
        app.selected = 1;
        let chip = |app: &App, label: &str| -> (u16, u16) {
            let rows = home_screen(app, 120);
            let bar = rows.len() - 1;
            let col = rows[bar]
                .find(label)
                .unwrap_or_else(|| panic!("{label}: {rows:?}"));
            // `find` counts bytes; the bar's glyphs before the chips are not all one.
            (rows[bar][..col].chars().count() as u16, bar as u16)
        };
        // At most five keys for the selection, the first the main one; then
        // getting around, faint.
        let bar = home_screen(&app, 120).last().cloned().unwrap();
        assert!(bar.trim_start().starts_with("↩ open"), "{bar}");
        assert!(bar.contains("? keys"), "{bar}");
        // Narrow, `? keys` is the last of them to go.
        let narrow = home_screen(&app, 64).last().cloned().unwrap();
        assert!(narrow.contains("? keys"), "{narrow}");
        let (x, y) = chip(&app, "rename");
        app.on_input(press(0, x, y)).await;
        assert!(app.renaming.is_some(), "the rename key renames");
        app.renaming = None;
        // `x` asks first, from its key on the bar too.
        let (x, y) = chip(&app, "close");
        app.on_input(press(0, x, y)).await;
        assert_eq!(app.kill_armed, Some(1));
        let notice = home_screen(&app, 120);
        let notice = &notice[notice.len() - 2];
        assert!(notice.contains("x again closes it"), "{notice}");
        // Any other key keeps it.
        app.on_input(b"j".to_vec()).await;
        assert_eq!(app.kill_armed, None);
        // Blank space on the bar is no key.
        app.on_input(press(0, 60, y)).await;
        assert!(app.renaming.is_none() && app.kill_armed.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A disk running low is the first Needs-you row, its key frees space, and it
    /// goes when the daemon says the disk has room again.
    #[tokio::test]
    async fn a_low_disk_is_the_first_inbox_row() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-disk-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        app.sessions = vec![info(1)];
        let low = valkyrie_proto::LowDisk {
            path: "/srv".into(),
            free: 8_240_000_000,
            total: 1_800_000_000_000,
        };
        app.on_push(ServerMsg::Disk { low: vec![low] }).unwrap();
        assert_eq!(app.inbox_rows()[1], InboxRow::Disk);
        assert_eq!(app.need_you(), 1);
        app.selected = 1;
        assert_eq!(app.row(), Some(InboxRow::Disk));
        let rows = home_screen(&app, 120);
        assert!(
            rows.iter()
                .any(|r| r.contains("disk low") && r.contains("8.2 GB free")),
            "{rows:#?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("8.2 GB free of 1.8 TB")),
            "{rows:#?}"
        );
        let bar = rows.last().unwrap();
        assert!(bar.trim_start().starts_with("↩ free space"), "{bar}");
        app.on_push(ServerMsg::Disk { low: Vec::new() }).unwrap();
        assert!(!app.inbox_rows().contains(&InboxRow::Disk));
        assert_eq!(app.need_you(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Items agents filed are one Needs-you row on home, a click target like any.
    #[tokio::test]
    async fn filed_items_are_one_inbox_row() {
        let dir = std::env::temp_dir().join(format!("valkyrie-tui-bpanel-{}", std::process::id()));
        let (mut app, _listener) = idle_app(&dir).await;
        let waiting = Item {
            parent: None,
            pr_merged: false,
            priority: Default::default(),
            blocked_by: Vec::new(),
            id: 2,
            number: 2,
            title: "Flaky exit test".into(),
            body: String::new(),
            kind: valkyrie_proto::ItemKind::Bug,
            status: valkyrie_proto::ItemStatus::Proposed,
            project: Some("/r/app".into()),
            attachments: Vec::new(),
            by: "claude".into(),
            sessions: Vec::new(),
            boot: 0,
            taken: Vec::new(),
            branch: None,
            branch_taken: false,
            checkout: None,
            pr: None,
            filed_by: None,
            unaccepted: false,
            pr_closed: false,
            joined: Default::default(),
            notes: Vec::new(),
            launch: None,
            created: 0,
            updated: 2,
        };
        app.on_push(ServerMsg::Items {
            launches: Vec::new(),
            items: vec![waiting],
        })
        .unwrap();
        let rows = home_screen(&app, 110);
        let y = rows.iter().position(|r| r.contains("1 filed")).unwrap() as u16;
        let filed = app.inbox_rows().iter().position(|r| *r == InboxRow::Filed);
        assert_eq!(
            app.home_hit(Position::new(10, y)).zip(filed),
            Some((Hit::Row(filed.unwrap()), filed.unwrap()))
        );
        // The blank space under it is not.
        assert_eq!(app.home_hit(Position::new(10, 28)), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

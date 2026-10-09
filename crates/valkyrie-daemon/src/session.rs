//! One PTY-hosted program plus its screen model and agent state (DESIGN §13–14).
//!
//! Three threads per session, none of which ever runs on a tokio worker:
//! - reader: PTY output → transcript + screen → broadcast diff (ends at PTY EOF, or
//!   between two reads when an upgrade handoff stops it; ADR-0006)
//! - writer: drains the input queue into the PTY; a program that stops reading stdin
//!   blocks only this thread. A handoff stops it between writes and the unwritten
//!   rest of the queue crosses the exec
//! - waiter: reaps the child and publishes `Exited`, independent of PTY EOF (a
//!   background job can hold the tty open long after the agent exits)
//!
//! Agent state (`Tracker`) lives under the same lock as the screen. Every input to it
//! (hooks, output marks, screen verdicts, bells, attaches, exit) and every state it
//! reaches goes to `<ns>-<id>.events.jsonl` next to the transcript, so a recorded
//! session replays to the same states (DESIGN §14.6).

use crate::{chat, foreground};
use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Notify, broadcast};
use valkyrie_agents::{Adapter, AgentEvent, Screen, Tracker};
use valkyrie_proto::AgentStatus;
use valkyrie_proto::{
    AgentState, Reply, ScrollAnchor, ServerMsg, SessionId, SessionInfo, Size, SpawnSpec,
};
use valkyrie_term::{Signal, VtScreen, graphics};

/// Server pushes for one session. Lagging receivers resync from a fresh snapshot.
pub type Feed = broadcast::Receiver<Arc<ServerMsg>>;

const FEED_CAPACITY: usize = 1024;
/// How long `kill` waits after SIGHUP before escalating to SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(2);
/// Screen heuristics run once output has been quiet this long, never per chunk.
const SCAN_QUIET_MS: u64 = 150;
/// ...and glance at least this often while output keeps coming: an animated spinner
/// never goes quiet, and Codex's auto-reviewer only shows under one. Glances only
/// drive `Tracker::glance`.
const SCAN_EVERY_MS: u64 = 500;
/// Per session, the most image data kept for clients that attach later.
const GRAPHICS_LIMIT: usize = 32 << 20;
/// At most one output offset mark per this interval in the event log.
const MARK_EVERY_MS: u64 = 250;

/// What every session needs from the daemon.
pub struct Host {
    pub transcript_dir: PathBuf,
    /// The valk binary that agent hooks run (`valk hook <agent>`).
    pub hook_exe: PathBuf,
    /// Exported to sessions so their hooks reach this daemon.
    pub socket: PathBuf,
    /// Poked whenever any session's agent status changes.
    pub changed: Arc<Notify>,
    /// Set to stop every reader for an upgrade handoff; replaced if it fails.
    pub stop: Mutex<Arc<StopPipe>>,
    /// The last cell size in pixels a client reported; new sessions start with it.
    pub cell_px: Mutex<Option<(u16, u16)>>,
    /// The decision store (ADR-0007), exported so `valk context hook` reads this
    /// daemon's.
    pub context_dir: PathBuf,
}

/// Readers poll this alongside their PTY and return, before reading another byte,
/// once it is set: unread output stays in the kernel for the next daemon image.
pub struct StopPipe {
    read: OwnedFd,
    write: OwnedFd,
}

impl StopPipe {
    pub fn new() -> std::io::Result<Self> {
        // Close-on-exec, on every platform.
        let (read, write) = std::io::pipe()?;
        Ok(Self {
            read: read.into(),
            write: write.into(),
        })
    }

    /// Level-triggered and never drained, so every poller sees it, now and later.
    pub fn set(&self) {
        // SAFETY: writes one byte from a valid buffer to our own pipe.
        unsafe { libc::write(self.write.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
    }
}

/// The PTY master, held as a plain fd so it can survive an exec (ADR-0006).
struct Pty(OwnedFd);

impl Pty {
    /// `cell` in pixels fills the winsize pixel fields, which image programs read.
    fn resize(&self, size: Size, cell: (u16, u16)) -> std::io::Result<()> {
        let ws = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.cols.saturating_mul(cell.0),
            ws_ypixel: size.rows.saturating_mul(cell.1),
        };
        // SAFETY: TIOCSWINSZ reads a winsize from a valid pointer.
        if unsafe { libc::ioctl(self.0.as_raw_fd(), libc::TIOCSWINSZ, &ws) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// The foreground process group, as the shell's job control set it.
    fn foreground(&self) -> Option<i32> {
        // SAFETY: tcgetpgrp only reads the terminal's state.
        let group = unsafe { libc::tcgetpgrp(self.0.as_raw_fd()) };
        (group > 0).then_some(group)
    }

    /// An independent handle for a reader or writer thread.
    fn file(&self) -> std::io::Result<File> {
        Ok(File::from(self.0.try_clone()?))
    }
}

/// Input waiting for the PTY. The writer takes from the front; when a handoff stops
/// it mid-buffer, the unwritten rest goes back to the front, and the whole queue
/// crosses the exec in `SavedSession::input`.
#[derive(Default)]
struct InputQueue {
    state: Mutex<InputState>,
    ready: Condvar,
}

#[derive(Default)]
struct InputState {
    bufs: VecDeque<Vec<u8>>,
    /// Stop taking input (handoff); cleared if the handoff fails.
    halt: bool,
    /// The session is gone; the writer ends once the queue is empty.
    closed: bool,
}

impl InputQueue {
    fn push(&self, data: Vec<u8>) {
        let mut state = self.state.lock().unwrap();
        if !state.closed {
            state.bufs.push_back(data);
            self.ready.notify_one();
        }
    }

    fn set(&self, f: impl FnOnce(&mut InputState)) {
        f(&mut self.state.lock().unwrap());
        self.ready.notify_all();
    }
}

/// What reaps the program: the handle from spawning it, or (after a handoff, where
/// only the pid crossed the exec) `waitpid` on that pid.
enum Reap {
    Child(Box<dyn Child + Send + Sync>),
    Pid(u32),
}

/// One session as it crosses an upgrade handoff (ADR-0006).
#[derive(Debug, Serialize, Deserialize)]
pub struct SavedSession {
    pub id: SessionId,
    name: String,
    command: Vec<String>,
    cwd: PathBuf,
    pub pid: Option<u32>,
    created_unix: u64,
    /// The PTY master, inherited across the exec.
    pub fd: RawFd,
    transcript: PathBuf,
    events: PathBuf,
    offset: u64,
    size: Size,
    status: AgentStatus,
    exited: Option<Option<i32>>,
    /// Input the old image had not written to the PTY yet.
    #[serde(default)]
    pub input: Vec<u8>,
    #[serde(default)]
    conversation: Option<String>,
    #[serde(default)]
    chat: Option<PathBuf>,
    /// Its place in the tab order; older images had none (id order).
    #[serde(default)]
    rank: Option<u64>,
    #[serde(default)]
    driven_by: Option<String>,
}

/// What brings a session back after the daemon restarts (DESIGN §8.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RestoreEntry {
    pub name: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub conversation: Option<String>,
}

pub struct Session {
    pub id: SessionId,
    /// The name given at spawn or by a rename; lists otherwise show the directory.
    name: Mutex<Option<String>>,
    /// Its place in the tab order: a new session goes last.
    rank: AtomicU64,
    command: Vec<String>,
    cwd: PathBuf,
    pid: Option<u32>,
    created_unix: u64,
    pub clients: AtomicU32,
    /// When a client last typed into it (ms since the epoch; 0: never).
    last_input_ms: AtomicU64,
    adapter: &'static dyn Adapter,
    state: Mutex<State>,
    input: Arc<InputQueue>,
    pty: Pty,
    transcript: PathBuf,
    events: PathBuf,
    reader: Mutex<Option<JoinHandle<()>>>,
    writer: Mutex<Option<JoinHandle<()>>>,
    feed: broadcast::Sender<Arc<ServerMsg>>,
    changed: Arc<Notify>,
}

/// The fixed facts about a session, whether spawned or adopted.
struct Meta {
    id: SessionId,
    name: Option<String>,
    rank: u64,
    command: Vec<String>,
    cwd: PathBuf,
    pid: Option<u32>,
    created_unix: u64,
    transcript: PathBuf,
    events: PathBuf,
}

/// Everything shared threads mutate. Diffs and `Exited` are broadcast while this lock
/// is held, so a subscriber that snapshots under the same lock never misses or
/// repeats a message.
struct State {
    screen: VtScreen,
    exited: Option<Option<i32>>,
    tracker: Tracker,
    log: File,
    /// Transcript bytes so far; event log lines point into the transcript with it.
    offset: u64,
    last_output_ms: u64,
    last_mark_ms: u64,
    last_scan_ms: u64,
    scan_due: bool,
    /// The agent's own conversation id, from its hooks, to resume it after a restart.
    conversation: Option<String>,
    /// The agent that started this session or typed into it (ADR-0007 §4): what
    /// runs here may be the agent's doing, so it can only propose decisions.
    driven_by: Option<String>,
    /// What its agent has been doing, for the agents beside it.
    activity: crate::activity::Activity,
    /// The agent's own transcript, for the web app's Chat view (DESIGN §8.7).
    chat: Option<PathBuf>,
    /// When the program exited (ms), if it did while this image ran.
    exited_ms: Option<u64>,
    /// Images still shown, for clients that attach later (DESIGN §8.4).
    graphics: graphics::Log,
    /// The terminal's foreground process group at the last look, and when the
    /// processes in it were last checked for an agent.
    fg_group: Option<i32>,
    fg_checked_ms: u64,
    /// Where the foreground program is now (`cd` moves a shell); `None` until known.
    live_cwd: Option<PathBuf>,
}

impl State {
    fn log(&mut self, now: u64, kind: &str, mut fields: Value) {
        fields["t"] = now.into();
        fields["k"] = kind.into();
        fields["off"] = self.offset.into();
        let mut line = fields.to_string();
        line.push('\n');
        if let Err(e) = self.log.write_all(line.as_bytes()) {
            tracing::warn!("event log write failed: {e}");
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl Session {
    pub fn spawn(id: SessionId, spec: SpawnSpec, host: &Host) -> Result<Arc<Session>> {
        let size = spec.size.clamped();
        let program = spec.command.first().context("empty command")?.clone();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let adapter = valkyrie_agents::adapter_for(&spec.command);
        let cwd = match spec.cwd {
            Some(dir) => dir,
            None => std::env::current_dir()?,
        };
        // portable-pty would quietly start the program somewhere else.
        anyhow::ensure!(cwd.is_dir(), "no such directory: {}", cwd.display());

        // Opened before spawning so a failure here cannot orphan the child. Ids restart
        // with the daemon, so the nanosecond timestamp keeps transcripts distinct.
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&host.transcript_dir)?;
        let stem = host.transcript_dir.join(format!("{}-{id}", now.as_nanos()));
        let private = |ext: &str| {
            OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(stem.with_extension(ext))
        };
        let transcript = private("raw")?;
        let log = private("events.jsonl")?;
        let transcript_path = stem.with_extension("raw");
        let events_path = stem.with_extension("events.jsonl");

        let cell = *host.cell_px.lock().unwrap();
        let pair = native_pty_system()
            .openpty(pty_size(size, cell.unwrap_or((0, 0))))
            .context("openpty")?;
        let mut argv = spec.command.clone();
        adapter.prepare(&mut argv, &host.hook_exe);
        let mut cmd = CommandBuilder::new(&program);
        cmd.args(&argv[1..]);
        cmd.cwd(&cwd);
        // Not the daemon's: programs that trust `$PWD` (yazi) would start elsewhere.
        cmd.env("PWD", &cwd);
        for (key, value) in &spec.env {
            match value {
                Some(value) => cmd.env(key, value),
                None => cmd.env_remove(key),
            }
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("VALK_SESSION", id.to_string());
        cmd.env("VALK_SOCKET", &host.socket);
        cmd.env("VALK_CONTEXT", &host.context_dir);

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawn {program}"))?;
        drop(pair.slave);
        let fd = pair.master.as_raw_fd().context("pty master has no fd")?;
        // SAFETY: the master is open until `pair.master` drops, after this dup.
        let pty = Pty(unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?);
        drop(pair.master);

        let now_ms = now.as_millis() as u64;
        let mut screen = VtScreen::new(size);
        if let Some(cell) = cell {
            screen.set_cell_pixels(cell);
        }
        let mut state = State {
            screen,
            exited: None,
            tracker: Tracker::new(adapter.name(), adapter.hook_gaps(), now_ms),
            log,
            offset: 0,
            last_output_ms: now_ms,
            last_mark_ms: 0,
            last_scan_ms: 0,
            scan_due: false,
            conversation: None,
            driven_by: None,
            activity: Default::default(),
            chat: None,
            exited_ms: None,
            graphics: graphics::Log::new(GRAPHICS_LIMIT),
            fg_group: None,
            fg_checked_ms: 0,
            live_cwd: None,
        };
        state.log(
            now_ms,
            "start",
            json!({"agent": adapter.name(), "command": spec.command, "size": size}),
        );

        let pid = child.process_id();
        let session = Session::new(
            Meta {
                id,
                name: own_name(spec.name, &spec.command),
                // Ids only grow, and ranks are renumbered from 0 on a move.
                rank: id as u64,
                command: spec.command,
                cwd,
                pid,
                created_unix: now.as_secs(),
                transcript: transcript_path,
                events: events_path,
            },
            adapter,
            state,
            pty,
            host,
        );
        session.start_threads(Some(Reap::Child(child)), transcript, host)?;
        Ok(session)
    }

    /// Takes over a session saved by the previous daemon image (ADR-0006): the PTY
    /// fd came across the exec, the screen is rebuilt from the transcript, and the
    /// program is still our child, so `waitpid` keeps working.
    pub fn adopt(saved: SavedSession, host: &Host, generation: u32) -> Result<Arc<Session>> {
        // SAFETY: the previous image passed this fd to us and nothing else owns it.
        let fd = unsafe { OwnedFd::from_raw_fd(saved.fd) };
        // It crossed the exec without CLOEXEC; children we spawn must not inherit it.
        // SAFETY: plain fcntl on an fd we own.
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
        let adapter = valkyrie_agents::adapter_for(&saved.command);
        // Best effort from here on: losing the screen or a log beats losing the agent.
        let (mut screen, graphics) =
            rebuild_screen(&saved.transcript, &saved.events, saved.offset, saved.size)
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        session = saved.id,
                        "screen not rebuilt, starting blank: {e:#}"
                    );
                    (
                        VtScreen::new(saved.size),
                        graphics::Log::new(GRAPHICS_LIMIT),
                    )
                });
        if let Some(cell) = *host.cell_px.lock().unwrap() {
            screen.set_cell_pixels(cell);
        }
        let now = now_ms();
        let log = append_private(&saved.events)?;
        let transcript = append_private(&saved.transcript)?;
        let mut state = State {
            screen,
            exited: saved.exited,
            tracker: Tracker::resume(
                saved.status,
                adapter.hook_gaps(),
                saved.exited.is_some(),
                now,
            ),
            log,
            offset: saved.offset,
            last_output_ms: now,
            last_mark_ms: 0,
            last_scan_ms: 0,
            scan_due: false,
            conversation: saved.conversation,
            driven_by: saved.driven_by,
            activity: Default::default(),
            chat: saved.chat,
            // Exited under the old image: long enough ago to drop from the restore list.
            exited_ms: saved.exited.map(|_| 0),
            graphics,
            fg_group: None,
            fg_checked_ms: 0,
            live_cwd: None,
        };
        state.log(now, "resume", json!({"generation": generation}));
        let reap = match (saved.exited, saved.pid) {
            (None, Some(pid)) => Some(Reap::Pid(pid)),
            _ => None,
        };
        let session = Session::new(
            Meta {
                id: saved.id,
                name: own_name(Some(saved.name), &saved.command),
                rank: saved.rank.unwrap_or(saved.id as u64),
                command: saved.command,
                cwd: saved.cwd,
                pid: saved.pid,
                created_unix: saved.created_unix,
                transcript: saved.transcript,
                events: saved.events,
            },
            adapter,
            state,
            Pty(fd),
            host,
        );
        if !saved.input.is_empty() {
            session.input.push(saved.input);
        }
        session.start_threads(reap, transcript, host)?;
        Ok(session)
    }

    fn new(
        meta: Meta,
        adapter: &'static dyn Adapter,
        state: State,
        pty: Pty,
        host: &Host,
    ) -> Arc<Session> {
        let (feed, _) = broadcast::channel(FEED_CAPACITY);
        Arc::new(Session {
            id: meta.id,
            name: Mutex::new(meta.name),
            rank: AtomicU64::new(meta.rank),
            command: meta.command,
            cwd: meta.cwd,
            pid: meta.pid,
            created_unix: meta.created_unix,
            clients: AtomicU32::new(0),
            last_input_ms: AtomicU64::new(0),
            adapter,
            state: Mutex::new(state),
            input: Arc::default(),
            pty,
            transcript: meta.transcript,
            events: meta.events,
            reader: Mutex::new(None),
            writer: Mutex::new(None),
            feed,
            changed: host.changed.clone(),
        })
    }

    fn start_threads(
        self: &Arc<Self>,
        reap: Option<Reap>,
        transcript: File,
        host: &Host,
    ) -> Result<()> {
        if let Some(reap) = reap {
            let waiter = self.clone();
            std::thread::Builder::new()
                .name(format!("pty-x-{}", self.id))
                .spawn(move || waiter.wait_exit(reap))?;
        }
        self.start_io(transcript, host.stop.lock().unwrap().clone())
    }

    fn start_io(self: &Arc<Self>, transcript: File, stop: Arc<StopPipe>) -> Result<()> {
        // Non-blocking (the reader and writer share one open file description), so
        // a stopped writer is never stuck inside write(); both poll first.
        // SAFETY: plain fcntl on our own fd.
        unsafe {
            let fd = self.pty.0.as_raw_fd();
            libc::fcntl(
                fd,
                libc::F_SETFL,
                libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK,
            );
        }
        // The writer holds no Arc<Session>: it ends once the session is dropped
        // (`closed`), which happens after the reader sees EOF.
        let (writer, queue, id, writer_stop) =
            (self.pty.file()?, self.input.clone(), self.id, stop.clone());
        let handle = std::thread::Builder::new()
            .name(format!("pty-w-{id}"))
            .spawn(move || write_loop(id, writer, queue, writer_stop))?;
        *self.writer.lock().unwrap() = Some(handle);
        let reader = self.pty.file()?;
        let session = self.clone();
        let handle = std::thread::Builder::new()
            .name(format!("pty-r-{id}"))
            .spawn(move || session.pump(reader, transcript, stop))?;
        *self.reader.lock().unwrap() = Some(handle);
        Ok(())
    }

    /// Waits until the reader and writer have returned, after `StopPipe::set`. Unread
    /// output stays in the kernel; unwritten input stays in the queue.
    pub fn stop_io(&self) {
        self.input.set(|state| state.halt = true);
        for thread in [&self.reader, &self.writer] {
            if let Some(handle) = thread.lock().unwrap().take() {
                let _ = handle.join();
            }
        }
    }

    /// Restarts the reader and writer after a handoff that failed to exec.
    pub fn resume_io(self: &Arc<Self>, stop: Arc<StopPipe>) -> Result<()> {
        self.input.set(|state| state.halt = false);
        let transcript = append_private(&self.transcript)?;
        self.start_io(transcript, stop)
    }

    pub fn exited(&self) -> bool {
        self.state.lock().unwrap().exited.is_some()
    }

    /// SIGKILL now, for a killed session that must not outlive a handoff unreaped.
    pub fn force_kill(&self) {
        if let Some(pid) = self.pid {
            self.signal_group(pid, libc::SIGKILL);
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Everything the next daemon image needs; call with the reader stopped.
    pub fn save(&self) -> SavedSession {
        let state = self.state.lock().unwrap();
        SavedSession {
            id: self.id,
            name: self.saved_name(),
            rank: Some(self.rank()),
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            pid: self.pid,
            created_unix: self.created_unix,
            fd: self.pty.0.as_raw_fd(),
            transcript: self.transcript.clone(),
            events: self.events.clone(),
            offset: state.offset,
            size: state.screen.size(),
            status: state.tracker.status().clone(),
            exited: state.exited,
            conversation: state.conversation.clone(),
            driven_by: state.driven_by.clone(),
            chat: state.chat.clone(),
            input: self
                .input
                .state
                .lock()
                .unwrap()
                .bufs
                .iter()
                .flatten()
                .copied()
                .collect(),
        }
    }

    pub fn pty_fd(&self) -> RawFd {
        self.pty.0.as_raw_fd()
    }

    fn watching(&self) -> bool {
        self.clients.load(Ordering::Relaxed) > 0
    }

    fn pump(self: Arc<Self>, mut reader: File, mut transcript: File, stop: Arc<StopPipe>) {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut fds = [
                libc::pollfd {
                    fd: reader.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: stop.read.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: `fds` is a valid array of two pollfds.
            if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            // Checked first: whatever is readable now belongs to the next image.
            if fds[1].revents != 0 {
                tracing::debug!(session = self.id, "reader stopped for handoff");
                return;
            }
            // POLLHUP/POLLERR show up as EOF or EIO from read.
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                Err(_) => break,
            };
            let bytes = &buf[..n];
            if let Err(e) = transcript.write_all(bytes) {
                tracing::warn!(session = self.id, "transcript write failed: {e}");
            }
            let now = now_ms();
            let mut state = self.state.lock().unwrap();
            let signals = state.screen.feed(bytes);
            if let Some(update) = state.screen.take_diff() {
                let _ = self.feed.send(Arc::new(ServerMsg::Screen {
                    session: self.id,
                    update,
                }));
            }
            state.offset += n as u64;
            state.last_output_ms = now;
            state.scan_due = true;
            if now.saturating_sub(state.last_mark_ms) >= MARK_EVERY_MS {
                state.last_mark_ms = now;
                state.log(now, "out", json!({}));
            }
            let mut changed = state.tracker.output(now);
            let mut replies = Vec::new();
            for signal in signals {
                match signal {
                    Signal::Reply(reply) => replies.push(reply),
                    Signal::Bell => {
                        state.log(now, "bell", json!({}));
                        changed |= state.tracker.apply(&AgentEvent::Bell, now, self.watching());
                    }
                    Signal::Title(_) => {}
                    Signal::Graphics(cmd) => {
                        let kept = state.graphics.record(cmd);
                        let _ = self.feed.send(Arc::new(ServerMsg::Graphics {
                            session: self.id,
                            x: kept.x,
                            y: kept.y,
                            data: kept.data.to_string(),
                        }));
                    }
                    Signal::Clipboard(text) => {
                        state.log(now, "copy", json!({"chars": text.chars().count()}));
                        let _ = self.feed.send(Arc::new(ServerMsg::Clipboard {
                            session: self.id,
                            text,
                        }));
                    }
                }
            }
            if changed {
                self.after_change(&mut state, now);
            }
            drop(state);
            for reply in replies {
                self.write_input(reply);
            }
        }
        tracing::debug!(session = self.id, "pty closed");
    }

    fn wait_exit(self: Arc<Self>, reap: Reap) {
        let code = match reap {
            Reap::Child(mut child) => child.wait().ok().map(|s| s.exit_code() as i32),
            Reap::Pid(pid) => wait_pid(pid),
        };
        tracing::info!(session = self.id, ?code, "exited");
        let now = now_ms();
        let mut state = self.state.lock().unwrap();
        state.exited = Some(code);
        state.exited_ms = Some(now);
        let _ = self.feed.send(Arc::new(ServerMsg::Exited {
            session: self.id,
            code,
        }));
        state.log(now, "exit", json!({"code": code}));
        if state
            .tracker
            .apply(&AgentEvent::Exited { code }, now, self.watching())
        {
            self.after_change(&mut state, now);
        }
    }

    /// A hook event forwarded by `valk hook <agent>`. Every process in the session
    /// inherits `VALK_SESSION`, so only the session's own agent may drive it (a
    /// `codex exec` run by Claude must not); a plain shell session takes any agent.
    pub fn hook(self: &Arc<Self>, agent: &str, sent_us: u64, payload: Value) {
        let adapter = valkyrie_agents::by_name(agent);
        let ours = adapter.name() != "generic"
            && (self.adapter.name() == "generic" || self.adapter.name() == adapter.name());
        let events = adapter.normalize(&payload);
        let now = now_ms();
        let mut state = self.state.lock().unwrap();
        state.log(
            now,
            "hook",
            json!({"agent": agent, "sent_us": sent_us, "payload": payload, "events": events,
                   "ignored": !ours}),
        );
        if ours {
            state.activity.note(&payload, now);
        }
        // Only a session start names the conversation, and not mid-turn: Codex hooks
        // are global, so a `codex exec` the agent runs reports its own one-shot id.
        if ours
            && events.contains(&AgentEvent::SessionStarted)
            && state.tracker.status().state != AgentState::Working
            && let Some(id) = adapter.conversation(&payload)
        {
            state.conversation = Some(id);
        }
        // Its transcript too, by the same rule; a prompt names it as well, for an
        // agent that started before this daemon did.
        if ours
            && (events.contains(&AgentEvent::SessionStarted)
                || events.contains(&AgentEvent::PromptSubmitted))
            && state.tracker.status().state != AgentState::Working
            && let Some(path) = chat::from_hook(&payload)
            && state.chat.as_ref() != Some(&path)
        {
            state.chat = Some(path);
            self.changed.notify_one();
        }
        if ours && state.tracker.hook(&events, sent_us, now, self.watching()) {
            self.after_change(&mut state, now);
        }
    }

    /// Periodic: screen heuristics once output is quiet (glances in between), then
    /// the tracker's timers.
    pub fn tick(self: &Arc<Self>, now: u64) {
        let mut state = self.state.lock().unwrap();
        if state.exited.is_none() {
            let pid = self.pty.foreground().or(self.pid.map(|p| p as i32));
            if let Some(cwd) = pid.and_then(foreground::cwd) {
                state.live_cwd = Some(cwd);
            }
        }
        let mut changed = self.follow_foreground(&mut state, now);
        if let Some(quiet) = scan_kind(
            state.scan_due,
            now,
            state.last_output_ms,
            state.last_scan_ms,
        ) {
            // Only a quiet scan settles it: the screen a burst ends on must get one.
            state.scan_due = !quiet;
            state.last_scan_ms = now;
            let adapter = self.agent(&state);
            if adapter.name() != "generic" {
                let verdict = adapter.scan(&state.screen.unwrapped_text());
                let watching = self.watching();
                if quiet {
                    state.log(now, "scan", json!({"screen": screen_label(&verdict)}));
                    changed |= state.tracker.screen(verdict, now, watching);
                } else {
                    let label = screen_label(&verdict);
                    state.log(now, "scan", json!({"screen": label, "quiet": false}));
                    changed |= state.tracker.glance(verdict, now, watching);
                }
            }
        }
        changed |= state.tracker.tick(now, self.watching());
        if changed {
            self.after_change(&mut state, now);
        }
    }

    /// The session's agent: its own, or in a shell, the one in its foreground.
    fn agent(&self, state: &State) -> &'static dyn Adapter {
        if self.adapter.name() != "generic" {
            return self.adapter;
        }
        valkyrie_agents::by_name(&state.tracker.status().agent)
    }

    /// A shell session takes on the agent its foreground runs (`claude` typed at the
    /// prompt) and drops it when that exits. Returns whether the status changed.
    fn follow_foreground(&self, state: &mut State, now: u64) -> bool {
        if self.adapter.name() != "generic" || state.exited.is_some() {
            return false;
        }
        let group = self.pty.foreground();
        // A wrapper can exec the agent without a new group, so look again now and then.
        if group == state.fg_group
            && now.saturating_sub(state.fg_checked_ms) < FOREGROUND_RECHECK_MS
        {
            return false;
        }
        state.fg_group = group;
        state.fg_checked_ms = now;
        let found = group.and_then(foreground::agent);
        let agent = found.map_or("generic", |(name, _)| name);
        // A `claude` typed at the prompt has no hooks to name its transcript. Looked
        // at each check, since `/clear` starts another.
        if let Some(("claude", pid)) = found
            && let Some(path) = chat::claude(pid)
            && state.chat.as_ref() != Some(&path)
        {
            state.chat = Some(path);
            self.changed.notify_one();
        }
        let status = state.tracker.status();
        if status.agent == agent {
            return false;
        }
        // A fresh tracker for the new agent; a bumped seq, so clients see the change.
        let mut fresh = AgentStatus::new(agent, now);
        fresh.seq = status.seq + 1;
        let adapter = valkyrie_agents::by_name(agent);
        state.tracker = Tracker::resume(fresh, adapter.hook_gaps(), false, now);
        state.scan_due = true;
        state.log(now, "foreground", json!({"agent": agent}));
        true
    }

    /// What lists show: the session's own name, else its directory. The agent or
    /// program running is shown beside it, from the status and the command.
    fn display_name(&self, state: &State) -> String {
        if let Some(name) = self.name.lock().unwrap().clone() {
            return name;
        }
        match self.live_cwd(state).file_name() {
            Some(dir) => dir.to_string_lossy().into_owned(),
            None => default_name(&self.command),
        }
    }

    /// Where the session is now: the foreground program's directory, else where it
    /// started.
    fn live_cwd(&self, state: &State) -> PathBuf {
        state.live_cwd.clone().unwrap_or_else(|| self.cwd.clone())
    }

    /// The name saved for an upgrade or a restart: older images expect one always.
    fn saved_name(&self) -> String {
        let name = self.name.lock().unwrap().clone();
        name.unwrap_or_else(|| default_name(&self.command))
    }

    pub fn rank(&self) -> u64 {
        self.rank.load(Ordering::Relaxed)
    }

    pub fn set_rank(&self, rank: u64) {
        self.rank.store(rank, Ordering::Relaxed);
    }

    /// Exited with nothing to show for it: an interactive shell, however it ended
    /// (`exit` passes on the last command's code), or anything that succeeded. A
    /// failure (an agent that crashed, `valk new -- cargo test`) stays until closed,
    /// `blocked` in the queue.
    pub fn gone(&self) -> bool {
        match self.state.lock().unwrap().exited {
            None => false,
            Some(code) => code == Some(0) || valkyrie_agents::is_shell(&self.command),
        }
    }

    /// Gone for `grace_ms`: long enough that a reboot, which ends every program at
    /// once, would have taken the daemon down too, so the restore list can drop it.
    pub fn removable(&self, now: u64, grace_ms: u64) -> bool {
        let exited_ms = self.state.lock().unwrap().exited_ms;
        self.gone() && exited_ms.is_some_and(|at| now.saturating_sub(at) >= grace_ms)
    }

    pub fn rename(&self, name: Option<String>) {
        let name = own_name(name, &self.command);
        let now = now_ms();
        self.state
            .lock()
            .unwrap()
            .log(now, "rename", json!({"name": name}));
        *self.name.lock().unwrap() = name;
        // The queue carries names too.
        self.changed.notify_one();
    }

    /// A client attached: whatever it is showing is now seen.
    pub fn attach(self: &Arc<Self>) {
        let now = now_ms();
        let mut state = self.state.lock().unwrap();
        let clients = self.clients.fetch_add(1, Ordering::Relaxed) + 1;
        state.log(now, "attach", json!({"clients": clients}));
        if state.tracker.seen() {
            let seq = state.tracker.status().seq;
            state.log(now, "seen", json!({"seq": seq}));
            self.changed.notify_one();
        }
    }

    pub fn detach(&self) {
        let now = now_ms();
        let mut state = self.state.lock().unwrap();
        let clients = self.clients.fetch_sub(1, Ordering::Relaxed) - 1;
        state.log(now, "detach", json!({"clients": clients}));
    }

    pub fn mark_seen(self: &Arc<Self>, seq: u64) {
        let now = now_ms();
        let mut state = self.state.lock().unwrap();
        if state.tracker.mark_seen(seq) {
            state.log(now, "seen", json!({"seq": seq}));
            self.changed.notify_one();
        }
    }

    fn after_change(self: &Arc<Self>, state: &mut State, now: u64) {
        let status = state.tracker.status().clone();
        state.log(now, "state", json!({"status": status}));
        self.changed.notify_one();
        if status.state == AgentState::ReviewReady && !status.seen {
            let session = self.clone();
            std::thread::spawn(move || {
                let cwd = session.live_cwd(&session.state.lock().unwrap());
                let Some(stat) = diff_stat(&cwd) else {
                    return;
                };
                let mut state = session.state.lock().unwrap();
                if state.tracker.annotate(status.seq, &stat) {
                    let status = state.tracker.status().clone();
                    state.log(now_ms(), "state", json!({"status": status}));
                    session.changed.notify_one();
                }
            });
        }
    }

    /// Subscribe to diffs and get the snapshot they apply on top of, plus the `Exited`
    /// message if the program already ended (its broadcast predates this subscriber).
    pub fn subscribe(&self) -> (Feed, ServerMsg, Option<ServerMsg>, Vec<ServerMsg>) {
        let state = self.state.lock().unwrap();
        let rx = self.feed.subscribe();
        let snapshot = ServerMsg::Screen {
            session: self.id,
            update: state.screen.snapshot(),
        };
        let exited = state.exited.map(|code| ServerMsg::Exited {
            session: self.id,
            code,
        });
        // The images a new client's terminal has not seen: shared under the lock,
        // copied into messages after it.
        let kept = state.graphics.replay();
        drop(state);
        let graphics = kept
            .into_iter()
            .map(|cmd| ServerMsg::Graphics {
                session: self.id,
                x: cmd.x,
                y: cmd.y,
                data: cmd.data.to_string(),
            })
            .collect();
        (rx, snapshot, exited, graphics)
    }

    /// Never blocks: input is queued for the writer thread.
    pub fn write_input(&self, data: Vec<u8>) {
        self.input.push(data);
    }

    /// Input a client typed (not the terminal's own replies to queries).
    pub fn typed(&self, data: Vec<u8>) {
        self.last_input_ms.store(now_ms(), Ordering::Relaxed);
        self.write_input(data);
    }

    pub fn resize(&self, size: Size) {
        let size = size.clamped();
        let mut state = self.state.lock().unwrap();
        if state.screen.size() == size {
            return;
        }
        state.screen.resize(size);
        state.log(now_ms(), "resize", json!({"size": size}));
        if let Err(e) = self.pty.resize(size, state.screen.cell_pixels()) {
            tracing::warn!(session = self.id, "pty resize failed: {e}");
        }
        if let Some(update) = state.screen.take_diff() {
            let _ = self.feed.send(Arc::new(ServerMsg::Screen {
                session: self.id,
                update,
            }));
        }
    }

    /// SIGHUP the program's whole process group (it is a session leader, so this
    /// includes background jobs), then SIGKILL after a grace period. Returns at once.
    pub fn kill(self: &Arc<Self>) {
        let Some(pid) = self.pid else { return };
        if !self.signal_group(pid, libc::SIGHUP) {
            return;
        }
        let session = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(KILL_GRACE);
            session.signal_group(pid, libc::SIGKILL);
        });
    }

    /// Signals only while the leader is unreaped, so a recycled pgid is never hit.
    /// Returns whether a signal was sent.
    fn signal_group(&self, pid: u32, sig: libc::c_int) -> bool {
        let state = self.state.lock().unwrap();
        if state.exited.is_some() {
            return false;
        }
        // SAFETY: plain syscall; the group exists because its leader is not yet reaped.
        unsafe { libc::killpg(pid as libc::pid_t, sig) == 0 }
    }

    pub fn text(&self) -> String {
        self.state.lock().unwrap().screen.text()
    }

    /// The attached client's cell size in pixels: answers for image programs, and the
    /// PTY's pixel size.
    pub fn set_cell_pixels(&self, cell: (u16, u16)) {
        let mut state = self.state.lock().unwrap();
        if state.screen.cell_pixels() == cell {
            return;
        }
        state.screen.set_cell_pixels(cell);
        let size = state.screen.size();
        if let Err(e) = self.pty.resize(size, cell) {
            tracing::warn!(session = self.id, "pty resize failed: {e}");
        }
    }

    /// Starts the restore list off with the conversation a restored agent resumes, so
    /// it is kept even if the daemon stops before the agent's first hook.
    pub fn set_conversation(&self, id: Option<String>) {
        self.state.lock().unwrap().conversation = id;
    }

    /// The agent's own conversation id, once a hook named it.
    pub fn conversation(&self) -> Option<String> {
        self.state.lock().unwrap().conversation.clone()
    }

    pub fn activity(&self) -> crate::activity::Activity {
        self.state.lock().unwrap().activity.clone()
    }

    /// Whether `text` is news to this session's agent (see `Activity::tell`).
    pub fn tell(&self, text: &str) -> bool {
        self.state.lock().unwrap().activity.tell(text)
    }

    /// Marks this session as an agent's doing; it stays marked.
    pub fn driven_by(&self, agent: &str) {
        let mut state = self.state.lock().unwrap();
        if state.driven_by.is_none() {
            state.driven_by = Some(agent.to_owned());
        }
    }

    /// The agent behind this session: the one it runs (following the foreground),
    /// else one that started it or typed into it. `None`: a human's.
    pub fn agent_behind(&self) -> Option<String> {
        let state = self.state.lock().unwrap();
        let running = &state.tracker.status().agent;
        if running != "generic" {
            return Some(running.clone());
        }
        state.driven_by.clone()
    }

    /// The PTY's terminal device (`foreground::dev_key`), the controlling terminal of
    /// whatever runs in it.
    pub fn tty(&self) -> Option<u64> {
        use std::os::unix::fs::MetadataExt;
        let mut name = [0 as libc::c_char; 128];
        #[cfg(target_os = "linux")]
        // SAFETY: the buffer is valid for its length; ptsname_r NUL-terminates.
        let ok = unsafe { libc::ptsname_r(self.pty_fd(), name.as_mut_ptr(), name.len()) } == 0;
        #[cfg(not(target_os = "linux"))]
        // SAFETY: TIOCPTYGNAME writes at most 128 bytes, NUL-terminated.
        let ok =
            unsafe { libc::ioctl(self.pty_fd(), libc::TIOCPTYGNAME as _, name.as_mut_ptr()) } == 0;
        if !ok {
            return None;
        }
        // SAFETY: NUL-terminated above.
        let path = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) };
        let rdev = std::fs::metadata(path.to_str().ok()?).ok()?.rdev() as libc::dev_t;
        Some(crate::foreground::dev_key(
            libc::major(rdev) as u32,
            libc::minor(rdev) as u32,
        ))
    }

    /// This session's line in the restore list (DESIGN §8.3): while it runs, and for
    /// `grace_ms` after it exits. A reboot ends every program at once, and the daemon
    /// may reap some before it dies itself; an exit seen that late must not drop them.
    pub fn restore_entry(&self, now: u64, grace_ms: u64) -> Option<RestoreEntry> {
        let state = self.state.lock().unwrap();
        if state
            .exited_ms
            .is_some_and(|at| now.saturating_sub(at) >= grace_ms)
        {
            return None;
        }
        Some(RestoreEntry {
            name: self.saved_name(),
            command: self.command.clone(),
            // A shell comes back where it was, not where it started.
            cwd: self.live_cwd(&state),
            conversation: state.conversation.clone(),
        })
    }

    pub fn scrollback(&self, anchor: ScrollAnchor) -> Reply {
        let (from_top, history, rows) = self.state.lock().unwrap().screen.scrollback(anchor);
        Reply::Scrollback {
            from_top,
            history,
            rows,
        }
    }

    pub fn info(&self) -> SessionInfo {
        let state = self.state.lock().unwrap();
        SessionInfo {
            id: self.id,
            name: self.display_name(&state),
            command: self.command.clone(),
            cwd: self.live_cwd(&state),
            pid: self.pid,
            created_unix: self.created_unix,
            title: state.screen.title().map(str::to_owned),
            clients: self.clients.load(Ordering::Relaxed),
            exited: state.exited,
            status: state.tracker.status().clone(),
            last_input_ms: self.last_input_ms.load(Ordering::Relaxed),
            chat: state.chat.clone(),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.input.set(|state| state.closed = true);
    }
}

/// Whether to scan on this tick: `Some(true)` quiet scan, `Some(false)` glance.
fn scan_kind(due: bool, now: u64, last_output_ms: u64, last_scan_ms: u64) -> Option<bool> {
    if !due {
        return None;
    }
    let quiet = now.saturating_sub(last_output_ms) >= SCAN_QUIET_MS;
    (quiet || now.saturating_sub(last_scan_ms) >= SCAN_EVERY_MS).then_some(quiet)
}

fn screen_label(verdict: &Option<Screen>) -> Value {
    match verdict {
        None => Value::Null,
        Some(Screen::Busy) => "busy".into(),
        Some(Screen::Reviewing) => "reviewing".into(),
        Some(Screen::Background) => "background".into(),
        Some(Screen::Idle) => "idle".into(),
        Some(Screen::Prompt { summary }) => json!({"prompt": summary}),
    }
}

/// `uncommitted: 2 files +10 -3` for a git working tree with changes, else `None`.
fn diff_stat(cwd: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["diff", "--shortstat", "HEAD"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_shortstat(&String::from_utf8_lossy(&out.stdout))
}

/// ` 2 files changed, 10 insertions(+), 3 deletions(-)` → `uncommitted: 2 files +10 -3`.
fn parse_shortstat(text: &str) -> Option<String> {
    let mut files = None;
    let mut parts = Vec::new();
    for chunk in text.trim().split(", ") {
        let (n, what) = chunk.split_once(' ')?;
        if what.starts_with("file") {
            files = Some(format!("{n} file{}", if n == "1" { "" } else { "s" }));
        } else if what.starts_with("insertion") {
            parts.push(format!("+{n}"));
        } else if what.starts_with("deletion") {
            parts.push(format!("-{n}"));
        }
    }
    let files = files?;
    Some(
        format!("uncommitted: {files} {}", parts.join(" "))
            .trim_end()
            .to_string(),
    )
}

fn write_loop(id: SessionId, mut writer: File, queue: Arc<InputQueue>, stop: Arc<StopPipe>) {
    let mut failed = false;
    loop {
        let mut buf = {
            let mut state = queue.state.lock().unwrap();
            loop {
                if state.halt {
                    return;
                }
                if let Some(buf) = state.bufs.pop_front() {
                    break buf;
                }
                if state.closed {
                    return;
                }
                state = queue.ready.wait(state).unwrap();
            }
        };
        // After a failure keep draining, so the queue doesn't grow forever.
        let mut written = 0;
        while !failed && written < buf.len() {
            let mut fds = [
                libc::pollfd {
                    fd: writer.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                },
                libc::pollfd {
                    fd: stop.read.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: `fds` is a valid array of two pollfds.
            let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
            if rc < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if fds[1].revents != 0 {
                // Handoff: the unwritten rest goes back to the front of the queue.
                buf.drain(..written);
                queue.state.lock().unwrap().bufs.push_front(buf);
                return;
            }
            match writer.write(&buf[written..]) {
                Ok(n) => written += n,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => {
                    tracing::debug!(session = id, "pty write failed, dropping input: {e}");
                    failed = true;
                }
            }
        }
    }
}

/// Opens a transcript or event log to append, recreating it (0600) if it was deleted.
fn append_private(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

/// Reaps a program that could not be adopted, so it doesn't stay a zombie.
pub fn reap_orphan(pid: u32) {
    std::thread::spawn(move || {
        let code = wait_pid(pid);
        tracing::info!(pid, ?code, "reaped orphan");
    });
}

/// Reaps `pid` with the exit code portable-pty would report (1 when killed by a
/// signal). `None` if it is not our child any more (reaped just before the exec).
fn wait_pid(pid: u32) -> Option<i32> {
    let mut status = 0;
    loop {
        // SAFETY: waits on our own child with a valid status pointer.
        let rc = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, 0) };
        if rc == pid as libc::pid_t {
            break;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return None;
        }
    }
    if libc::WIFEXITED(status) {
        Some(libc::WEXITSTATUS(status))
    } else {
        Some(1)
    }
}

/// The screen as it stood at `upto` bytes into the transcript: replays the bytes with
/// the resizes from the event log at the offsets they happened. Replies to terminal
/// queries are discarded; the program got the real ones long ago.
/// The screen, and the images it shows, rebuilt from the transcript.
fn rebuild_screen(
    transcript: &Path,
    events: &Path,
    upto: u64,
    size: Size,
) -> Result<(VtScreen, graphics::Log)> {
    let mut start = None;
    let mut resizes = Vec::new();
    for line in BufReader::new(File::open(events)?).lines() {
        let Ok(line) = serde_json::from_str::<Value>(&line?) else {
            continue;
        };
        let at: Option<Size> = serde_json::from_value(line["size"].clone()).ok();
        match (line["k"].as_str(), at) {
            (Some("start"), Some(at)) => start = Some(at),
            (Some("resize"), Some(at)) => resizes.push((line["off"].as_u64().unwrap_or(0), at)),
            _ => {}
        }
    }
    let mut screen = VtScreen::new(start.unwrap_or(size));
    let mut graphics = graphics::Log::new(GRAPHICS_LIMIT);
    let mut raw = BufReader::new(File::open(transcript)?).take(upto);
    let mut resizes = resizes.into_iter().peekable();
    let mut pos = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        while let Some(&(off, at)) = resizes.peek()
            && off <= pos
        {
            screen.resize(at);
            resizes.next();
        }
        let room = resizes
            .peek()
            .map_or(buf.len() as u64, |&(off, _)| off - pos)
            .min(buf.len() as u64) as usize;
        let n = raw.read(&mut buf[..room])?;
        if n == 0 {
            break;
        }
        for signal in screen.feed(&buf[..n]) {
            if let Signal::Graphics(cmd) = signal {
                graphics.record(cmd);
            }
        }
        pos += n as u64;
    }
    if screen.size() != size {
        screen.resize(size);
    }
    let _ = screen.take_diff();
    Ok((screen, graphics))
}

fn pty_size(size: Size, cell: (u16, u16)) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: size.cols.saturating_mul(cell.0),
        pixel_height: size.rows.saturating_mul(cell.1),
    }
}

/// How often a shell session's foreground is searched for an agent while its group
/// stays the same (a group change is checked every tick).
const FOREGROUND_RECHECK_MS: u64 = 2000;

/// A name worth keeping: not blank, and not just the program's (what older images
/// saved for an unnamed session).
fn own_name(name: Option<String>, command: &[String]) -> Option<String> {
    let name = name?.trim().to_string();
    (!name.is_empty() && name != default_name(command)).then_some(name)
}

fn default_name(command: &[String]) -> String {
    command
        .first()
        .and_then(|p| Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_burst_ending_on_a_glance_still_gets_a_quiet_scan() {
        // Output every 100 ms until 950 ms, then silence; ticks every 250 ms. The
        // glance at 1_000 lands 50 ms after the last chunk.
        let (mut due, mut last_output, mut last_scan) = (false, 0, 0);
        let mut scans = Vec::new();
        for now in (0..3_000u64).step_by(50) {
            if now <= 950 && now % 100 == 50 {
                due = true;
                last_output = now;
            }
            if now % 250 == 0
                && let Some(quiet) = super::scan_kind(due, now, last_output, last_scan)
            {
                due = !quiet;
                last_scan = now;
                scans.push((now, quiet));
            }
        }
        // The tick at 1_250 is 300 ms after the last output (quiet); the glances
        // came before it, every 500 ms while output kept coming.
        assert_eq!(
            scans,
            vec![(500, false), (1_000, false), (1_250, true)],
            "{scans:?}"
        );
    }

    #[test]
    fn rebuilt_screen_matches_the_live_one_across_resizes() {
        use valkyrie_proto::Size;
        let dir = std::env::temp_dir().join(format!("valkyrie-rebuild-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (raw, events) = (dir.join("t.raw"), dir.join("t.events.jsonl"));
        let start = Size { cols: 20, rows: 5 };
        let wide = Size { cols: 40, rows: 8 };
        let first = b"\x1b[2J\x1b[Hline one is long enough to wrap\r\nsecond\x1b[6n";
        // Row 8 only exists after the resize: replaying it at the old size would
        // clamp it to row 5 (reflow alone would hide a missed resize).
        let second = b"\r\nafter resize \x1b[1mbold\x1b[0m\x1b[8;1Hbottom";
        let mut live = valkyrie_term::VtScreen::new(start);
        live.feed(first);
        live.resize(wide);
        live.feed(second);
        std::fs::write(&raw, [&first[..], &second[..], b"not yet"].concat()).unwrap();
        let log = [
            serde_json::json!({"k": "start", "off": 0, "size": start}),
            serde_json::json!({"k": "out", "off": 10}),
            serde_json::json!({"k": "resize", "off": first.len(), "size": wide}),
        ];
        let log: String = log.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&events, log + "garbage\n").unwrap();

        let upto = (first.len() + second.len()) as u64;
        let (rebuilt, _) = super::rebuild_screen(&raw, &events, upto, wide).unwrap();
        assert_eq!(rebuilt.text(), live.text());
        assert_eq!(rebuilt.snapshot(), live.snapshot());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shortstat() {
        let p = super::parse_shortstat;
        assert_eq!(
            p(" 2 files changed, 10 insertions(+), 3 deletions(-)\n").as_deref(),
            Some("uncommitted: 2 files +10 -3")
        );
        assert_eq!(
            p(" 1 file changed, 1 deletion(-)").as_deref(),
            Some("uncommitted: 1 file -1")
        );
        assert_eq!(p(""), None);
    }
}

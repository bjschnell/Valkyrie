//! One PTY-hosted program plus its screen model (DESIGN §13 data flow).
//!
//! Three threads per session, none of which ever runs on a tokio worker:
//! - reader: PTY output → transcript + screen → broadcast diff (ends at PTY EOF)
//! - writer: drains the input queue into the PTY; a program that stops reading stdin
//!   blocks only this thread
//! - waiter: reaps the child and publishes `Exited`, independent of PTY EOF (a
//!   background job can hold the tty open long after the agent exits)

use anyhow::{Context, Result};
use overseer_proto::{ServerMsg, SessionId, SessionInfo, Size, SpawnSpec};
use overseer_term::{Signal, VtScreen};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

/// Server pushes for one session. Lagging receivers resync from a fresh snapshot.
pub type Feed = broadcast::Receiver<Arc<ServerMsg>>;

const FEED_CAPACITY: usize = 1024;
/// How long `kill` waits after SIGHUP before escalating to SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(2);

pub struct Session {
    pub id: SessionId,
    name: String,
    command: Vec<String>,
    cwd: PathBuf,
    pid: Option<u32>,
    created_unix: u64,
    pub clients: AtomicU32,
    state: Mutex<State>,
    input: mpsc::Sender<Vec<u8>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    feed: broadcast::Sender<Arc<ServerMsg>>,
}

/// Everything shared threads mutate. Diffs and `Exited` are broadcast while this lock
/// is held, so a subscriber that snapshots under the same lock never misses or
/// repeats a message.
struct State {
    screen: VtScreen,
    exited: Option<Option<i32>>,
}

impl Session {
    pub fn spawn(id: SessionId, spec: SpawnSpec, transcript_dir: &Path) -> Result<Arc<Session>> {
        let size = spec.size.clamped();
        let program = spec.command.first().context("empty command")?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        // Opened before spawning so a failure here cannot orphan the child. Ids restart
        // with the daemon, so the nanosecond timestamp keeps transcripts distinct.
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(transcript_dir)?;
        let transcript = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(transcript_dir.join(format!("{}-{id}.raw", now.as_nanos())))?;

        let pair = native_pty_system()
            .openpty(pty_size(size))
            .context("openpty")?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(&spec.command[1..]);
        let cwd = match spec.cwd {
            Some(dir) => dir,
            None => std::env::current_dir()?,
        };
        cmd.cwd(&cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("OVERSEER_SESSION", id.to_string());

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawn {program}"))?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (input, input_rx) = mpsc::channel();

        let (feed, _) = broadcast::channel(FEED_CAPACITY);
        let session = Arc::new(Session {
            id,
            name: spec.name.unwrap_or_else(|| default_name(&spec.command)),
            command: spec.command,
            cwd,
            pid: child.process_id(),
            created_unix: now.as_secs(),
            clients: AtomicU32::new(0),
            state: Mutex::new(State {
                screen: VtScreen::new(size),
                exited: None,
            }),
            input,
            master: Mutex::new(pair.master),
            feed,
        });

        // The writer holds no Arc<Session>: it ends when the session (the last input
        // sender) is dropped, which happens after the reader sees EOF.
        std::thread::Builder::new()
            .name(format!("pty-w-{id}"))
            .spawn(move || write_loop(id, writer, input_rx))?;
        let waiter = session.clone();
        std::thread::Builder::new()
            .name(format!("pty-x-{id}"))
            .spawn(move || waiter.wait_exit(child))?;
        let reader_session = session.clone();
        std::thread::Builder::new()
            .name(format!("pty-r-{id}"))
            .spawn(move || reader_session.pump(reader, transcript))?;
        Ok(session)
    }

    fn pump(self: Arc<Self>, mut reader: Box<dyn Read + Send>, mut transcript: File) {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let bytes = &buf[..n];
            if let Err(e) = transcript.write_all(bytes) {
                tracing::warn!(session = self.id, "transcript write failed: {e}");
            }
            let mut state = self.state.lock().unwrap();
            let signals = state.screen.feed(bytes);
            if let Some(update) = state.screen.take_diff() {
                let _ = self.feed.send(Arc::new(ServerMsg::Screen {
                    session: self.id,
                    update,
                }));
            }
            drop(state);
            for signal in signals {
                if let Signal::Reply(reply) = signal {
                    self.write_input(reply);
                }
            }
        }
        tracing::debug!(session = self.id, "pty closed");
    }

    fn wait_exit(self: Arc<Self>, mut child: Box<dyn Child + Send + Sync>) {
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        tracing::info!(session = self.id, ?code, "exited");
        let mut state = self.state.lock().unwrap();
        state.exited = Some(code);
        let _ = self.feed.send(Arc::new(ServerMsg::Exited {
            session: self.id,
            code,
        }));
    }

    /// Subscribe to diffs and get the snapshot they apply on top of, plus the `Exited`
    /// message if the program already ended (its broadcast predates this subscriber).
    pub fn subscribe(&self) -> (Feed, ServerMsg, Option<ServerMsg>) {
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
        (rx, snapshot, exited)
    }

    /// Never blocks: input is queued for the writer thread.
    pub fn write_input(&self, data: Vec<u8>) {
        let _ = self.input.send(data);
    }

    pub fn resize(&self, size: Size) {
        let size = size.clamped();
        let mut state = self.state.lock().unwrap();
        if state.screen.size() == size {
            return;
        }
        state.screen.resize(size);
        if let Err(e) = self.master.lock().unwrap().resize(pty_size(size)) {
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

    pub fn info(&self) -> SessionInfo {
        let state = self.state.lock().unwrap();
        SessionInfo {
            id: self.id,
            name: self.name.clone(),
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            pid: self.pid,
            created_unix: self.created_unix,
            title: state.screen.title().map(str::to_owned),
            clients: self.clients.load(Ordering::Relaxed),
            exited: state.exited,
        }
    }
}

fn write_loop(id: SessionId, mut writer: Box<dyn Write + Send>, input: mpsc::Receiver<Vec<u8>>) {
    for data in input {
        if let Err(e) = writer.write_all(&data).and_then(|_| writer.flush()) {
            tracing::debug!(session = id, "pty write failed, dropping input: {e}");
            return;
        }
    }
}

fn pty_size(size: Size) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn default_name(command: &[String]) -> String {
    command
        .first()
        .and_then(|p| Path::new(p).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "session".into())
}

//! One PTY-hosted program plus its screen model and agent state (DESIGN §13–14).
//!
//! Three threads per session, none of which ever runs on a tokio worker:
//! - reader: PTY output → transcript + screen → broadcast diff (ends at PTY EOF)
//! - writer: drains the input queue into the PTY; a program that stops reading stdin
//!   blocks only this thread
//! - waiter: reaps the child and publishes `Exited`, independent of PTY EOF (a
//!   background job can hold the tty open long after the agent exits)
//!
//! Agent state (`Tracker`) lives under the same lock as the screen. Every input to it
//! (hooks, output marks, screen verdicts, bells, attaches, exit) and every state it
//! reaches goes to `<ns>-<id>.events.jsonl` next to the transcript, so a recorded
//! session replays to the same states (DESIGN §14.6).

use anyhow::{Context, Result};
use overseer_agents::{Adapter, AgentEvent, Screen, Tracker};
use overseer_proto::{AgentState, ServerMsg, SessionId, SessionInfo, Size, SpawnSpec};
use overseer_term::{Signal, VtScreen};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Notify, broadcast};

/// Server pushes for one session. Lagging receivers resync from a fresh snapshot.
pub type Feed = broadcast::Receiver<Arc<ServerMsg>>;

const FEED_CAPACITY: usize = 1024;
/// How long `kill` waits after SIGHUP before escalating to SIGKILL.
const KILL_GRACE: Duration = Duration::from_secs(2);
/// Screen heuristics run once output has been quiet this long, never per chunk.
const SCAN_QUIET_MS: u64 = 150;
/// At most one output offset mark per this interval in the event log.
const MARK_EVERY_MS: u64 = 250;

/// What every session needs from the daemon.
pub struct Host {
    pub transcript_dir: PathBuf,
    /// The overseer binary that agent hooks run (`overseer hook <agent>`).
    pub hook_exe: PathBuf,
    /// Exported to sessions so their hooks reach this daemon.
    pub socket: PathBuf,
    /// Poked whenever any session's agent status changes.
    pub changed: Arc<Notify>,
}

pub struct Session {
    pub id: SessionId,
    name: String,
    command: Vec<String>,
    cwd: PathBuf,
    pid: Option<u32>,
    created_unix: u64,
    pub clients: AtomicU32,
    adapter: &'static dyn Adapter,
    state: Mutex<State>,
    input: mpsc::Sender<Vec<u8>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    feed: broadcast::Sender<Arc<ServerMsg>>,
    changed: Arc<Notify>,
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
    scan_due: bool,
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
        let adapter = overseer_agents::adapter_for(&spec.command);

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

        let pair = native_pty_system()
            .openpty(pty_size(size))
            .context("openpty")?;
        let mut argv = spec.command.clone();
        adapter.prepare(&mut argv, &host.hook_exe);
        let mut cmd = CommandBuilder::new(&program);
        cmd.args(&argv[1..]);
        let cwd = match spec.cwd {
            Some(dir) => dir,
            None => std::env::current_dir()?,
        };
        cmd.cwd(&cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("OVERSEER_SESSION", id.to_string());
        cmd.env("OVERSEER_SOCKET", &host.socket);

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawn {program}"))?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (input, input_rx) = mpsc::channel();

        let now_ms = now.as_millis() as u64;
        let mut state = State {
            screen: VtScreen::new(size),
            exited: None,
            tracker: Tracker::new(adapter.name(), adapter.hook_gaps(), now_ms),
            log,
            offset: 0,
            last_output_ms: now_ms,
            last_mark_ms: 0,
            scan_due: false,
        };
        state.log(
            now_ms,
            "start",
            json!({"agent": adapter.name(), "command": spec.command, "size": size}),
        );

        let (feed, _) = broadcast::channel(FEED_CAPACITY);
        let session = Arc::new(Session {
            id,
            name: spec.name.unwrap_or_else(|| default_name(&spec.command)),
            command: spec.command,
            cwd,
            pid: child.process_id(),
            created_unix: now.as_secs(),
            clients: AtomicU32::new(0),
            adapter,
            state: Mutex::new(state),
            input,
            master: Mutex::new(pair.master),
            feed,
            changed: host.changed.clone(),
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

    fn watching(&self) -> bool {
        self.clients.load(Ordering::Relaxed) > 0
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

    fn wait_exit(self: Arc<Self>, mut child: Box<dyn Child + Send + Sync>) {
        let code = child.wait().ok().map(|s| s.exit_code() as i32);
        tracing::info!(session = self.id, ?code, "exited");
        let now = now_ms();
        let mut state = self.state.lock().unwrap();
        state.exited = Some(code);
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

    /// A hook event forwarded by `overseer hook <agent>`. Every process in the session
    /// inherits `OVERSEER_SESSION`, so only the session's own agent may drive it (a
    /// `codex exec` run by Claude must not); a plain shell session takes any agent.
    pub fn hook(self: &Arc<Self>, agent: &str, sent_us: u64, payload: Value) {
        let adapter = overseer_agents::by_name(agent);
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
        if ours && state.tracker.hook(&events, sent_us, now, self.watching()) {
            self.after_change(&mut state, now);
        }
    }

    /// Periodic: screen heuristics once output is quiet, then the tracker's timers.
    pub fn tick(self: &Arc<Self>, now: u64) {
        let mut state = self.state.lock().unwrap();
        let mut changed = false;
        if state.scan_due && now.saturating_sub(state.last_output_ms) >= SCAN_QUIET_MS {
            state.scan_due = false;
            if self.adapter.name() != "generic" {
                let verdict = self.adapter.scan(&state.screen.unwrapped_text());
                state.log(now, "scan", json!({"screen": screen_label(&verdict)}));
                changed |= state.tracker.screen(verdict, now, self.watching());
            }
        }
        changed |= state.tracker.tick(now, self.watching());
        if changed {
            self.after_change(&mut state, now);
        }
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
                let Some(stat) = diff_stat(&session.cwd) else {
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
        state.log(now_ms(), "resize", json!({"size": size}));
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
            status: state.tracker.status().clone(),
        }
    }
}

fn screen_label(verdict: &Option<Screen>) -> Value {
    match verdict {
        None => Value::Null,
        Some(Screen::Busy) => "busy".into(),
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

#[cfg(test)]
mod tests {
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

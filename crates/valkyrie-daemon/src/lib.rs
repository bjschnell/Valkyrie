//! The daemon: the only stateful component (DESIGN §4). Hosts sessions and serves the
//! protocol over a unix socket.

mod session;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use session::{Host, SavedSession, Session, StopPipe};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::io::{Read, Seek, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Notify, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use valkyrie_proto::codec::{read_frame, write_frame};
use valkyrie_proto::{ClientMsg, QueueItem, Reply, ReqId, ServerMsg, SessionId};

/// Format of the state handed across an upgrade exec (ADR-0006). An upgrade is only
/// attempted when the new binary reports the same number.
pub const HANDOFF_VERSION: u32 = 1;
/// How long the new binary gets to answer `daemon --handoff-check`.
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(5);
/// Before freezing, connections already accepted get this long to have their frames
/// handled (a hook writes one frame right after connecting, then closes). Newer ones
/// wait in the listen backlog for the next image.
const ACCEPTED_GRACE: Duration = Duration::from_millis(50);
/// How long a handoff waits for killed sessions to die after SIGKILL.
const KILL_WAIT: Duration = Duration::from_millis(500);

/// Everything the next daemon image needs, written to a memfd it inherits.
#[derive(Serialize, Deserialize)]
struct Handoff {
    version: u32,
    generation: u32,
    /// Identifies this daemon across handoffs (`Hello::boot`).
    boot: u64,
    next_id: u32,
    socket: PathBuf,
    listener: RawFd,
    sessions: Vec<SavedSession>,
    /// Killed programs not reaped yet; the next image reaps them.
    orphans: Vec<u32>,
}

/// Run by the new binary on `daemon --handoff-check` with a sample handoff on stdin:
/// it must parse, or the upgrade is refused before anything is frozen.
pub fn check_handoff(json: &[u8]) -> Result<()> {
    let handoff: Handoff = serde_json::from_slice(json).context("parse handoff")?;
    anyhow::ensure!(
        handoff.version == HANDOFF_VERSION,
        "handoff format {} is not {HANDOFF_VERSION}",
        handoff.version
    );
    Ok(())
}

/// An accepted `Upgrade`, carried out by the accept loop so nothing is accepted
/// meanwhile. `done` only ever receives the error of an exec that failed.
struct UpgradeRequest {
    exe: PathBuf,
    done: oneshot::Sender<Result<()>>,
}

type Queue = Arc<Vec<QueueItem>>;

struct Registry {
    sessions: Mutex<BTreeMap<SessionId, Arc<Session>>>,
    next_id: AtomicU32,
    host: Host,
    /// The current ranked queue; recomputed when `host.changed` is poked.
    queue: watch::Sender<Queue>,
    /// Upgrade handoffs these sessions have been through.
    generation: u32,
    /// When the first image of this daemon started (ns); survives handoffs.
    boot: u64,
    /// Set while a handoff is under way: no new sessions.
    frozen: AtomicBool,
    /// Spawns hold it shared from the `frozen` check to the insert; a handoff takes
    /// it exclusively, so no child is forked that the snapshot would miss.
    gate: std::sync::RwLock<()>,
    /// Killed sessions, until reaped: a handoff must not strand them. Weak, so a
    /// reaped session still drops (and frees its PTY and threads) as before.
    dying: Mutex<Vec<std::sync::Weak<Session>>>,
    upgrades: mpsc::Sender<UpgradeRequest>,
}

impl Registry {
    fn all(&self) -> Vec<Arc<Session>> {
        self.sessions.lock().unwrap().values().cloned().collect()
    }

    fn compute_queue(&self) -> Vec<QueueItem> {
        let mut items: Vec<QueueItem> = self
            .all()
            .iter()
            .map(|s| s.info())
            .filter(|info| valkyrie_agents::queued(&info.status))
            .map(|info| QueueItem {
                session: info.id,
                name: info.name,
                cwd: info.cwd,
                status: info.status,
            })
            .collect();
        valkyrie_agents::sort_queue(&mut items);
        items
    }

    fn get(&self, id: SessionId) -> Result<Arc<Session>> {
        self.sessions
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .with_context(|| format!("no session {id}"))
    }
}

/// Per-connection queue of outgoing frames. When a slow client fills it, its screen
/// stream is dropped and resynced from a fresh snapshot instead of growing memory.
const OUT_CAPACITY: usize = 256;

type Out = mpsc::Sender<Arc<ServerMsg>>;

/// How often sessions run screen heuristics and state timers.
const TICK: Duration = Duration::from_millis(250);

/// Serve forever. Transcripts go to `<state_dir>/sessions/<ns>-<id>.raw`, event logs
/// next to them. Agent hooks registered by adapters run `hook_exe`.
pub async fn run(socket: &Path, state_dir: &Path, hook_exe: &Path) -> Result<()> {
    let socket = &std::path::absolute(socket)?;
    let dir = socket.parent().context("socket path has no parent")?;
    valkyrie_proto::ensure_private_dir(dir)?;
    if socket.exists() {
        if UnixStream::connect(socket).await.is_ok() {
            anyhow::bail!("a daemon is already listening on {}", socket.display());
        }
        std::fs::remove_file(socket)?;
    }
    let listener =
        UnixListener::bind(socket).with_context(|| format!("bind {}", socket.display()))?;
    tracing::info!("listening on {}", socket.display());
    let boot = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let (registry, upgrades) = new_registry(socket.clone(), state_dir, hook_exe, 1, 0, boot)?;
    serve_forever(listener, registry, upgrades).await
}

/// The new image after an upgrade exec: adopt the listener and every session from
/// the handoff memfd `fd`, then serve as usual (ADR-0006).
pub async fn resume(fd: RawFd, state_dir: &Path, hook_exe: &Path) -> Result<()> {
    // SAFETY: the previous image passed this memfd to us and nothing else owns it.
    let mut memfd = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    let mut json = Vec::new();
    memfd.read_to_end(&mut json)?;
    check_handoff(&json)?;
    let handoff: Handoff = serde_json::from_slice(&json)?;
    // Before any thread runs: nothing we start may inherit these.
    for saved in &handoff.sessions {
        set_cloexec(saved.fd, true);
    }
    // SAFETY: as above, for the listening socket.
    let listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(handoff.listener) };
    set_cloexec(listener.as_raw_fd(), true);
    listener.set_nonblocking(true)?;
    let listener = UnixListener::from_std(listener)?;
    let generation = handoff.generation + 1;
    let (registry, upgrades) = new_registry(
        handoff.socket.clone(),
        state_dir,
        hook_exe,
        handoff.next_id,
        generation,
        handoff.boot,
    )?;
    for pid in handoff.orphans {
        session::reap_orphan(pid);
    }
    let total = handoff.sessions.len();
    for saved in handoff.sessions {
        let (id, pid) = (saved.id, saved.pid);
        match Session::adopt(saved, &registry.host, generation) {
            Ok(session) => {
                registry.sessions.lock().unwrap().insert(id, session);
            }
            Err(e) => {
                tracing::error!(session = id, "could not adopt session: {e:#}");
                if let Some(pid) = pid {
                    session::reap_orphan(pid);
                }
            }
        }
    }
    let kept = registry.sessions.lock().unwrap().len();
    tracing::info!(
        generation,
        "resumed on {} with {kept}/{total} sessions",
        handoff.socket.display()
    );
    registry.host.changed.notify_one();
    serve_forever(listener, registry, upgrades).await
}

fn new_registry(
    socket: PathBuf,
    state_dir: &Path,
    hook_exe: &Path,
    next_id: u32,
    generation: u32,
    boot: u64,
) -> Result<(Arc<Registry>, mpsc::Receiver<UpgradeRequest>)> {
    let (upgrades, rx) = mpsc::channel(1);
    let registry = Arc::new(Registry {
        sessions: Mutex::default(),
        next_id: AtomicU32::new(next_id),
        host: Host {
            transcript_dir: state_dir.join("sessions"),
            hook_exe: hook_exe.to_path_buf(),
            socket,
            changed: Arc::new(Notify::new()),
            stop: Mutex::new(Arc::new(StopPipe::new()?)),
        },
        queue: watch::Sender::new(Arc::default()),
        generation,
        boot,
        frozen: AtomicBool::new(false),
        gate: std::sync::RwLock::new(()),
        dying: Mutex::default(),
        upgrades,
    });
    Ok((registry, rx))
}

async fn serve_forever(
    listener: UnixListener,
    registry: Arc<Registry>,
    mut upgrades: mpsc::Receiver<UpgradeRequest>,
) -> Result<()> {
    tokio::spawn(publish_queue(registry.clone()));
    tokio::spawn(tick(registry.clone()));
    loop {
        let stream = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(e) => {
                    // Usually fd exhaustion; exiting would SIGHUP every hosted agent.
                    tracing::warn!("accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            // Handled here, so nothing is accepted while it runs: connections wait in
            // the listen backlog for whichever image comes out of it.
            Some(request) = upgrades.recv() => {
                tokio::time::sleep(ACCEPTED_GRACE).await;
                let error = hand_off(&registry, &listener, &request.exe);
                tracing::error!("upgrade failed, carrying on: {error:#}");
                let _ = request.done.send(Err(error));
                continue;
            }
        };
        let registry = registry.clone();
        tokio::spawn(async move {
            if let Err(e) = serve(stream, registry).await {
                tracing::debug!("client error: {e:#}");
            }
        });
    }
}

/// Freezes every session, saves it to a memfd and execs `exe` with the PTYs and the
/// listener inherited (ADR-0006). Only returns if that failed, after unfreezing.
fn hand_off(registry: &Registry, listener: &UnixListener, exe: &Path) -> anyhow::Error {
    registry.frozen.store(true, Ordering::SeqCst);
    let gate = registry.gate.write().unwrap();
    // Held until the exec, so no session is added or removed under the snapshot.
    let sessions_guard = registry.sessions.lock().unwrap();
    let sessions: Vec<Arc<Session>> = sessions_guard.values().cloned().collect();
    let orphans = finish_kills(registry);
    let stop = registry.host.stop.lock().unwrap().clone();
    stop.set();
    for session in &sessions {
        session.stop_io();
    }
    let error = match exec_successor(registry, listener, exe, &sessions, orphans) {
        Err(e) => e,
        Ok(never) => match never {},
    };
    // Still here: the exec failed. Restart I/O on a fresh stop pipe.
    match StopPipe::new() {
        Ok(stop) => {
            let stop = Arc::new(stop);
            *registry.host.stop.lock().unwrap() = stop.clone();
            for session in &sessions {
                if let Err(e) = session.resume_io(stop.clone()) {
                    tracing::error!(session = session.id, "I/O not restarted: {e:#}");
                }
            }
        }
        Err(e) => tracing::error!("I/O not restarted: {e}"),
    }
    drop(sessions_guard);
    drop(gate);
    registry.frozen.store(false, Ordering::SeqCst);
    error
}

/// Killed sessions get their SIGKILL now instead of after the grace period (that
/// timer would die with this image). Returns the pids still unreaped after a short
/// wait, for the next image to reap.
fn finish_kills(registry: &Registry) -> Vec<u32> {
    let dying: Vec<Arc<Session>> = registry
        .dying
        .lock()
        .unwrap()
        .drain(..)
        .filter_map(|s| s.upgrade())
        .filter(|s| !s.exited())
        .collect();
    for session in &dying {
        session.force_kill();
    }
    let deadline = Instant::now() + KILL_WAIT;
    while dying.iter().any(|s| !s.exited()) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    dying
        .iter()
        .filter(|s| !s.exited())
        .filter_map(|s| s.pid())
        .collect()
}

fn exec_successor(
    registry: &Registry,
    listener: &UnixListener,
    exe: &Path,
    sessions: &[Arc<Session>],
    orphans: Vec<u32>,
) -> Result<std::convert::Infallible> {
    let handoff = snapshot(registry, listener.as_raw_fd(), sessions, orphans);
    let name = CString::new("valkyrie-handoff")?;
    // No MFD_CLOEXEC: the next image reads it.
    // SAFETY: valid NUL-terminated name.
    let fd = unsafe { libc::memfd_create(name.as_ptr(), 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("memfd_create");
    }
    // SAFETY: memfd_create just returned it.
    let mut memfd = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    memfd.write_all(&serde_json::to_vec(&handoff)?)?;
    memfd.rewind()?;

    let arg = |s: &std::ffi::OsStr| CString::new(s.as_bytes());
    let argv = [
        arg(exe.as_os_str())?,
        CString::new("--socket")?,
        arg(registry.host.socket.as_os_str())?,
        CString::new("daemon")?,
        CString::new("--resume-fd")?,
        CString::new(fd.to_string())?,
    ];
    let mut ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    // Last, with nothing fallible left before the exec.
    let mut inherit: Vec<RawFd> = sessions.iter().map(|s| s.pty_fd()).collect();
    inherit.push(listener.as_raw_fd());
    for &fd in &inherit {
        set_cloexec(fd, false);
    }
    tracing::info!(
        generation = registry.generation + 1,
        sessions = sessions.len(),
        "handing off to {}",
        exe.display()
    );
    // SAFETY: argv is a NULL-terminated array of valid C strings that outlive the call.
    unsafe { libc::execv(argv[0].as_ptr(), ptrs.as_ptr()) };
    let error = std::io::Error::last_os_error();
    for &fd in &inherit {
        set_cloexec(fd, true);
    }
    Err(error).with_context(|| format!("exec {}", exe.display()))
}

fn snapshot(
    registry: &Registry,
    listener: RawFd,
    sessions: &[Arc<Session>],
    orphans: Vec<u32>,
) -> Handoff {
    Handoff {
        version: HANDOFF_VERSION,
        generation: registry.generation,
        boot: registry.boot,
        next_id: registry.next_id.load(Ordering::SeqCst),
        socket: registry.host.socket.clone(),
        listener,
        sessions: sessions.iter().map(|s| s.save()).collect(),
        orphans,
    }
}

fn set_cloexec(fd: RawFd, on: bool) {
    let flag = if on { libc::FD_CLOEXEC } else { 0 };
    // SAFETY: plain fcntl on an fd this process owns.
    unsafe { libc::fcntl(fd, libc::F_SETFD, flag) };
}

/// Checks `exe` before anything is frozen: it must be an absolute path that runs and
/// parses a handoff of the live sessions (not just claim the same format number).
async fn preflight(registry: &Registry, exe: &Path) -> Result<()> {
    anyhow::ensure!(
        exe.is_absolute(),
        "{} is not an absolute path",
        exe.display()
    );
    let mut sample = snapshot(registry, -1, &registry.all(), Vec::new());
    // Only the format matters here; queued keystrokes (maybe a pasted secret) stay put.
    for saved in &mut sample.sessions {
        saved.input.clear();
    }
    let sample = serde_json::to_vec(&sample)?;
    let mut child = tokio::process::Command::new(exe)
        .args(["daemon", "--handoff-check"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("run {}", exe.display()))?;
    let mut stdin = child.stdin.take().context("probe stdin")?;
    let probe = async move {
        use tokio::io::AsyncWriteExt;
        // A binary that ignores stdin may close it early; its answer still decides.
        let _ = stdin.write_all(&sample).await;
        drop(stdin);
        child.wait_with_output().await
    };
    let out = tokio::time::timeout(PREFLIGHT_TIMEOUT, probe)
        .await
        .context("new binary did not answer --handoff-check")?
        .with_context(|| format!("run {}", exe.display()))?;
    let version = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    anyhow::ensure!(
        out.status.success() && version == HANDOFF_VERSION.to_string(),
        "{} speaks handoff format {:?}, this daemon {HANDOFF_VERSION}; restart the daemon instead (this ends its sessions)",
        exe.display(),
        if version.is_empty() { "none" } else { &version }
    );
    Ok(())
}

async fn upgrade(registry: &Registry, exe: PathBuf) -> Result<Reply> {
    preflight(registry, &exe).await?;
    let (done, result) = oneshot::channel();
    registry
        .upgrades
        .send(UpgradeRequest { exe, done })
        .await
        .map_err(|_| anyhow::anyhow!("daemon is shutting down"))?;
    // On success the process is replaced before this resolves; the client sees EOF.
    result.await?.map(|()| Reply::Done)
}

async fn publish_queue(registry: Arc<Registry>) {
    loop {
        registry.host.changed.notified().await;
        let items = registry.compute_queue();
        registry.queue.send_if_modified(|queue| {
            let changed = **queue != items;
            if changed {
                *queue = Arc::new(items);
            }
            changed
        });
    }
}

async fn tick(registry: Arc<Registry>) {
    let mut interval = tokio::time::interval(TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        let now = session::now_ms();
        for session in registry.all() {
            session.tick(now);
        }
    }
}

struct Attachment {
    session: Arc<Session>,
    task: JoinHandle<()>,
}

impl Attachment {
    /// Stops forwarding and waits until the task is gone, so none of its frames can
    /// land after whatever this connection sends next.
    async fn end(self) {
        self.task.abort();
        let _ = self.task.await;
        self.session.detach();
    }
}

async fn serve(stream: UnixStream, registry: Arc<Registry>) -> Result<()> {
    let (mut rd, mut wr) = stream.into_split();
    let (out, mut out_rx) = mpsc::channel::<Arc<ServerMsg>>(OUT_CAPACITY);
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if write_frame(&mut wr, &*msg).await.is_err() {
                break;
            }
        }
    });

    let mut attachment: Option<Attachment> = None;
    let mut queue_watch: Option<JoinHandle<()>> = None;
    // Cleanup below must run however the connection ends: a client that dies with
    // unread frames shows up as a read error (ECONNRESET), not EOF.
    let ended = loop {
        let msg = match read_frame::<_, ClientMsg>(&mut rd).await {
            Ok(Some(msg)) => msg,
            Ok(None) => break Ok(()),
            Err(e) => break Err(e),
        };
        let (req, result) = match msg {
            ClientMsg::Hello { req } => (
                req,
                Ok(Reply::Hello {
                    protocol: valkyrie_proto::PROTOCOL,
                    generation: registry.generation,
                    boot: registry.boot,
                }),
            ),
            ClientMsg::Input { session, data } => {
                if let Ok(s) = registry.get(session) {
                    s.write_input(data);
                }
                continue;
            }
            ClientMsg::Resize { session, size } => {
                if let Ok(s) = registry.get(session) {
                    s.resize(size);
                }
                continue;
            }
            ClientMsg::Hook {
                session,
                agent,
                sent_us,
                payload,
            } => {
                match registry.get(session) {
                    Ok(s) => s.hook(&agent, sent_us, payload),
                    Err(_) => tracing::debug!(session, "hook for unknown session"),
                }
                continue;
            }
            ClientMsg::WatchQueue { req } => {
                reply(&out, req, Ok(Reply::Done)).await;
                if queue_watch.is_none() {
                    let rx = registry.queue.subscribe();
                    queue_watch = Some(tokio::spawn(forward_queue(rx, out.clone())));
                }
                continue;
            }
            ClientMsg::MarkSeen { req, session, seq } => (
                req,
                registry.get(session).map(|s| {
                    s.mark_seen(seq);
                    Reply::Done
                }),
            ),
            ClientMsg::Spawn { req, spec } => (req, spawn(&registry, spec)),
            ClientMsg::Upgrade { req, exe } => (req, upgrade(&registry, exe).await),
            ClientMsg::List { req } => {
                let sessions = registry
                    .sessions
                    .lock()
                    .unwrap()
                    .values()
                    .map(|s| s.info())
                    .collect();
                (req, Ok(Reply::Sessions { sessions }))
            }
            ClientMsg::Kill { req, session } => {
                // Moved to `dying` under the sessions lock, so a handoff sees the
                // session in one list or the other.
                let removed = {
                    let mut sessions = registry.sessions.lock().unwrap();
                    let removed = sessions.remove(&session);
                    if let Some(s) = &removed
                        && !s.exited()
                    {
                        let mut dying = registry.dying.lock().unwrap();
                        dying.retain(|d| d.upgrade().is_some_and(|d| !d.exited()));
                        dying.push(Arc::downgrade(s));
                    }
                    removed
                };
                let result = match removed {
                    Some(s) => {
                        s.kill();
                        registry.host.changed.notify_one();
                        Ok(Reply::Done)
                    }
                    None => Err(anyhow::anyhow!("no session {session}")),
                };
                (req, result)
            }
            ClientMsg::Dump { req, session } => (
                req,
                registry
                    .get(session)
                    .map(|s| Reply::Text { text: s.text() }),
            ),
            ClientMsg::Detach { req } => {
                if let Some(a) = attachment.take() {
                    a.end().await;
                }
                (req, Ok(Reply::Done))
            }
            ClientMsg::Attach { req, session, size } => match registry.get(session) {
                Ok(s) => {
                    if let Some(a) = attachment.take() {
                        a.end().await;
                    }
                    s.resize(size);
                    s.attach();
                    reply(&out, req, Ok(Reply::Done)).await;
                    let task = tokio::spawn(forward(s.clone(), out.clone()));
                    attachment = Some(Attachment { session: s, task });
                    continue;
                }
                Err(e) => (req, Err(e)),
            },
        };
        reply(&out, req, result).await;
    };
    if let Some(a) = attachment {
        a.end().await;
    }
    if let Some(task) = queue_watch {
        task.abort();
    }
    writer.abort();
    ended
}

async fn reply(out: &Out, req: ReqId, result: Result<Reply>) {
    let msg = match result {
        Ok(reply) => ServerMsg::Ok { req, reply },
        Err(e) => ServerMsg::Err {
            req,
            message: format!("{e:#}"),
        },
    };
    let _ = out.send(Arc::new(msg)).await;
}

fn spawn(registry: &Registry, spec: valkyrie_proto::SpawnSpec) -> Result<Reply> {
    let _gate = registry.gate.read().unwrap();
    anyhow::ensure!(
        !registry.frozen.load(Ordering::SeqCst),
        "the daemon is upgrading; try again in a moment"
    );
    let id = registry.next_id.fetch_add(1, Ordering::Relaxed);
    let session = Session::spawn(id, spec, &registry.host)?;
    let info = session.info();
    registry.sessions.lock().unwrap().insert(id, session);
    registry.host.changed.notify_one();
    tracing::info!(session = id, command = ?info.command, "spawned");
    Ok(Reply::Session { info })
}

/// Pushes the queue to one connection: the current one, then every change. Only the
/// latest queue matters, so a slow client simply skips intermediate ones.
async fn forward_queue(mut rx: watch::Receiver<Queue>, out: Out) {
    loop {
        let items = (**rx.borrow_and_update()).clone();
        if out
            .send(Arc::new(ServerMsg::Queue { items }))
            .await
            .is_err()
        {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

/// Streams one session to one connection: a snapshot (plus `Exited` if already
/// over), then diffs. Falling behind, on the broadcast or on this connection's own
/// queue, restarts from a fresh snapshot.
async fn forward(session: Arc<Session>, out: Out) {
    'resync: loop {
        let (mut feed, snapshot, exited) = session.subscribe();
        // Waiting for room here is what makes the dropped diffs safe to skip.
        for msg in std::iter::once(snapshot).chain(exited) {
            if out.send(Arc::new(msg)).await.is_err() {
                return;
            }
        }
        loop {
            let msg = match feed.recv().await {
                Ok(msg) => msg,
                Err(RecvError::Lagged(_)) => continue 'resync,
                Err(RecvError::Closed) => return,
            };
            match out.try_send(msg) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => continue 'resync,
                Err(mpsc::error::TrySendError::Closed(_)) => return,
            }
        }
    }
}

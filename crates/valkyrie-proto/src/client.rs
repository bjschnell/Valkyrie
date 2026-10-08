//! Async daemon client: request/response matching plus a stream of server pushes.

use crate::codec::{read_frame, write_frame};
use crate::{ClientMsg, Reply, ReqId, ServerMsg, SessionId, SessionInfo, Size, SpawnSpec};
use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};

type Pending = Arc<Mutex<HashMap<ReqId, oneshot::Sender<ServerMsg>>>>;

#[derive(Clone)]
pub struct Client {
    out: mpsc::UnboundedSender<ClientMsg>,
    pending: Pending,
    closed: Arc<AtomicBool>,
    next: Arc<AtomicU64>,
}

/// Screen and image pushes held for a consumer that isn't reading (a TUI whose
/// terminal stalled, e.g. over SSH from a sleeping laptop). Past this, newer ones are
/// dropped and `Pushes::take_lagged` says to fetch a fresh snapshot instead. The
/// connection keeps being read either way, so replies still arrive.
const SCREEN_BACKLOG: usize = 1024;

/// Server-pushed messages (`Screen`, `Exited`, `Queue`). Closes when the connection drops.
pub struct Pushes {
    rx: mpsc::UnboundedReceiver<ServerMsg>,
    lag: Arc<Lag>,
}

#[derive(Default)]
struct Lag {
    /// Screen and image pushes sent but not yet received.
    held: AtomicUsize,
    dropped: AtomicBool,
}

/// Pushes that only make sense in order on top of the screen before them.
fn is_screen(msg: &ServerMsg) -> bool {
    matches!(msg, ServerMsg::Screen { .. } | ServerMsg::Graphics { .. })
}

impl Pushes {
    pub async fn recv(&mut self) -> Option<ServerMsg> {
        let msg = self.rx.recv().await?;
        Some(self.received(msg))
    }

    pub fn try_recv(&mut self) -> Result<ServerMsg, mpsc::error::TryRecvError> {
        self.rx.try_recv().map(|msg| self.received(msg))
    }

    /// Whether screen pushes were dropped since the last call: the attached screen is
    /// stale until re-attached (which sends a fresh snapshot).
    pub fn take_lagged(&self) -> bool {
        self.lag.dropped.swap(false, Ordering::SeqCst)
    }

    fn received(&self, msg: ServerMsg) -> ServerMsg {
        if is_screen(&msg) {
            self.lag.held.fetch_sub(1, Ordering::SeqCst);
        }
        msg
    }
}

/// What `Hello` tells about the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DaemonInfo {
    pub protocol: u32,
    /// Upgrade handoffs so far (ADR-0006).
    pub generation: u32,
    /// Same across handoffs, different after a restart.
    pub boot: u64,
}

impl Client {
    pub async fn connect(path: &Path) -> Result<(Client, Pushes)> {
        if let Some(dir) = path.parent() {
            crate::ensure_private_dir(dir)?;
        }
        let stream = UnixStream::connect(path).await?;
        let (mut rd, mut wr) = stream.into_split();
        let (out, mut out_rx) = mpsc::unbounded_channel::<ClientMsg>();
        let (push_tx, push_rx) = mpsc::unbounded_channel();
        let lag = Arc::new(Lag::default());
        let reader_lag = lag.clone();
        let pending: Pending = Arc::default();

        tokio::spawn(async move {
            while let Some(msg) = out_rx.recv().await {
                if write_frame(&mut wr, &msg).await.is_err() {
                    break;
                }
            }
        });

        let closed = Arc::new(AtomicBool::new(false));
        let reader_pending = pending.clone();
        let reader_closed = closed.clone();
        tokio::spawn(async move {
            while let Ok(Some(msg)) = read_frame::<_, ServerMsg>(&mut rd).await {
                let req = match &msg {
                    ServerMsg::Ok { req, .. } | ServerMsg::Err { req, .. } => Some(*req),
                    _ => None,
                };
                match req {
                    Some(req) => {
                        if let Some(tx) = reader_pending.lock().unwrap().remove(&req) {
                            let _ = tx.send(msg);
                        }
                    }
                    None if is_screen(&msg) => {
                        // Counted before sending, so the receiver never counts it first.
                        if reader_lag.held.fetch_add(1, Ordering::SeqCst) >= SCREEN_BACKLOG
                            || push_tx.send(msg).is_err()
                        {
                            reader_lag.held.fetch_sub(1, Ordering::SeqCst);
                            reader_lag.dropped.store(true, Ordering::SeqCst);
                        }
                    }
                    // Nobody listening for pushes must not stop replies from arriving.
                    None => {
                        let _ = push_tx.send(msg);
                    }
                }
            }
            // Dropping the pending senders wakes every waiter with an error.
            let mut pending = reader_pending.lock().unwrap();
            reader_closed.store(true, Ordering::SeqCst);
            pending.clear();
        });

        Ok((
            Client {
                out,
                pending,
                closed,
                next: Arc::new(AtomicU64::new(1)),
            },
            Pushes { rx: push_rx, lag },
        ))
    }

    async fn request(&self, build: impl FnOnce(ReqId) -> ClientMsg) -> Result<Reply> {
        let req = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            // Checked under the lock the reader clears on exit, so no waiter is stranded.
            let mut pending = self.pending.lock().unwrap();
            if self.closed.load(Ordering::SeqCst) {
                bail!("daemon connection closed");
            }
            pending.insert(req, tx);
        }
        self.out
            .send(build(req))
            .map_err(|_| anyhow!("daemon connection closed"))?;
        match rx.await.map_err(|_| anyhow!("daemon connection closed"))? {
            ServerMsg::Ok { reply, .. } => Ok(reply),
            ServerMsg::Err { message, .. } => bail!(message),
            other => bail!("unexpected reply: {other:?}"),
        }
    }

    /// The daemon's protocol version.
    pub async fn hello(&self) -> Result<u32> {
        self.hello_info().await.map(|info| info.protocol)
    }

    pub async fn hello_info(&self) -> Result<DaemonInfo> {
        match self.request(|req| ClientMsg::Hello { req }).await? {
            Reply::Hello {
                protocol,
                generation,
                boot,
            } => Ok(DaemonInfo {
                protocol,
                generation,
                boot,
            }),
            other => bail!("unexpected reply: {other:?}"),
        }
    }

    /// Asks the daemon to re-exec as `exe`. `Ok` means the daemon accepted and closed
    /// the connection to hand off; confirm with a fresh connection's generation.
    pub async fn upgrade(&self, exe: PathBuf) -> Result<()> {
        match self.request(|req| ClientMsg::Upgrade { req, exe }).await {
            Ok(other) => bail!("unexpected reply: {other:?}"),
            Err(_) if self.closed.load(Ordering::SeqCst) => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub async fn spawn(&self, spec: SpawnSpec) -> Result<SessionInfo> {
        match self.request(|req| ClientMsg::Spawn { req, spec }).await? {
            Reply::Session { info } => Ok(info),
            other => bail!("unexpected reply: {other:?}"),
        }
    }

    pub async fn list(&self) -> Result<Vec<SessionInfo>> {
        match self.request(|req| ClientMsg::List { req }).await? {
            Reply::Sessions { sessions } => Ok(sessions),
            other => bail!("unexpected reply: {other:?}"),
        }
    }

    pub async fn kill(&self, session: SessionId) -> Result<()> {
        self.request(|req| ClientMsg::Kill { req, session })
            .await
            .map(drop)
    }

    pub async fn attach(&self, session: SessionId, size: Size) -> Result<()> {
        let size = Some(size);
        self.request(|req| ClientMsg::Attach { req, session, size })
            .await
            .map(drop)
    }

    pub async fn detach(&self) -> Result<()> {
        self.request(|req| ClientMsg::Detach { req })
            .await
            .map(drop)
    }

    pub async fn dump(&self, session: SessionId) -> Result<String> {
        match self.request(|req| ClientMsg::Dump { req, session }).await? {
            Reply::Text { text } => Ok(text),
            other => bail!("unexpected reply: {other:?}"),
        }
    }

    /// A screenful of scrollback: `(from_top, history, rows)` (see `Reply::Scrollback`).
    pub async fn scrollback(
        &self,
        session: SessionId,
        anchor: crate::ScrollAnchor,
    ) -> Result<(u32, u32, Vec<crate::Row>)> {
        match self
            .request(|req| ClientMsg::Scrollback {
                req,
                session,
                anchor,
            })
            .await?
        {
            Reply::Scrollback {
                from_top,
                history,
                rows,
            } => Ok((from_top, history, rows)),
            other => bail!("unexpected reply: {other:?}"),
        }
    }

    /// Start receiving `Queue` pushes (the current queue first).
    pub async fn watch_queue(&self) -> Result<()> {
        self.request(|req| ClientMsg::WatchQueue { req })
            .await
            .map(drop)
    }

    pub async fn move_session(&self, session: SessionId, to: usize) -> Result<()> {
        self.request(|req| ClientMsg::Move { req, session, to })
            .await
            .map(drop)
    }

    pub async fn rename(&self, session: SessionId, name: Option<String>) -> Result<()> {
        self.request(|req| ClientMsg::Rename { req, session, name })
            .await
            .map(drop)
    }

    pub async fn mark_seen(&self, session: SessionId, seq: u64) -> Result<()> {
        self.request(|req| ClientMsg::MarkSeen { req, session, seq })
            .await
            .map(drop)
    }

    /// Fire-and-forget: the terminal's cell size in pixels (see `ClientMsg::CellPixels`).
    pub fn cell_pixels(&self, session: Option<SessionId>, width: u16, height: u16) -> Result<()> {
        self.out
            .send(ClientMsg::CellPixels {
                session,
                width,
                height,
            })
            .map_err(|_| anyhow!("daemon connection closed"))
    }

    /// Fire-and-forget, like `valk hook`.
    pub fn hook(
        &self,
        session: SessionId,
        agent: &str,
        sent_us: u64,
        payload: serde_json::Value,
    ) -> Result<()> {
        self.out
            .send(ClientMsg::Hook {
                session,
                agent: agent.to_owned(),
                sent_us,
                payload,
            })
            .map_err(|_| anyhow!("daemon connection closed"))
    }

    /// Fire-and-forget; input has no reply so keystrokes never wait on a round trip.
    pub fn input(&self, session: SessionId, data: Vec<u8>) -> Result<()> {
        self.out
            .send(ClientMsg::Input { session, data })
            .map_err(|_| anyhow!("daemon connection closed"))
    }

    pub fn resize(&self, session: SessionId, size: Size) -> Result<()> {
        self.out
            .send(ClientMsg::Resize { session, size })
            .map_err(|_| anyhow!("daemon connection closed"))
    }
}

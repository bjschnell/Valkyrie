//! Async daemon client: request/response matching plus a stream of server pushes.

use crate::codec::{read_frame, write_frame};
use crate::{ClientMsg, Reply, ReqId, ServerMsg, SessionId, SessionInfo, Size, SpawnSpec};
use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// Server-pushed messages (`Screen`, `Exited`). Closes when the connection drops.
pub type Pushes = mpsc::UnboundedReceiver<ServerMsg>;

impl Client {
    pub async fn connect(path: &Path) -> Result<(Client, Pushes)> {
        if let Some(dir) = path.parent() {
            crate::ensure_private_dir(dir)?;
        }
        let stream = UnixStream::connect(path).await?;
        let (mut rd, mut wr) = stream.into_split();
        let (out, mut out_rx) = mpsc::unbounded_channel::<ClientMsg>();
        let (push_tx, push_rx) = mpsc::unbounded_channel();
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
            push_rx,
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

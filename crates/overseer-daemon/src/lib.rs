//! The daemon: the only stateful component (DESIGN §4). Hosts sessions and serves the
//! protocol over a unix socket.

mod session;

use anyhow::{Context, Result};
use overseer_proto::codec::{read_frame, write_frame};
use overseer_proto::{ClientMsg, Reply, ReqId, ServerMsg, SessionId};
use session::Session;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Default)]
struct Registry {
    sessions: Mutex<BTreeMap<SessionId, Arc<Session>>>,
    next_id: AtomicU32,
    transcript_dir: PathBuf,
}

impl Registry {
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

/// Serve forever. Transcripts go to `<state_dir>/sessions/<ns>-<id>.raw`.
pub async fn run(socket: &Path, state_dir: &Path) -> Result<()> {
    let dir = socket.parent().context("socket path has no parent")?;
    overseer_proto::ensure_private_dir(dir)?;
    if socket.exists() {
        if UnixStream::connect(socket).await.is_ok() {
            anyhow::bail!("a daemon is already listening on {}", socket.display());
        }
        std::fs::remove_file(socket)?;
    }
    let listener =
        UnixListener::bind(socket).with_context(|| format!("bind {}", socket.display()))?;
    tracing::info!("listening on {}", socket.display());

    let registry = Arc::new(Registry {
        next_id: AtomicU32::new(1),
        transcript_dir: state_dir.join("sessions"),
        ..Default::default()
    });
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) => {
                // Usually fd exhaustion; exiting would SIGHUP every hosted agent.
                tracing::warn!("accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
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
        self.session.clients.fetch_sub(1, Ordering::Relaxed);
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
    while let Some(msg) = read_frame::<_, ClientMsg>(&mut rd).await? {
        let (req, result) = match msg {
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
            ClientMsg::Spawn { req, spec } => (req, spawn(&registry, spec)),
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
                let removed = registry.sessions.lock().unwrap().remove(&session);
                let result = match removed {
                    Some(s) => {
                        s.kill();
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
                    s.clients.fetch_add(1, Ordering::Relaxed);
                    reply(&out, req, Ok(Reply::Done)).await;
                    let task = tokio::spawn(forward(s.clone(), out.clone()));
                    attachment = Some(Attachment { session: s, task });
                    continue;
                }
                Err(e) => (req, Err(e)),
            },
        };
        reply(&out, req, result).await;
    }
    if let Some(a) = attachment {
        a.end().await;
    }
    writer.abort();
    Ok(())
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

fn spawn(registry: &Registry, spec: overseer_proto::SpawnSpec) -> Result<Reply> {
    let id = registry.next_id.fetch_add(1, Ordering::Relaxed);
    let session = Session::spawn(id, spec, &registry.transcript_dir)?;
    let info = session.info();
    registry.sessions.lock().unwrap().insert(id, session);
    tracing::info!(session = id, command = ?info.command, "spawned");
    Ok(Reply::Session { info })
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

//! Wire protocol shared by the daemon and every client (ADR-0003).
//!
//! Frames are a 4-byte big-endian length followed by a JSON body. Requests carry a
//! `req` id that the matching `Ok`/`Err` reply echoes; server pushes (`Screen`,
//! `Exited`) carry none.

pub mod agent;
pub mod client;
pub mod codec;
pub mod screen;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use agent::{AgentState, AgentStatus, AskKind, QueueItem};
pub use screen::{Color, Cursor, CursorShape, Modes, Row, ScreenUpdate, Span, Style};

/// Bumped on incompatible protocol changes; clients check it with `Hello`.
pub const PROTOCOL: u32 = 2;

pub type ReqId = u64;
pub type SessionId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

impl Size {
    /// Smallest size the daemon will apply to a PTY and screen.
    pub fn clamped(self) -> Size {
        Size {
            cols: self.cols.max(2),
            rows: self.rows.max(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpawnSpec {
    pub command: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub name: Option<String>,
    pub size: Size,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub name: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub pid: Option<u32>,
    pub created_unix: u64,
    pub title: Option<String>,
    pub clients: u32,
    /// `None` while running; `Some(code)` once the child exited (`code` may be unknown).
    pub exited: Option<Option<i32>>,
    #[serde(default)]
    pub status: AgentStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Version check. A daemon older than protocol 2 doesn't know it and hangs up.
    Hello {
        req: ReqId,
    },
    Spawn {
        req: ReqId,
        spec: SpawnSpec,
    },
    List {
        req: ReqId,
    },
    Kill {
        req: ReqId,
        session: SessionId,
    },
    /// Subscribe this connection to a session's screen. Replaces any previous attachment.
    Attach {
        req: ReqId,
        session: SessionId,
        size: Size,
    },
    Detach {
        req: ReqId,
    },
    /// Plain-text dump of the visible screen.
    Dump {
        req: ReqId,
        session: SessionId,
    },
    Input {
        session: SessionId,
        data: Vec<u8>,
    },
    Resize {
        session: SessionId,
        size: Size,
    },
    /// An agent hook event, forwarded by `overseer hook <agent>` (ADR-0005). No reply:
    /// the hook never waits on the daemon.
    Hook {
        session: SessionId,
        agent: String,
        /// When the hook process sent it (µs since the epoch); orders racing hooks.
        sent_us: u64,
        payload: serde_json::Value,
    },
    /// Subscribe this connection to `Queue` pushes, starting with the current queue.
    WatchQueue {
        req: ReqId,
    },
    /// Mark a session's current state seen (DESIGN §14.4). Ignored if the session has
    /// moved past `seq` since the client looked.
    MarkSeen {
        req: ReqId,
        session: SessionId,
        seq: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum ServerMsg {
    Ok {
        req: ReqId,
        reply: Reply,
    },
    Err {
        req: ReqId,
        message: String,
    },
    Screen {
        session: SessionId,
        update: ScreenUpdate,
    },
    Exited {
        session: SessionId,
        code: Option<i32>,
    },
    /// The whole ranked queue, pushed whenever it changes.
    Queue {
        items: Vec<QueueItem>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Reply {
    Done,
    Hello { protocol: u32 },
    Session { info: SessionInfo },
    Sessions { sessions: Vec<SessionInfo> },
    Text { text: String },
}

/// Default daemon socket: `$XDG_RUNTIME_DIR/overseer/overseer.sock`, falling back to
/// `/tmp/overseer-<uid>/overseer.sock`.
pub fn default_socket_path() -> PathBuf {
    runtime_dir().join("overseer.sock")
}

pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("overseer"),
        // SAFETY: getuid cannot fail.
        None => PathBuf::from(format!("/tmp/overseer-{}", unsafe { libc::getuid() })),
    }
}

/// Creates the socket's directory if needed and refuses to use it unless it is a real
/// directory (not a symlink) owned by us with no group/other access. Whoever controls
/// that directory controls which daemon our keystrokes go to.
pub fn ensure_private_dir(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        anyhow::bail!(
            "refusing to use {}: must be a directory owned by uid {uid} with mode 0700",
            dir.display()
        );
    }
    Ok(())
}

/// `$XDG_STATE_HOME/overseer`, falling back to `~/.local/state/overseer`.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(dir).join("overseer");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/state/overseer")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_dir_is_created_0700_and_loose_or_symlinked_dirs_are_refused() {
        let base = std::env::temp_dir().join(format!("overseer-dir-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let fresh = base.join("fresh");
        ensure_private_dir(&fresh).unwrap();
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);

        let loose = base.join("loose");
        std::fs::create_dir(&loose).unwrap();
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ensure_private_dir(&loose).is_err());

        let link = base.join("link");
        std::os::unix::fs::symlink(&fresh, &link).unwrap();
        assert!(ensure_private_dir(&link).is_err());

        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn clamped_size_is_never_zero() {
        assert_eq!(
            Size { cols: 0, rows: 0 }.clamped(),
            Size { cols: 2, rows: 1 }
        );
    }
}

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
pub const PROTOCOL: u32 = 3;

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
    /// The spawning client's login environment (`login_env`); `None` unsets a
    /// variable the daemon inherited from whichever login started it.
    #[serde(default)]
    pub env: Vec<(String, Option<String>)>,
}

/// Variables tied to one login rather than the user: the daemon outlives the login
/// that started it, so each spawn takes them from the client asking (as tmux's
/// `update-environment` does). Otherwise a later SSH login's agents would get a dead
/// `SSH_AUTH_SOCK` (git push fails) or a deleted `XDG_RUNTIME_DIR`.
pub const LOGIN_ENV: &[&str] = &[
    "SSH_AUTH_SOCK",
    "SSH_CONNECTION",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
];

pub fn login_env() -> Vec<(String, Option<String>)> {
    LOGIN_ENV
        .iter()
        .map(|k| (k.to_string(), std::env::var(k).ok()))
        .collect()
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
    /// An agent hook event, forwarded by `valk hook <agent>` (ADR-0005). No reply:
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
    /// Re-exec the daemon as `exe`, keeping every session (ADR-0006). Only a refusal
    /// is answered; on success the connection simply closes and the next `Hello`
    /// reports a higher `generation`.
    Upgrade {
        req: ReqId,
        exe: PathBuf,
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
    Hello {
        protocol: u32,
        /// How many upgrade handoffs this daemon's sessions have been through.
        #[serde(default)]
        generation: u32,
        /// Identifies this daemon across handoffs: a restarted daemon (whose session
        /// ids start over) has a different one.
        #[serde(default)]
        boot: u64,
    },
    Session {
        info: SessionInfo,
    },
    Sessions {
        sessions: Vec<SessionInfo>,
    },
    Text {
        text: String,
    },
}

/// Default daemon socket: `<state dir>/run/<hostname>.sock`. Not `$XDG_RUNTIME_DIR`:
/// logind deletes that when the user's last login ends, and SSH logins may not set
/// it, so a later `ssh host; valk` would start a second, empty daemon while the
/// first one keeps the sessions (herdr keeps its socket under `~/.config` too). The
/// hostname keeps machines sharing an NFS home from evicting each other's daemon.
pub fn default_socket_path() -> PathBuf {
    state_dir().join("run").join(format!("{}.sock", hostname()))
}

/// Where older daemons listened, to point upgraders at a stray one: the pre-rename
/// `<state dir>/../overseer`, and the `$XDG_RUNTIME_DIR` and `/tmp` sockets used
/// before 2026-10-07.
pub fn legacy_socket_paths() -> Vec<PathBuf> {
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let mut paths = vec![
        state_dir()
            .with_file_name("overseer")
            .join("run")
            .join(format!("{}.sock", hostname())),
    ];
    paths.extend(
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(|dir| PathBuf::from(dir).join("overseer/overseer.sock")),
    );
    paths.push(PathBuf::from(format!("/tmp/overseer-{uid}/overseer.sock")));
    paths
}

/// Longest path a unix socket address holds (`sun_path` is 108 bytes with the NUL).
pub const MAX_SOCKET_PATH: usize = 107;

/// This machine's name, safe as a file name.
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length; gethostname NUL-terminates or
    // truncates within it.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name: String = String::from_utf8_lossy(&buf[..if ok { end } else { 0 }])
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '_',
        })
        .take(64)
        .collect();
    match name.as_str() {
        "" | "." | ".." => "localhost".into(),
        _ => name,
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

/// `$XDG_STATE_HOME/valkyrie`, falling back to `~/.local/state/valkyrie`.
pub fn state_dir() -> PathBuf {
    // Relative values are invalid per the XDG spec, and would make the socket path
    // depend on the working directory.
    let absolute = |var| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    if let Some(dir) = absolute("XDG_STATE_HOME") {
        return dir.join("valkyrie");
    }
    absolute("HOME")
        .or_else(passwd_home)
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".local/state/valkyrie")
}

/// The home directory from the password database, for when `$HOME` is unset.
fn passwd_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 4096];
    let mut out = std::ptr::null_mut();
    // SAFETY: all pointers are valid for the call; `out` is null on failure.
    let rc = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut out,
        )
    };
    if rc != 0 || out.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir points into `buf`, NUL-terminated by getpwuid_r.
    let dir = unsafe { CStr::from_ptr(pwd.pw_dir) };
    let dir = PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes()));
    dir.is_absolute().then_some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_dir_is_created_0700_and_loose_or_symlinked_dirs_are_refused() {
        let base = std::env::temp_dir().join(format!("valkyrie-dir-test-{}", std::process::id()));
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

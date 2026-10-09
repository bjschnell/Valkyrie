//! Wire protocol shared by the daemon and every client (ADR-0003).
//!
//! Frames are a 4-byte big-endian length followed by a JSON body. Requests carry a
//! `req` id that the matching `Ok`/`Err` reply echoes; server pushes (`Screen`,
//! `Exited`) carry none.

pub mod agent;
pub mod client;
pub mod codec;
pub mod context;
pub mod layout;
pub mod screen;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use agent::{AgentState, AgentStatus, AskKind, QueueItem};
pub use context::{Decision, DecisionKind, DecisionStatus, NewDecision, Provenance, ReviewAction};
pub use layout::{Axis, Pane, Side};
pub use screen::{Color, Cursor, CursorShape, Modes, Row, ScreenUpdate, Span, Style};

/// Bumped on incompatible protocol changes; clients check it with `Hello`.
pub const PROTOCOL: u32 = 10;

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
    /// When a client last typed into it (ms since the epoch; 0: never). The web
    /// app's server reads it to tell someone at a keyboard from someone away.
    #[serde(default)]
    pub last_input_ms: u64,
    /// The agent's own transcript (Claude's or Codex's JSONL log), when known: the
    /// web app's Chat view reads it (DESIGN §8.7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<PathBuf>,
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
    /// With a size the session takes it (the TUI); without, the screen is watched at
    /// whatever size it has (a phone must not shrink the desktop's session).
    Attach {
        req: ReqId,
        session: SessionId,
        size: Option<Size>,
    },
    /// Subscribe this connection to every pane of a tab (DESIGN §8.9), each taking
    /// its size: the connection then watches exactly these. Panes it already watched
    /// carry on without a new snapshot.
    AttachPanes {
        req: ReqId,
        panes: Vec<(SessionId, Size)>,
    },
    Detach {
        req: ReqId,
    },
    /// Start `spec` as a new pane on `side` of `session`, in the same tab. Replies
    /// `Session`.
    Split {
        req: ReqId,
        session: SessionId,
        side: Side,
        spec: SpawnSpec,
    },
    /// Move the divider between the panes of `a` and `b` (the split where they part):
    /// `a`'s side gets `ratio` thousandths of the room.
    Ratio {
        req: ReqId,
        a: SessionId,
        b: SessionId,
        ratio: u16,
    },
    /// Plain-text dump of the visible screen.
    Dump {
        req: ReqId,
        session: SessionId,
    },
    /// One screenful of a session's scrollback (history above the screen, then the
    /// screen), for scrolling back while attached.
    Scrollback {
        req: ReqId,
        session: SessionId,
        anchor: ScrollAnchor,
    },
    Input {
        session: SessionId,
        data: Vec<u8>,
    },
    Resize {
        session: SessionId,
        size: Size,
    },
    /// The client terminal's cell size in pixels, for programs that size images by
    /// it. Without `session`, the default for sessions spawned from now on. No reply.
    CellPixels {
        session: Option<SessionId>,
        width: u16,
        height: u16,
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
    /// Move a session's tab to place `to` among the tabs (the order every client
    /// shows). Its panes move with it.
    Move {
        req: ReqId,
        session: SessionId,
        to: usize,
    },
    /// Give a session its own name; `None` goes back to the default, its directory.
    Rename {
        req: ReqId,
        session: SessionId,
        name: Option<String>,
    },
    /// Re-exec the daemon as `exe`, keeping every session (ADR-0006). Only a refusal
    /// is answered; on success the connection simply closes and the next `Hello`
    /// reports a higher `generation`.
    Upgrade {
        req: ReqId,
        exe: PathBuf,
    },
    /// Record a project decision (ADR-0007). Replies `Decision`. From a session
    /// running an agent it is only ever proposed.
    Decide {
        req: ReqId,
        decision: NewDecision,
    },
    /// Accept, reject, retire or reword a decision. Replies `Decision`.
    Review {
        req: ReqId,
        project: PathBuf,
        id: u32,
        action: ReviewAction,
    },
    /// The decisions of the project holding `cwd`, or of every project. Replies
    /// `Decisions`.
    Decisions {
        req: ReqId,
        cwd: Option<PathBuf>,
    },
    /// Asks whether the connecting process is a human's (ADR-0007 §4), before doing
    /// something only the user may, like pairing a phone. Replies `Done`, or an
    /// error naming `what`.
    Vouch {
        req: ReqId,
        what: String,
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
    /// Every project's decisions waiting on review, oldest first; pushed to queue
    /// watchers after the first `Queue` and whenever they change (ADR-0007).
    Proposals {
        items: Vec<Decision>,
    },
    /// The attached session's program copied `text` (OSC 52); the client puts it on
    /// its own clipboard.
    Clipboard {
        session: SessionId,
        text: String,
    },
    /// A Kitty graphics command from the attached session's program (DESIGN §8.4),
    /// to write to the client's terminal with the cursor at (`x`, `y`) of the session
    /// screen. Sent live, and replayed after the snapshot on attach.
    Graphics {
        session: SessionId,
        x: u16,
        y: u16,
        data: String,
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
        /// The tabs with more than one pane; every other session is a tab of its own.
        #[serde(default)]
        layouts: Vec<Pane>,
    },
    Text {
        text: String,
    },
    Decision {
        decision: Decision,
    },
    Decisions {
        decisions: Vec<Decision>,
    },
    Scrollback {
        /// The first row's line, counting from the oldest line in history.
        from_top: u32,
        /// Lines of history above the screen; `from_top == history` is the live screen.
        history: u32,
        /// One screenful, `y` from 0.
        rows: Vec<Row>,
    },
}

/// Where a `Scrollback` page starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollAnchor {
    /// This many lines above the live screen.
    Up(u32),
    /// At this line from the oldest one in history; stays put while output arrives.
    FromTop(u32),
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

/// Longest path a unix socket address holds (`sun_path` with its NUL: 108 bytes on
/// Linux, 104 on macOS and the BSDs).
#[cfg(target_os = "linux")]
pub const MAX_SOCKET_PATH: usize = 107;
#[cfg(not(target_os = "linux"))]
pub const MAX_SOCKET_PATH: usize = 103;

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

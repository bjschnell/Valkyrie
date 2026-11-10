//! `valk statusline [--then <command>]`: Claude Code's status line, as Valkyrie
//! puts it in the user's settings (`install_ambient`). Claude Code hands the status
//! line how much of the plan is used, and nothing else does, so this reads that and
//! tells the daemon, which shows it on the home screens. Then it runs the user's own
//! status line on the same input, whose output (and exit) is the status line's.
//!
//! Like the hook, it must never get in the way: a daemon that isn't there, or a
//! payload without the quota, costs the reading, never the status line.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use valkyrie_proto::{ClientMsg, Quota};

/// A status line payload is a couple of kilobytes; anything past this is cut.
const MAX_PAYLOAD: u64 = 1024 * 1024;
const SEND_TIMEOUT: Duration = Duration::from_millis(300);

pub fn run(socket: &Path, then: Option<&str>) -> ! {
    let mut raw = Vec::new();
    let _ = std::io::stdin().take(MAX_PAYLOAD).read_to_end(&mut raw);
    // Theirs starts first, so it draws while the daemon hears.
    let child = then.and_then(|command| {
        let mut child = Command::new("sh")
            .args(["-c", command])
            .stdin(Stdio::piped())
            .spawn()
            .ok()?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&raw);
        }
        Some(child)
    });
    if let Some(quota) = read(&raw) {
        let _ = send(socket, quota);
    }
    let code = child
        .and_then(|mut child| child.wait().ok())
        .and_then(|status| status.code())
        .unwrap_or(0);
    std::process::exit(code)
}

fn read(raw: &[u8]) -> Option<Quota> {
    let payload: serde_json::Value = serde_json::from_slice(raw).ok()?;
    Some(Quota {
        at: SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs(),
        limits: valkyrie_agents::statusline_quota(&payload)?,
    })
}

fn send(socket: &Path, quota: Quota) -> anyhow::Result<()> {
    let frame = valkyrie_proto::codec::encode(&ClientMsg::Quota { quota })?;
    valkyrie_proto::ipc::exchange(socket, &frame, false, SEND_TIMEOUT)?;
    Ok(())
}

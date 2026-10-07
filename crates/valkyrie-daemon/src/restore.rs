//! Sessions come back after the daemon restarts, a reboot included (DESIGN §8.3).
//!
//! The daemon keeps `<state dir>/restore-<host>.json` in step with its sessions. A
//! freshly started daemon (not an upgrade, which keeps the sessions themselves) spawns
//! each restorable one again: agents resume their own conversation, interactive shells
//! start over in the same directory, and other programs are left alone, since running
//! an arbitrary command again could do harm.

use crate::session::RestoreEntry;
use crate::{Registry, spawn_with};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use valkyrie_proto::{Reply, Size, SpawnSpec};

/// A program that exits stays on the list this long, so the deaths of a reboot, which
/// the daemon may reap before it dies itself, do not empty it.
pub const EXIT_GRACE_MS: u64 = 3_000;
/// How often the list is compared with the sessions (written only when it differs).
const KEEP_EVERY: Duration = Duration::from_secs(1);
/// Restored sessions start at this size; the first attach resizes them.
const SIZE: Size = Size {
    cols: 120,
    rows: 40,
};

#[derive(Debug, Default, Serialize, Deserialize)]
struct List {
    sessions: Vec<RestoreEntry>,
}

/// Per host, so machines sharing an NFS home keep their own lists, and per socket, so
/// a second daemon on another socket (a test one, say) never restores this one's live
/// sessions.
pub fn path(state_dir: &Path, socket: &Path) -> PathBuf {
    let host = valkyrie_proto::hostname();
    if socket == valkyrie_proto::default_socket_path() {
        return state_dir.join(format!("restore-{host}.json"));
    }
    // FNV-1a: stable across builds, unlike the std hasher.
    let hash = socket
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |h, &b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        });
    state_dir.join(format!("restore-{host}-{hash:016x}.json"))
}

fn read(path: &Path) -> Result<Vec<RestoreEntry>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice::<List>(&bytes)
            .with_context(|| format!("parse {}", path.display()))?
            .sessions),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Atomically, readable only by the user: it holds command lines.
fn write(path: &Path, json: &str) -> Result<()> {
    let dir = path.parent().context("restore list has no directory")?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(json.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn render(registry: &Registry) -> Option<String> {
    let now = crate::session::now_ms();
    // Entries that failed to come back stay for the next restart to try again (a
    // `claude` missing from PATH, a cwd on a mount not up yet).
    let mut sessions: Vec<RestoreEntry> = registry.unrestored.lock().unwrap().clone();
    sessions.extend(
        registry
            .all()
            .iter()
            .filter_map(|s| s.restore_entry(now, EXIT_GRACE_MS)),
    );
    serde_json::to_string_pretty(&List { sessions }).ok()
}

/// Keeps the list in step with the sessions for as long as the daemon runs: checked
/// every second, and at once when a session is killed.
pub async fn keep(registry: Arc<Registry>, path: PathBuf) {
    let mut written = None;
    loop {
        if let Some(json) = render(&registry)
            && written.as_ref() != Some(&json)
        {
            match write(&path, &json) {
                Ok(()) => written = Some(json),
                Err(e) => tracing::warn!("restore list not written: {e:#}"),
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(KEEP_EVERY) => {}
            _ = registry.restore_now.notified() => {}
        }
    }
}

/// Spawns what the last daemon left on the list. Returns how many came back.
/// `VALK_RESTORE=off` skips it (the list is then rewritten without them).
pub fn restore(registry: &Registry, path: &Path) -> usize {
    if std::env::var("VALK_RESTORE").is_ok_and(|v| v == "off") {
        return 0;
    }
    let entries = match read(path) {
        Ok(entries) => entries,
        Err(e) => {
            // Set aside rather than overwritten by the next write.
            let bad = path.with_extension("json.bad");
            tracing::warn!("nothing restored, list kept as {}: {e:#}", bad.display());
            let _ = std::fs::rename(path, bad);
            return 0;
        }
    };
    let mut restored = 0;
    for entry in entries {
        let adapter = valkyrie_agents::adapter_for(&entry.command);
        let Some(command) = adapter.restore(&entry.command, entry.conversation.as_deref()) else {
            tracing::info!(name = entry.name, command = ?entry.command, "not restored");
            continue;
        };
        // The daemon's own environment: per-spawn variables are not saved.
        let spec = SpawnSpec {
            command: command.clone(),
            cwd: Some(entry.cwd.clone()),
            name: Some(entry.name.clone()),
            size: SIZE,
            env: Vec::new(),
        };
        match spawn_with(registry, spec, entry.conversation.clone()) {
            Ok(Reply::Session { info }) => {
                tracing::info!(session = info.id, ?command, "restored");
                restored += 1;
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(name = entry.name, ?command, "restore failed: {e:#}");
                registry.unrestored.lock().unwrap().push(entry);
            }
        }
    }
    restored
}

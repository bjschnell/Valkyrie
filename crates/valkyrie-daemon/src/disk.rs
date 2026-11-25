//! Watching free disk space (backlog #300). On 2026-11-19 agents' worktree
//! builds filled the disk and took the daemon down with nothing said first. The
//! sweep now trims build output (`sweep.rs`), and this is the backstop: every
//! minute the daemon reads the free space of the disks under its state folder
//! and the repositories it knows of, pushes those running low to queue watchers
//! as `Disk` for the Inbox to show, and raises a toast when one starts running
//! low. The Inbox's row offers `FreeDisk`, which trims every worktree's idle
//! `target/` at once.

use crate::Registry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use valkyrie_proto::LowDisk;

/// Between readings: `statvfs` is cheap, and a build can eat gigabytes a minute.
const EVERY: Duration = Duration::from_secs(60);
/// Low below this many bytes free, however big the disk.
const FLOOR: u64 = 10_000_000_000;
/// Or below this share of the disk free, in percent: a few agents' builds.
const SHARE: u64 = 5;

/// What the Inbox shows: the disks running low, or none.
pub(crate) type Held = Arc<Vec<LowDisk>>;

/// Whether `free` bytes of `total` is running low.
pub(crate) fn low(free: u64, total: u64) -> bool {
    free < FLOOR || free.saturating_mul(100) < total.saturating_mul(SHARE)
}

/// Whether `new` is worth pushing over `old`: another disk ran low or recovered,
/// or one's free space moved by a gigabyte. Smaller moves would redraw every
/// Inbox each minute for nothing.
pub(crate) fn changed(old: &[LowDisk], new: &[LowDisk]) -> bool {
    old.len() != new.len()
        || old
            .iter()
            .zip(new)
            .any(|(a, b)| a.path != b.path || a.free.abs_diff(b.free) >= 1_000_000_000)
}

/// Reads the disks every `EVERY`, from the start.
pub(crate) async fn run(registry: Arc<Registry>) {
    let mut interval = tokio::time::interval(EVERY);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        registry.read_disks().await;
    }
}

/// The disks under `paths`, one folder each, with their free and total bytes.
pub(crate) fn read(paths: &[PathBuf]) -> Vec<LowDisk> {
    let mut seen = Vec::new();
    let mut disks = Vec::new();
    for path in paths {
        let Some(id) = device(path) else {
            continue;
        };
        if seen.contains(&id) {
            continue;
        }
        let Some((free, total)) = space(path) else {
            continue;
        };
        seen.push(id);
        disks.push(LowDisk {
            path: path.clone(),
            free,
            total,
        });
    }
    disks
}

/// Which disk `path` is on.
#[cfg(unix)]
fn device(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

/// Which disk `path` is on: its drive (`C:`) or share.
#[cfg(windows)]
fn device(path: &Path) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let canonical = valkyrie_proto::canonical(path).ok()?;
    let root = canonical.components().next()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    root.as_os_str().to_ascii_lowercase().hash(&mut h);
    Some(h.finish())
}

/// The bytes free to an ordinary user and the size of the disk under `path`.
#[cfg(unix)]
fn space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path and `st` a valid out-pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    // u64 on Linux, narrower on macOS and 32-bit targets.
    #[allow(clippy::useless_conversion)]
    let unit = u64::from(st.f_frsize);
    #[allow(clippy::useless_conversion)]
    let blocks = |n| u64::from(n).saturating_mul(unit);
    Some((blocks(st.f_bavail), blocks(st.f_blocks)))
}

/// The bytes free to this user and the size of the disk under `path`.
#[cfg(windows)]
fn space(path: &Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: `wide` is NUL-terminated, and the out-pointers are valid u64s.
    let ok =
        unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    (ok != 0).then_some((free, total))
}

impl Registry {
    /// Reads the disks under the state folder and the known repositories, and
    /// pushes those running low when that changed, with a toast for each that
    /// just started running low.
    pub(crate) async fn read_disks(&self) {
        let mut paths = vec![self.state_dir.clone()];
        paths.extend(self.known_repos());
        let Ok(disks) = tokio::task::spawn_blocking(move || read(&paths)).await else {
            return;
        };
        let now: Vec<LowDisk> = disks.into_iter().filter(|d| low(d.free, d.total)).collect();
        let mut started = Vec::new();
        self.disk.send_if_modified(|held| {
            if !changed(held, &now) {
                return false;
            }
            started = now
                .iter()
                .filter(|d| !held.iter().any(|h| h.path == d.path))
                .cloned()
                .collect();
            *held = Arc::new(now);
            true
        });
        for d in started {
            let text = format!(
                "Disk low: {} free of {} under {}",
                crate::trim::shown(d.free),
                crate::trim::shown(d.total),
                d.path.display()
            );
            tracing::warn!("{text}");
            let _ = self.notify(&text, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_below_the_floor_or_the_share() {
        let tb = 1_000_000_000_000;
        assert!(!low(100_000_000_000, tb));
        // 4% of a terabyte is 40 GB: above the floor, below the share.
        assert!(low(40_000_000_000, tb));
        // A small disk is low only under the floor.
        assert!(!low(12_000_000_000, 64_000_000_000));
        assert!(low(9_000_000_000, 64_000_000_000));
    }

    #[test]
    fn pushes_only_what_moved() {
        let d = |path: &str, free| LowDisk {
            path: PathBuf::from(path),
            free,
            total: 1,
        };
        assert!(!changed(&[], &[]));
        assert!(changed(&[], &[d("/a", 5)]));
        assert!(!changed(
            &[d("/a", 5_000_000_000)],
            &[d("/a", 5_400_000_000)]
        ));
        assert!(changed(
            &[d("/a", 5_000_000_000)],
            &[d("/a", 3_900_000_000)]
        ));
        assert!(changed(&[d("/a", 5)], &[d("/b", 5)]));
    }

    #[test]
    fn reads_each_disk_once() {
        let here = std::env::current_dir().unwrap();
        let disks = read(&[
            here.clone(),
            here.join("src"),
            PathBuf::from("/nowhere/at/all"),
        ]);
        assert_eq!(disks.len(), 1, "{disks:?}");
        assert!(disks[0].total > 0 && disks[0].free <= disks[0].total);
    }
}

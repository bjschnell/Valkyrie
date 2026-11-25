//! Free disk space (protocol 53, backlog #300): agents' worktrees and their
//! builds filled the disk once and took the daemon down, so the daemon watches
//! the disks under its state and the repositories it knows of, and pushes
//! those running low to queue watchers, for the Inbox to show.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A disk running low on space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct LowDisk {
    /// A folder on it the daemon looked at: its state folder or a repository.
    pub path: PathBuf,
    /// Bytes free for an ordinary user, and the disk's size.
    pub free: u64,
    pub total: u64,
}

/// `bytes` for a person: `12.3 GB`, `512 MB`, `1.8 TB`.
pub fn bytes(n: u64) -> String {
    let b = n as f64;
    if b >= 1e12 {
        format!("{:.1} TB", b / 1e12)
    } else if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else {
        format!("{:.0} MB", b / 1e6)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shows_bytes() {
        assert_eq!(super::bytes(1_800_000_000_000), "1.8 TB");
        assert_eq!(super::bytes(8_240_000_000), "8.2 GB");
        assert_eq!(super::bytes(512_000_000), "512 MB");
    }
}

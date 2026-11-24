//! What the integration tests that act as a human share.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

/// `line`, held back until the returned [`Gate`] drops.
///
/// A test acts as a human with `setsid -f script -qec <line>`: a terminal of its
/// own, outside every session. But `setsid -f` forks, and until its parent exits,
/// the child's ancestry still runs through this test and whatever ran `cargo test`.
/// Under an agent, a loaded machine could let the line reach the daemon in that
/// window, and the daemon rightly took it for the agent's (backlog #293). Once
/// `.status()` has returned, that parent is gone and the child reparented, so keep
/// the gate alive past it: the line starts only when the gate drops.
pub fn after_setsid(line: &str) -> (String, Gate) {
    static N: AtomicU32 = AtomicU32::new(0);
    let go = std::env::temp_dir().join(format!(
        "valk-human-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    // `script` runs the line with $SHELL, which may not be sh: keep the wait a
    // plain command any shell parses. It gives up after 30s rather than hang.
    let held = format!(
        "timeout 30 sh -c 'until [ -e \"$0\" ]; do sleep 0.01; done' '{g}'; rm -f '{g}'; {line}",
        g = go.display()
    );
    (held, Gate(go))
}

/// Lets its line go when dropped, panics included.
pub struct Gate(PathBuf);

impl Drop for Gate {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "");
    }
}

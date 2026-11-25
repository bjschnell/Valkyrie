//! Deleting a worktree's cargo build folder, `target/` (backlog #230). Each
//! agent's worktree builds its own, 4-24 GB, and the sweep (`sweep.rs`) only
//! takes away worktrees nothing holds: one an agent still sits in after its
//! work merged, or one an open item left untouched for days, kept its build
//! output, and 52 of them filled the disk. The sweep says when a worktree's
//! `target/` may go; this finds it, tells when it was last built in, and
//! deletes it, never while cargo builds there or while it holds the running
//! `valk`.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// How deep under `target/` a build leaves a trace in a folder's mtime:
/// `target/debug/deps`, `target/<triple>/debug/deps`.
const DEPTH: usize = 3;

/// The cargo build folder of the worktree at `worktree`: its `target/`, when
/// cargo made it (it tags it with a `CACHEDIR.TAG` saying so) and it isn't a
/// link to somewhere else.
pub(crate) fn cargo_target(worktree: &Path) -> Option<PathBuf> {
    let target = worktree.join("target");
    let meta = std::fs::symlink_metadata(&target).ok()?;
    if !meta.is_dir() {
        return None;
    }
    let tag = std::fs::read_to_string(target.join("CACHEDIR.TAG")).ok()?;
    tag.contains("cargo").then_some(target)
}

/// When a build last wrote under `target`, in seconds since the epoch: the
/// newest mtime of it and the folders `DEPTH` below it, as compiling anything
/// adds files to `deps/`, `.fingerprint/` or `incremental/`. Folders only, so
/// it doesn't stat each of the thousands of files there.
pub(crate) fn last_built(target: &Path) -> Option<u64> {
    let mut newest = None;
    let mut dirs = vec![(target.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&dir) else {
            continue;
        };
        let at = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        newest = newest.max(at);
        if depth == DEPTH {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push((entry.path(), depth + 1));
            }
        }
    }
    newest
}

/// The programs that must not go with a `target/`: the `valk` running and the
/// one on `PATH` (`valk upgrade --local` links it into a checkout's
/// `target/release`), and on Linux every program running, as cargo holds no
/// lock while its tests run, and a test or a daemon under verification may run
/// from a worktree's build.
pub(crate) fn live_binaries() -> Vec<PathBuf> {
    let name = format!("valk{}", std::env::consts::EXE_SUFFIX);
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|dir| dir.join(&name))
        .find(|p| p.is_file());
    let mut live: Vec<PathBuf> = std::env::current_exe()
        .ok()
        .into_iter()
        .chain(on_path)
        .filter_map(|p| valkyrie_proto::canonical(&p).ok())
        .collect();
    live.extend(running());
    live
}

/// The executables of the processes running, as `/proc` shows those it may
/// read; one deleted since it started still counts, at the path it had.
#[cfg(target_os = "linux")]
fn running() -> Vec<PathBuf> {
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    procs
        .flatten()
        .filter(|p| {
            p.file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        .filter_map(|p| std::fs::read_link(p.path().join("exe")).ok())
        .map(|exe| {
            let shown = exe.to_string_lossy();
            match shown.strip_suffix(" (deleted)") {
                Some(was) => PathBuf::from(was),
                None => exe,
            }
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn running() -> Vec<PathBuf> {
    Vec::new()
}

/// Deletes the cargo build folder `target`, unless cargo holds its lock (a
/// build is on) or it holds one of `live`. Renamed aside first,
/// beside its worktree, so a build that starts meanwhile makes a fresh one
/// instead of finding half of it. Returns the bytes freed.
pub(crate) fn trim(target: &Path, live: &[PathBuf]) -> Result<u64, String> {
    let resolved = valkyrie_proto::canonical(target).map_err(|e| format!("{e:#}"))?;
    if let Some(bin) = live.iter().find(|b| b.starts_with(&resolved)) {
        return Err(format!("{} runs from it", bin.display()));
    }
    let locks = locks(target);
    for lock in &locks {
        let Ok(file) = File::open(lock) else {
            continue;
        };
        if file.try_lock().is_err() {
            return Err("cargo is building in it".to_owned());
        }
        // Released when `file` drops, before the rename: Windows won't rename
        // a folder with a file open in it.
    }
    let worktree = target.parent().ok_or("it has no parent")?;
    let name = worktree.file_name().unwrap_or_default().to_string_lossy();
    let aside = worktree.with_file_name(format!(".{name}.target-trimming-{}", std::process::id()));
    std::fs::rename(target, &aside).map_err(|e| format!("moving it aside: {e}"))?;
    let size = size(&aside);
    std::fs::remove_dir_all(&aside)
        .map_err(|e| format!("deleting it (what's left is at {}): {e}", aside.display()))?;
    Ok(size)
}

/// Cargo's build locks under `target`: `debug/.cargo-lock`,
/// `<triple>/release/.cargo-lock`, and the like.
fn locks(target: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![(target.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        let lock = dir.join(".cargo-lock");
        if lock.is_file() {
            found.push(lock);
        }
        if depth == 2 {
            continue;
        }
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push((entry.path(), depth + 1));
            }
        }
    }
    found
}

/// The bytes of the files under `dir`, links not followed.
fn size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                dirs.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

/// `bytes` for a person: `12.3 GB`, `512 MB`.
pub(crate) fn shown(bytes: u64) -> String {
    const GB: f64 = 1e9;
    const MB: f64 = 1e6;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A worktree folder with a cargo-made `target/` holding `debug/deps/x`.
    fn tree(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("valkyrie-trim-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let deps = dir.join("wt/target/debug/deps");
        std::fs::create_dir_all(&deps).unwrap();
        std::fs::write(
            dir.join("wt/target/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n\
             # This file is a cache directory tag created by cargo.\n",
        )
        .unwrap();
        std::fs::write(deps.join("x"), vec![0u8; 2048]).unwrap();
        std::fs::write(dir.join("wt/target/debug/.cargo-lock"), "").unwrap();
        dir
    }

    #[test]
    fn finds_only_a_cargo_target() {
        let dir = tree("find");
        let wt = dir.join("wt");
        assert_eq!(cargo_target(&wt), Some(wt.join("target")));
        assert!(last_built(&wt.join("target")).is_some());
        // Some other tool's `target/` isn't cargo's.
        std::fs::write(wt.join("target/CACHEDIR.TAG"), "Signature: x\n").unwrap();
        assert_eq!(cargo_target(&wt), None);
        std::fs::remove_dir_all(wt.join("target")).unwrap();
        assert_eq!(cargo_target(&wt), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trims_unless_cargo_builds_or_valk_runs_there() {
        let dir = tree("trim");
        let target = dir.join("wt/target");
        // The running valk is in it.
        let bin = valkyrie_proto::canonical(&target.join("debug/deps/x")).unwrap();
        let err = trim(&target, &[bin]).unwrap_err();
        assert!(err.contains("runs from it"), "{err}");
        // Cargo holds its build lock.
        let held = File::open(target.join("debug/.cargo-lock")).unwrap();
        held.lock().unwrap();
        let err = trim(&target, &[]).unwrap_err();
        assert!(err.contains("building"), "{err}");
        assert!(target.exists());
        drop(held);
        // Otherwise it goes whole, and nothing is left aside.
        let tag = std::fs::metadata(target.join("CACHEDIR.TAG"))
            .unwrap()
            .len();
        // A child another test forks while `held` was open holds the lock too,
        // until it execs.
        let freed = (0..50).map(|_| trim(&target, &[])).find(|r| {
            let busy = r.as_ref().is_err_and(|e| e.contains("building"));
            if busy {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            !busy
        });
        assert_eq!(freed, Some(Ok(2048 + tag)));
        assert!(!target.exists());
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().collect();
        assert_eq!(left.len(), 1, "{left:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A test binary running from it keeps it, though cargo holds no lock then.
    #[cfg(target_os = "linux")]
    #[test]
    fn keeps_a_target_a_program_runs_from() {
        let dir = tree("running");
        let target = dir.join("wt/target");
        let exe = target.join("debug/deps/sleeper");
        std::fs::copy("/bin/sleep", &exe).unwrap();
        // Another test forking while the copy was open for writing makes the
        // exec fail with "text file busy" for a moment.
        let mut child = (0..50)
            .find_map(
                |_| match std::process::Command::new(&exe).arg("30").spawn() {
                    Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        None
                    }
                    spawned => Some(spawned.unwrap()),
                },
            )
            .expect("text file busy");
        let err = trim(&target, &live_binaries()).unwrap_err();
        assert!(err.contains("runs from it"), "{err}");
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(trim(&target, &live_binaries()).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shows_sizes() {
        assert_eq!(shown(12_345_678_901), "12.3 GB");
        assert_eq!(shown(512_000_000), "512 MB");
    }
}

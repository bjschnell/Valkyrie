//! `valk worktree prune` with the real binary (DESIGN §4.3): against a throwaway
//! daemon and a repository with a bare `origin`, it takes away only the worktree
//! that's merged, clean and idle, with its branch, and only when the user asks.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_valk");

/// Kills the isolated daemon's whole session, whatever the test did.
struct Daemon {
    dir: PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let dir = self.dir.to_str().unwrap().to_owned();
        let pids = std::fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok());
        for pid in pids {
            let is_ours = std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| {
                let c = String::from_utf8_lossy(&c);
                c.contains(&dir) && c.contains("daemon")
            });
            if is_ours {
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{pid}")])
                    .status();
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn wait_for(path: &Path) -> String {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(20) {
        if let Ok(text) = std::fs::read_to_string(path)
            && text.ends_with('\n')
        {
            return text;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{} never written", path.display());
}

fn git(dir: &Path, args: &[&str]) -> Output {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    out
}

/// The line `prune` printed about the worktree at `path`.
fn line<'a>(said: &'a str, path: &Path) -> &'a str {
    let path = path.to_str().unwrap();
    said.lines()
        .find(|l| l.contains(path))
        .unwrap_or_else(|| panic!("nothing about {path} in:\n{said}"))
}

#[test]
fn prunes_only_merged_clean_idle_worktrees_and_only_for_the_user() {
    if Command::new("script").arg("--version").output().is_err() {
        eprintln!("skipped: needs util-linux script for a human's terminal");
        return;
    }
    let dir = std::env::temp_dir().join(format!("valkyrie-prune-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dir = dir.canonicalize().unwrap();
    let daemon = Daemon { dir: dir.clone() };
    let (repo, origin, out) = (dir.join("repo"), dir.join("origin.git"), dir.join("out"));
    let socket = dir.join("run/v.sock");
    let state = dir.join("state");
    for d in [&repo, &out] {
        std::fs::create_dir_all(d).unwrap();
    }

    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README"), "hi\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "one"]);
    git(&dir, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
    git(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&repo, &["push", "-q", "-u", "origin", "main"]);
    git(&repo, &["remote", "set-head", "origin", "main"]);

    let add = |branch: &str| {
        let path = dir.join(format!("repo-{branch}"));
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                path.to_str().unwrap(),
            ],
        );
        path
    };
    // Merged on origin only: the local `main` lags, as after a pull request.
    let merged = add("merged");
    git(
        &merged,
        &["commit", "-q", "--allow-empty", "-m", "merged work"],
    );
    git(&merged, &["push", "-q", "origin", "merged:main"]);
    let unmerged = add("unmerged");
    git(&unmerged, &["commit", "-q", "--allow-empty", "-m", "wip"]);
    // Pushed, then its branch deleted on origin without its commit landing there.
    let squashed = add("squashed");
    git(
        &squashed,
        &["commit", "-q", "--allow-empty", "-m", "squash me"],
    );
    git(&squashed, &["push", "-q", "-u", "origin", "squashed"]);
    git(&repo, &["push", "-q", "origin", "--delete", "squashed"]);
    let dirty = add("dirty");
    std::fs::write(dirty.join("notes.txt"), "keep me\n").unwrap();
    let live = add("live");

    // Every call names the throwaway daemon: VALK_SOCKET may point at a real one.
    let valk = |args: &[&str]| {
        Command::new(BIN)
            .arg("--socket")
            .arg(&socket)
            .args(args)
            .current_dir(&repo)
            .env("XDG_STATE_HOME", &state)
            .env_remove("VALK_SESSION")
            .output()
            .unwrap()
    };
    let spawned = valk(&["new", "--cwd", live.to_str().unwrap(), "--", "sleep", "600"]);
    assert!(spawned.status.success(), "{spawned:?}");
    let session = String::from_utf8(spawned.stdout).unwrap();

    // A dry run, which anyone (this test counts as an agent) may ask for.
    let dry = valk(&["worktree", "prune"]);
    let said = String::from_utf8(dry.stdout).unwrap();
    assert!(
        dry.status.success(),
        "{said}{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert!(
        said.contains("merged means in origin/main or HEAD of"),
        "{said}"
    );
    assert!(line(&said, &merged).contains("would go"), "{said}");
    assert!(
        line(&said, &unmerged).ends_with("(unmerged): not merged"),
        "{said}"
    );
    assert!(
        line(&said, &squashed).ends_with("not merged (squash merge?)"),
        "{said}"
    );
    assert!(
        line(&said, &dirty).ends_with("has changes (1 files)"),
        "{said}"
    );
    assert!(
        line(&said, &live).ends_with(&format!("session {} runs in it", session.trim())),
        "{said}"
    );
    // Never the main checkout.
    assert!(!said.contains(&format!("{} (", repo.display())), "{said}");
    assert!(said.contains("1 may go"), "{said}");
    for w in [&merged, &unmerged, &squashed, &dirty, &live] {
        assert!(w.exists(), "the dry run took {}", w.display());
    }

    // Removing is the user's.
    let refused = valk(&["worktree", "prune", "--yes"]);
    let err = String::from_utf8_lossy(&refused.stderr);
    assert!(
        !refused.status.success() && err.contains("is for the user"),
        "{err}"
    );
    assert!(merged.exists());

    // A human, as in backlog.rs: on a terminal of its own, outside every session.
    let human = |cmd: &str, name: &str| {
        let o = out.join(name).display().to_string();
        let line = format!(
            "{BIN} --socket {} {cmd} > {o}.tmp 2>&1; mv {o}.tmp {o}",
            socket.display()
        );
        let status = Command::new("setsid")
            .args(["-f", "script", "-qec", &line, "/dev/null"])
            .current_dir(&repo)
            .env("XDG_STATE_HOME", &state)
            .env_remove("VALK_SESSION")
            .env_remove("VALK_CONTEXT")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "{cmd}");
    };
    human("worktree prune --yes", "prune");
    let said = wait_for(&out.join("prune"));
    assert!(
        said.contains(&format!("took away {} and branch merged", merged.display())),
        "{said}"
    );
    assert!(!merged.exists(), "{said}");
    for w in [&unmerged, &squashed, &dirty, &live] {
        assert!(w.exists(), "{} went: {said}", w.display());
    }
    let branches = String::from_utf8(git(&repo, &["branch", "--list"]).stdout).unwrap();
    let names: Vec<&str> = branches
        .lines()
        .map(|l| l.trim_start_matches(['*', '+', ' ']))
        .collect();
    assert_eq!(
        names,
        ["dirty", "live", "main", "squashed", "unmerged"],
        "{said}"
    );
    drop(daemon);
}

//! Docs with the real binary (ADR-0021): an agent writes and edits docs with its
//! provenance filled in by the daemon, an edit from a stale read is refused, and
//! only the user moves the docs folder.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;
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
    while start.elapsed() < Duration::from_secs(15) {
        if let Ok(text) = std::fs::read_to_string(path)
            && text.ends_with('\n')
        {
            return text;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{} never written", path.display());
}

#[test]
fn an_agent_writes_and_edits_docs_and_only_the_user_moves_them() {
    if Command::new("script").arg("--version").output().is_err() {
        eprintln!("skipped: needs util-linux script for a human's terminal");
        return;
    }
    let dir = std::env::temp_dir().join(format!("valkyrie-docs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let daemon = Daemon { dir: dir.clone() };
    let (bin, repo, out) = (dir.join("bin"), dir.join("myrepo"), dir.join("out"));
    for d in [&bin, &repo.join(".git"), &out] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::os::unix::fs::symlink(BIN, bin.join("valk")).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let o = |name: &str| out.join(name).display().to_string();
    let vault = dir.join("vault/valk");

    // A fake agent, known by its program name. It writes a doc, edits it once from
    // a stale hash and once from a fresh one, and tries to move the docs folder.
    let agent = bin.join("claude");
    std::fs::write(
        &agent,
        format!(
            "#!/bin/sh\n\
             cd {repo}\n\
             printf '# Plan\\n\\n1. one\\n' | valk doc write 'The plan' -k plan - > {w} 2>&1\n\
             valk doc edit 1 --base 0000000000000000 'stale' > {stale} 2>&1\n\
             H=$(valk doc read 1 | head -1 | sed 's/.*hash \\([0-9a-f]*\\).*/\\1/')\n\
             printf '# Plan\\n\\n1. one\\n2. two\\n' | valk doc edit 1 --base \"$H\" - > {e} 2>&1\n\
             valk doc root {elsewhere} > {r} 2>&1\n\
             echo done > {d}\n\
             sleep 30\n",
            repo = repo.display(),
            w = o("write"),
            stale = o("stale"),
            e = o("edit"),
            r = o("root"),
            elsewhere = dir.join("elsewhere").display(),
            d = o("agent-done"),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&agent, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    // A human: on a terminal of its own, outside every session (as in context.rs).
    let human = |line: &str| {
        let status = Command::new("setsid")
            .args(["-f", "script", "-qec", line, "/dev/null"])
            .current_dir(&repo)
            .env("PATH", &path)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("VALK_SOCKET", dir.join("run/v.sock"))
            .env_remove("VALK_SESSION")
            .env_remove("VALK_CONTEXT")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "{line}");
    };
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::set_permissions(
        dir.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();

    // The user points docs at a vault folder first.
    human(&format!(
        "valk doc root {} > {} 2>&1",
        vault.display(),
        o("set")
    ));
    assert!(
        wait_for(&out.join("set")).contains("vault/valk"),
        "{}",
        wait_for(&out.join("set"))
    );

    human(&format!(
        "valk new --cwd {} -- {} > {}",
        repo.display(),
        agent.display(),
        o("spawned")
    ));
    wait_for(&out.join("agent-done"));
    let read = |name: &str| wait_for(&out.join(name));
    assert!(read("write").contains("doc 1 written"), "{}", read("write"));
    assert!(
        read("stale").contains("changed since you read it"),
        "{}",
        read("stale")
    );
    assert!(read("edit").contains("doc 1 edited"), "{}", read("edit"));
    assert!(read("root").contains("is for the user"), "{}", read("root"));
    assert!(!dir.join("elsewhere").exists());

    // In the repository's folder, with who wrote it worked out by the daemon.
    let file = std::fs::read_dir(vault.join("repos/myrepo"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("valk-id: 1\nkind: plan\ntitle: \"The plan\"\n"),
        "{text}"
    );
    assert!(text.contains("by: \"claude\"\n"), "{text}");
    assert!(text.contains("edited:\n  - \"claude, "), "{text}");
    assert!(
        text.ends_with("---\n\n# Plan\n\n1. one\n2. two\n"),
        "{text}"
    );
    // The version it replaced is kept.
    let history: Vec<_> = std::fs::read_dir(dir.join("state/valkyrie/docs-history/1"))
        .unwrap()
        .collect();
    assert_eq!(history.len(), 1);

    // The user can move it back to the default.
    human(&format!("valk doc root --default > {} 2>&1", o("reset")));
    assert!(
        read("reset").contains("state/valkyrie/docs"),
        "{}",
        read("reset")
    );
    drop(daemon);
}

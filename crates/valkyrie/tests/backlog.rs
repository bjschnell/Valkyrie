//! The backlog with the real binary (DESIGN §8.12): an item written down with a
//! screenshot starts an agent in a worktree of its own with a brief for its kind,
//! and shipping it takes the clean worktree and merged branch away again.
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
    while start.elapsed() < Duration::from_secs(10) {
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
fn an_item_launches_in_its_own_worktree_and_ships_clean() {
    if Command::new("script").arg("--version").output().is_err() {
        eprintln!("skipped: needs util-linux script for a human's terminal");
        return;
    }
    let dir = std::env::temp_dir().join(format!("valkyrie-backlog-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let daemon = Daemon { dir: dir.clone() };
    let (bin, repo, out) = (dir.join("bin"), dir.join("repo"), dir.join("out"));
    for d in [&bin, &repo, &out] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::os::unix::fs::symlink(BIN, bin.join("valk")).unwrap();
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap()
    };
    assert!(git(&["init", "-q", "-b", "main"]).status.success());
    std::fs::write(repo.join("README"), "hi\n").unwrap();
    git(&["add", "."]);
    assert!(git(&["commit", "-qm", "one"]).status.success());
    let shot = dir.join("shot.png");
    std::fs::write(&shot, b"not really a png").unwrap();

    // A stand-in agent: where it ran and the brief it was given, then it's done.
    let o = |name: &str| out.join(name).display().to_string();
    let agent = bin.join("claude");
    std::fs::write(
        &agent,
        format!(
            "#!/bin/sh\nfor a; do last=$a; done\nprintf '%s\\n' \"$last\" > {b}.tmp && mv {b}.tmp {b}\npwd > {p}\n",
            b = o("brief"),
            p = o("pwd"),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&agent, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // A human, as in context.rs: on a terminal of its own, outside every session.
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

    human(&format!(
        "valk todo add -k bug -f {} 'Tab rename eats the last letter' > {} 2>&1",
        shot.display(),
        o("add")
    ));
    assert_eq!(
        wait_for(&out.join("add")),
        "#1 Tab rename eats the last letter (repo)\n"
    );

    human(&format!("valk todo launch 1 > {} 2>&1", o("launch")));
    let launched = wait_for(&out.join("launch"));
    assert!(launched.trim().parse::<u32>().is_ok(), "{launched}");
    let worktree = dir.join("repo-fix-1-tab-rename-eats-the-last");
    assert_eq!(
        wait_for(&out.join("pwd")).trim(),
        worktree.to_str().unwrap()
    );
    let brief = wait_for(&out.join("brief"));
    assert!(
        brief.starts_with("Fix this bug from my backlog."),
        "{brief}"
    );
    assert!(
        brief.contains("# Tab rename eats the last letter\n"),
        "{brief}"
    );
    let kept = dir.join("state/valkyrie/backlog/files/0001/1-shot.png");
    assert!(brief.contains(&format!("- {}", kept.display())), "{brief}");
    assert_eq!(std::fs::read(&kept).unwrap(), b"not really a png");

    // The stand-in has exited: the item waits on review.
    let start = Instant::now();
    let list = loop {
        let _ = std::fs::remove_file(out.join("list"));
        human(&format!("valk todo list > {} 2>&1", o("list")));
        let list = wait_for(&out.join("list"));
        if list.contains("to review") || start.elapsed() > Duration::from_secs(10) {
            break list;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(list.contains("#1") && list.contains("to review"), "{list}");

    human(&format!("valk todo ship 1 > {} 2>&1", o("ship")));
    let ship = wait_for(&out.join("ship"));
    assert!(ship.contains("shipped"), "{ship}");
    assert!(
        ship.contains("and branch fix/1-tab-rename-eats-the-last"),
        "{ship}"
    );
    assert!(!worktree.exists());
    let branches = String::from_utf8(git(&["branch", "--list"]).stdout).unwrap();
    assert!(!branches.contains("fix/1"), "{branches}");
    drop(daemon);
}

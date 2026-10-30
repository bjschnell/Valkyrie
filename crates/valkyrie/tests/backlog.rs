//! The backlog with the real binary (DESIGN §8.12): an item written down with a
//! screenshot starts an agent in a worktree of its own with a brief for its kind,
//! and shipping it takes the clean worktree and merged branch away again. An agent
//! takes accepted items with their branch and pull request, which the user then
//! ships in one go once merged.
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_valk");
const PR: &str = "https://github.com/o/r/pull/7";

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
            "#!/bin/sh\nfor a; do last=$a; done\npwd > {p}\n\
             valk todo note 'fixed the off-by-one; check IME input' > {n}.tmp 2>&1; mv {n}.tmp {n}\n\
             valk todo note -i 2 'not mine' > {x}.tmp 2>&1; mv {x}.tmp {x}\n\
             valk todo edit 1 -t 'Done already' > {e}.tmp 2>&1; mv {e}.tmp {e}\n\
             valk todo take 3 -b fix/3-clients --pr {pr} > {t}.tmp 2>&1; mv {t}.tmp {t}\n\
             valk todo take 4 -b fix/4-open > {t4}.tmp 2>&1; mv {t4}.tmp {t4}\n\
             valk todo take 4 -b fix/3-clients > {r4}.tmp 2>&1; mv {r4}.tmp {r4}\n\
             valk todo take 1 -b fix/3-clients > {r1}.tmp 2>&1; mv {r1}.tmp {r1}\n\
             valk todo note -i 3 'merged upstream' > {tn}.tmp 2>&1; mv {tn}.tmp {tn}\n\
             valk todo ship 3 > {s}.tmp 2>&1; mv {s}.tmp {s}\n\
             valk todo ship --merged --yes > {sm}.tmp 2>&1; mv {sm}.tmp {sm}\n\
             printf '%s\\n' \"$last\" > {b}.tmp && mv {b}.tmp {b}\n",
            b = o("brief"),
            p = o("pwd"),
            n = o("note"),
            x = o("not-mine"),
            e = o("agent-edit"),
            t = o("take"),
            t4 = o("take4"),
            tn = o("take-note"),
            s = o("agent-ship"),
            sm = o("agent-ship-merged"),
            r4 = o("retake4"),
            r1 = o("retake1"),
            pr = PR,
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
    // `script` runs detached, so its output appears in `out/<name>` by a rename once
    // the command is done: printed line by line, a partial file can end in `\n` too.
    let human = |cmd: &str, name: &str| {
        let line = format!("{cmd} > {o}.tmp 2>&1; mv {o}.tmp {o}", o = o(name));
        let status = Command::new("setsid")
            .args(["-f", "script", "-qec", &line, "/dev/null"])
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
        assert!(status.success(), "{cmd}");
    };

    human(
        &format!(
            "valk todo add -k bug -f {} 'Tab rename eats the last letter'",
            shot.display()
        ),
        "add",
    );
    assert_eq!(
        wait_for(&out.join("add")),
        "#1 Tab rename eats the last letter (repo)\n"
    );

    human("valk todo add --no-folder 'Someone else'", "add2");
    wait_for(&out.join("add2"));
    // Reworded and made a chore; the body, left out, stays.
    human(
        "valk todo edit 2 -t 'Someone else, renamed' -k chore -b 'the detail'",
        "edit",
    );
    assert_eq!(
        wait_for(&out.join("edit")),
        "#2 Someone else, renamed [chore] (no folder)\n"
    );
    human("valk todo edit 2 -k bug", "edit2");
    assert_eq!(
        wait_for(&out.join("edit2")),
        "#2 Someone else, renamed [bug] (no folder)\n"
    );
    human("valk todo show 2", "show2");
    assert!(
        wait_for(&out.join("show2")).contains("\nthe detail\n"),
        "the body is kept"
    );
    // Accepted items an agent will take on outside a launch.
    human("valk todo add 'Clients stay connected'", "add3");
    wait_for(&out.join("add3"));
    human("valk todo add 'Still open'", "add4");
    wait_for(&out.join("add4"));
    human("valk todo take 3", "human-take");
    assert!(
        wait_for(&out.join("human-take")).contains("from inside a valk session"),
        "outside every session there's nothing to follow"
    );
    human("valk todo launch 1", "launch");
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
        human("valk todo list", "list");
        let list = wait_for(&out.join("list"));
        if list.contains("to review") || start.elapsed() > Duration::from_secs(10) {
            break list;
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    assert!(list.contains("#1") && list.contains("to review"), "{list}");

    // The agent's handoff landed on its own item, and only there.
    assert_eq!(
        wait_for(&out.join("note")),
        "noted on #1 Tab rename eats the last letter\n"
    );
    let edited = wait_for(&out.join("agent-edit"));
    assert!(edited.contains("is for the user"), "{edited}");
    let refused = wait_for(&out.join("not-mine"));
    assert!(
        refused.contains("only the item its session was launched for"),
        "{refused}"
    );
    human("valk todo show 1", "show");
    let show = wait_for(&out.join("show"));
    assert!(
        show.contains("fix/1-tab-rename-eats-the-last: 0 commits"),
        "{show}"
    );
    assert!(
        show.contains("note from claude:\nfixed the off-by-one; check IME input"),
        "{show}"
    );

    // The agent took #3 and #4 with their branches, and may note them, but
    // shipping stays the user's.
    assert_eq!(
        wait_for(&out.join("take")),
        format!("#3 Clients stay connected taken (branch fix/3-clients, PR {PR})\n")
    );
    assert!(wait_for(&out.join("take4")).contains("taken (branch fix/4-open)"));
    assert!(wait_for(&out.join("take-note")).starts_with("noted on #3"));
    let refused = wait_for(&out.join("agent-ship"));
    assert!(refused.contains("is for the user"), "{refused}");
    let refused = wait_for(&out.join("agent-ship-merged"));
    assert!(refused.contains("is for the user"), "{refused}");
    // Nor may it point an item at another branch, which ship merged would judge.
    let refused = wait_for(&out.join("retake4"));
    assert!(refused.contains("only the user changes that"), "{refused}");
    let refused = wait_for(&out.join("retake1"));
    assert!(refused.contains("its launch's branch"), "{refused}");
    human("valk todo show 3", "show3");
    let show = wait_for(&out.join("show3"));
    assert!(show.contains(&format!("\npr: {PR}\n")), "{show}");
    assert!(
        show.contains("\nbranch: fix/3-clients\n"),
        "no checkout: {show}"
    );
    // Shown, but marked: a take's note never goes into a brief.
    assert!(
        show.contains("note from claude (from a take):\nmerged upstream"),
        "{show}"
    );

    // #3's branch is merged; #4's has work that isn't; #1's worktree has nothing
    // of its own yet.
    for args in [
        &["checkout", "-q", "-b", "fix/3-clients"][..],
        &["commit", "-q", "--allow-empty", "-m", "keep clients"],
        &["checkout", "-q", "-b", "fix/4-open", "main"],
        &["commit", "-q", "--allow-empty", "-m", "wip"],
        &["checkout", "-q", "main"],
        &["merge", "-q", "--ff-only", "fix/3-clients"],
    ] {
        let done = git(args);
        assert!(done.status.success(), "{args:?}: {done:?}");
    }
    // First only what would ship; --yes ships it.
    let repo_at = std::fs::canonicalize(&repo).unwrap();
    human("valk todo ship --merged", "would-ship");
    let would = wait_for(&out.join("would-ship"));
    assert!(
        would.contains(&format!(
            "#3 Clients stay connected: would ship fix/3-clients (set by a take), merged into main at {}\n",
            repo_at.display()
        )),
        "{would}"
    );
    assert!(
        would.contains("1 may ship: `valk todo ship --merged --yes`"),
        "{would}"
    );
    human("valk todo show 3", "still-open");
    assert!(wait_for(&out.join("still-open")).contains(" · open · "));
    human("valk todo ship --merged --yes", "ship-merged");
    let merged = wait_for(&out.join("ship-merged"));
    assert!(
        merged.contains(&format!(
            "#3 Clients stay connected: shipped fix/3-clients (set by a take), merged into main at {}\n",
            repo_at.display()
        )),
        "{merged}"
    );
    assert!(
        merged.contains("#4 Still open: kept, not merged\n"),
        "{merged}"
    );
    assert!(
        merged.contains("#1 Tab rename eats the last letter: kept, no commits of its own\n"),
        "{merged}"
    );
    human("valk todo list --all", "list-all");
    let list = wait_for(&out.join("list-all"));
    let line = |id: &str| list.lines().find(|l| l.starts_with(id)).unwrap_or("");
    assert!(line("#3 ").contains("shipped"), "{list}");
    assert!(line("#3 ").ends_with(" (PR)"), "{list}");
    assert!(!line("#4 ").contains("shipped"), "{list}");
    assert!(worktree.exists(), "#1 stays as it was");

    human("valk todo ship 1", "ship");
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

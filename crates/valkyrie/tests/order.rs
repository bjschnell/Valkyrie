//! Build order with the real binary (ADR-0019 §2): three items landing in a
//! repository get a proposed order on their own, `valk todo plan` asks for one,
//! and accepting applies its priorities and after-links, all or some, as the
//! user's own edits; an item edited since keeps its edit.
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
            && !text.is_empty()
        {
            return text;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{} never written", path.display());
}

#[test]
fn a_build_order_is_proposed_and_applied_only_as_far_as_the_user_accepts_it() {
    if Command::new("script").arg("--version").output().is_err() {
        eprintln!("skipped: needs util-linux script for a human's terminal");
        return;
    }
    let dir = std::env::temp_dir().join(format!("valkyrie-order-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _daemon = Daemon { dir: dir.clone() };
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

    // The model: records each prompt, answers with one order. It links #3 after
    // #2 and #2 after #3, the second of which would loop: only the first stays.
    let model = bin.join("model");
    std::fs::write(
        &model,
        format!(
            "#!/bin/sh\ncat >> {calls}\necho '---' >> {calls}\n\
             printf '%s\\n' '{{\"order\":[\
             {{\"item\":1,\"reason\":\"the base\",\"priority\":\"p1\"}},\
             {{\"item\":3,\"reason\":\"needs #2\",\"after\":[2]}},\
             {{\"item\":2,\"reason\":\"builds on #1\",\"priority\":\"p3\",\"after\":[1,3]}}]}}'\n",
            calls = out.join("calls").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&model, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut n = 0;
    // As the user at a terminal, so the daemon doesn't take it for an agent.
    let mut human = |cmd: &str| -> String {
        n += 1;
        let o = out.join(format!("h{n}"));
        let line = format!("{cmd} > {o}.tmp 2>&1; mv {o}.tmp {o}", o = o.display());
        let status = Command::new("setsid")
            .args(["-f", "script", "-qec", &line, "/dev/null"])
            .current_dir(&repo)
            .env("PATH", &path)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("VALK_SOCKET", dir.join("run/v.sock"))
            .env("VALK_EXTRACTOR", model.to_str().unwrap())
            .env("VALK_ORDER_SETTLE_MS", "300")
            .env_remove("VALK_SESSION")
            .env_remove("VALK_CONTEXT")
            .env_remove("VALK_ITEM")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "{cmd}");
        wait_for(&o)
    };
    let items = |human: &mut dyn FnMut(&str) -> String| {
        let list: serde_json::Value =
            serde_json::from_str(&human("valk todo list --json")).unwrap();
        list.as_array()
            .unwrap()
            .iter()
            .map(|i| {
                let id = i["id"].as_u64().unwrap();
                let after: Vec<u64> = i["blocked_by"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_u64()).collect())
                    .unwrap_or_default();
                (id, (i["priority"].as_str().unwrap().to_owned(), after))
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };

    for title in ["Base API", "Client on the API", "Docs for the client"] {
        assert!(human(&format!("valk todo add '{title}'")).starts_with('#'));
    }
    // Three in a day: the watcher asks on its own.
    let start = Instant::now();
    let mut shown = String::new();
    while start.elapsed() < Duration::from_secs(15) && !shown.contains("Build order") {
        std::thread::sleep(Duration::from_millis(200));
        shown = human("valk todo plan --show");
    }
    assert!(shown.contains("3+ items added within a day"), "{shown}");
    assert!(
        shown.contains("#1") && shown.contains("change: p2 → p1"),
        "{shown}"
    );
    assert!(shown.contains("why: builds on #1"), "{shown}");
    let calls = std::fs::read_to_string(out.join("calls")).unwrap();
    assert!(
        calls.contains("## #2 [p2 · feature] Client on the API"),
        "{calls}"
    );
    assert!(human("valk todo list").contains("a build order waits"));
    // Nothing changed yet.
    let before = items(&mut human);
    assert!(
        before.values().all(|(p, a)| p == "p2" && a.is_empty()),
        "{before:?}"
    );

    // Only #2 and #3's changes.
    let said = human("valk todo plan --accept 2 3");
    assert!(said.contains("changed #3, #2"), "{said}");
    let after = items(&mut human);
    assert_eq!(after[&1], ("p2".into(), vec![]));
    assert_eq!(after[&3], ("p2".into(), vec![2]));
    // #3 → #2 would loop with #3's new link: only #1 is added.
    assert_eq!(after[&2], ("p3".into(), vec![1]));
    assert!(human("valk todo plan --show").contains("no build order waits"));

    // Asked for, then #1 edited meanwhile: #1 keeps the edit.
    let asked = human("valk todo plan");
    assert!(
        asked.contains("Build order for repo (you asked)"),
        "{asked}"
    );
    std::thread::sleep(Duration::from_millis(1100));
    human("valk todo edit 1 -p p0");
    let said = human("valk todo plan --accept");
    assert!(said.contains("left #1 as it is: edited since"), "{said}");
    assert_eq!(items(&mut human)[&1].0, "p0");

    // Declined: nothing changes. Off: no model is asked.
    human("valk todo plan");
    assert!(human("valk todo plan --decline").contains("declined"));
    human("valk decisions auto off");
    let refused = human("valk todo plan");
    assert!(refused.contains("auto off"), "{refused}");
    assert_eq!(
        std::fs::read_to_string(out.join("calls"))
            .unwrap()
            .matches("---")
            .count(),
        3,
        "the watcher once, then two asks"
    );
}

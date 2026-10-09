//! ADR-0007 §4 with the real binary: an agent in a session can only propose
//! decisions, however it goes about it, and only a human can accept them.
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
fn agents_only_propose_and_never_review() {
    if Command::new("script").arg("--version").output().is_err() {
        eprintln!("skipped: needs util-linux script for a human's terminal");
        return;
    }
    let dir = std::env::temp_dir().join(format!("valkyrie-decisions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let daemon = Daemon { dir: dir.clone() };
    let (bin, repo, out) = (dir.join("bin"), dir.join("repo"), dir.join("out"));
    for d in [&bin, &repo.join(".git"), &out] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::os::unix::fs::symlink(BIN, bin.join("valk")).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let o = |name: &str| out.join(name).display().to_string();

    // A fake agent: Valkyrie knows it by its program name. It tries each way past
    // the review, each writing what valk said to a file.
    let agent = bin.join("claude");
    std::fs::write(
        &agent,
        format!(
            "#!/bin/sh\n\
             cd {repo}\n\
             valk decide 'Agent idea' 'from the agent' > {a} 2>&1\n\
             valk decisions accept 1 > {s} 2>&1\n\
             env -u VALK_SESSION valk decide 'No env' > {e} 2>&1\n\
             (valk decide 'Forked' > {f} 2>&1 &)\n\
             valk new --cwd {repo} -- sh -c \"valk decide 'Laundered' > {l} 2>&1; sleep 30\" > /dev/null\n\
             valk web pair > {p} 2>&1\n\
             echo done > {d}\n\
             sleep 30\n",
            repo = repo.display(),
            a = o("agent"),
            s = o("self-accept"),
            e = o("no-env"),
            f = o("forked"),
            l = o("laundered"),
            p = o("pair"),
            d = o("agent-done"),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&agent, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

    // Run as a human: on a terminal of its own, outside every session. Detached
    // (`setsid -f`), because the test itself may run under an agent, which would
    // rightly make everything it starts count as that agent's. (The same move by
    // an agent gets past the check; ADR-0007 §4 says why that's the agent's
    // sandbox's job.) `line` ends by writing a file, which is how we wait for it.
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

    human(&format!(
        "valk new --cwd {} -- {} > {}",
        repo.display(),
        agent.display(),
        o("spawned")
    ));
    wait_for(&out.join("agent-done"));
    let read = |name: &str| wait_for(&out.join(name));
    assert!(read("agent").contains("#1 proposed"), "{}", read("agent"));
    assert!(
        read("self-accept").contains("is for the user"),
        "{}",
        read("self-accept")
    );
    assert!(read("no-env").contains("proposed"), "{}", read("no-env"));
    assert!(read("forked").contains("proposed"), "{}", read("forked"));
    assert!(
        read("laundered").contains("proposed"),
        "{}",
        read("laundered")
    );
    assert!(read("pair").contains("is for the user"), "{}", read("pair"));

    // A human's own decision is active at once, and a human can accept.
    human(&format!(
        "valk decide 'Human rule' 'mine' > {} 2>&1",
        o("human")
    ));
    assert!(read("human").contains("recorded"), "{}", read("human"));
    human(&format!("valk decisions accept 1 > {} 2>&1", o("accept")));
    assert!(read("accept").contains("#1 active"), "{}", read("accept"));

    // What an agent gets at its next session start: active ones only.
    human(&format!(
        "echo '{{\"cwd\":\"{}\"}}' | VALK_SESSION=1 VALK_CONTEXT={} valk context-hook claude > {}",
        repo.display(),
        dir.join("state/valkyrie/context").display(),
        o("hook")
    ));
    let hook: serde_json::Value = serde_json::from_str(&read("hook")).unwrap();
    let context = hook["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("Human rule") && context.contains("Agent idea"),
        "{context}"
    );
    assert!(
        !context.contains("Forked") && !context.contains("Laundered"),
        "{context}"
    );
    drop(daemon);
}

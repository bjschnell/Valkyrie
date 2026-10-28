//! The context layer with the real binary (DESIGN §6, ADR-0007): an agent can only
//! propose decisions, corrections and failures become proposals, agents hear about
//! each other, and handoffs.
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
        "echo '{{\"hook_event_name\":\"SessionStart\",\"cwd\":\"{}\"}}' | VALK_SESSION=1 VALK_CONTEXT={} valk context-hook claude > {}",
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

/// DESIGN §6.3: a correction to an agent is read by the model (a stand-in here)
/// once, and what it finds is proposed, not made active.
#[test]
fn corrections_become_proposals() {
    let dir = std::env::temp_dir().join(format!("valkyrie-extract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let daemon = Daemon { dir: dir.clone() };
    let (bin, repo, out) = (dir.join("bin"), dir.join("repo"), dir.join("out"));
    for d in [&bin, &repo.join(".git"), &out] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::create_dir_all(dir.join("run")).unwrap();
    let exec: std::fs::Permissions = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(
        dir.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    std::os::unix::fs::symlink(BIN, bin.join("valk")).unwrap();

    // The model: records each prompt, answers with one decision.
    let model = bin.join("model");
    std::fs::write(
        &model,
        format!(
            "#!/bin/sh\ncat >> {calls}\necho '---' >> {calls}\n\
             echo '{{\"result\":\"{{\\\"decisions\\\":[{{\\\"kind\\\":\\\"constraint\\\",\\\"title\\\":\\\"Use pnpm, never npm\\\",\\\"body\\\":\\\"The repo is pnpm.\\\"}}]}}\"}}'\n",
            calls = out.join("calls").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&model, exec.clone()).unwrap();

    // The agent: a transcript where you corrected it, then its hooks.
    let transcript = out.join("t.jsonl");
    let line = |v: serde_json::Value| v.to_string();
    let lines = [
        line(
            serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"I'll run npm install."}]}}),
        ),
        line(
            serde_json::json!({"type":"user","message":{"content":"no, we use pnpm here, never npm"}}),
        ),
        line(
            serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":"Switching to pnpm."}]}}),
        ),
    ];
    std::fs::write(&transcript, lines.join("\n") + "\n").unwrap();
    let hook = |event: &str| {
        format!(
            "echo '{{\"hook_event_name\":\"{event}\",\"source\":\"startup\",\"session_id\":\"c1\",\"transcript_path\":\"{}\"}}' | valk hook claude\n",
            transcript.display()
        )
    };
    let agent = bin.join("claude");
    std::fs::write(
        &agent,
        format!(
            "#!/bin/sh\n{}sleep 0.3\n{}sleep 0.3\n{}sleep 30\n",
            hook("SessionStart"),
            hook("Stop"),
            // The same message ending another turn isn't read again.
            hook("Stop"),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&agent, exec).unwrap();

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let valk = |args: &[&str]| {
        let out = Command::new(BIN)
            .args(args)
            .current_dir(&repo)
            .env("PATH", &path)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("VALK_SOCKET", dir.join("run/v.sock"))
            .env("VALK_EXTRACTOR", model.to_str().unwrap())
            .env_remove("VALK_SESSION")
            .env_remove("VALK_CONTEXT")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    valk(&[
        "new",
        "--cwd",
        repo.to_str().unwrap(),
        "--",
        agent.to_str().unwrap(),
    ]);
    let start = Instant::now();
    let mut listed = String::new();
    while start.elapsed() < Duration::from_secs(10) && !listed.contains("pnpm") {
        std::thread::sleep(Duration::from_millis(100));
        listed = valk(&["decisions"]);
    }
    assert!(
        listed.contains("#1  constraint Use pnpm, never npm (proposed)"),
        "{listed}"
    );
    let shown = valk(&["decisions", "show", "1"]);
    assert!(
        shown.contains("by: valkyrie")
            && shown.contains("From your message: “no, we use pnpm here, never npm”"),
        "{shown}"
    );
    std::thread::sleep(Duration::from_millis(1500));
    let calls = std::fs::read_to_string(out.join("calls")).unwrap();
    assert_eq!(calls.matches("---").count(), 1, "{calls}");
    assert!(
        calls.contains("Developer: no, we use pnpm here, never npm"),
        "{calls}"
    );
    assert!(calls.contains("Agent: I'll run npm install."), "{calls}");

    assert!(valk(&["decisions", "auto"]).starts_with("claude:"));
    // Only the user changes it; this test may run under an agent, so it's either
    // changed or refused as not the user's.
    let out = Command::new(BIN)
        .args(["decisions", "auto", "off"])
        .current_dir(&repo)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("VALK_SOCKET", dir.join("run/v.sock"))
        .env_remove("VALK_SESSION")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
    assert!(
        said.starts_with("off:") || said.contains("is for the user"),
        "{said}"
    );
    drop(daemon);
}

/// ADR-0010: a turn that ended after a tool failed gets a retro. The failed call
/// and what worked next become a proposal, its signature is recorded, and a turn
/// with no new failure isn't read again.
#[test]
fn failures_become_proposals() {
    let dir = std::env::temp_dir().join(format!("valkyrie-retro-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let daemon = Daemon { dir: dir.clone() };
    let (bin, repo, out) = (dir.join("bin"), dir.join("repo"), dir.join("out"));
    for d in [&bin, &repo.join(".git"), &out] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::create_dir_all(dir.join("run")).unwrap();
    let exec: std::fs::Permissions = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(
        dir.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    std::os::unix::fs::symlink(BIN, bin.join("valk")).unwrap();

    let model = bin.join("model");
    std::fs::write(
        &model,
        format!(
            "#!/bin/sh\ncat >> {calls}\necho '---' >> {calls}\n\
             echo '{{\"result\":\"{{\\\"decisions\\\":[{{\\\"moment\\\":1,\\\"kind\\\":\\\"gotcha\\\",\\\"title\\\":\\\"Kill exact pids, never pkill -f\\\",\\\"body\\\":\\\"pkill -f matches the agent shell.\\\"}}]}}\"}}'\n",
            calls = out.join("calls").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&model, exec.clone()).unwrap();

    // The agent's transcript: `pkill -f` failed, `kill <pid>` worked.
    let transcript = out.join("t.jsonl");
    let call = |id: &str, command: &str| {
        serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":command}}]}}).to_string()
    };
    let result = |id: &str, error: Option<&str>| {
        serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id,"is_error":error.is_some(),"content":error.unwrap_or("ok")}]}}).to_string()
    };
    let lines = [
        call("1", "pkill -f 'valk daemon'"),
        result("1", Some("Exit code 144\nTerminated")),
        call("2", "kill 4242"),
        result("2", None),
    ];
    std::fs::write(&transcript, lines.join("\n") + "\n").unwrap();
    let hook = |event: &str| {
        format!(
            "echo '{{\"hook_event_name\":\"{event}\",\"source\":\"startup\",\"session_id\":\"c1\",\"tool_name\":\"Bash\",\"transcript_path\":\"{}\"}}' | valk hook claude\n",
            transcript.display()
        )
    };
    let agent = bin.join("claude");
    std::fs::write(
        &agent,
        format!(
            "#!/bin/sh\n{}sleep 0.3\n{}sleep 0.3\n{}sleep 0.3\n{}sleep 30\n",
            hook("SessionStart"),
            hook("PostToolUseFailure"),
            hook("Stop"),
            // No failure since: no retro.
            hook("Stop"),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&agent, exec).unwrap();

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let valk = |args: &[&str]| {
        let out = Command::new(BIN)
            .args(args)
            .current_dir(&repo)
            .env("PATH", &path)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("VALK_SOCKET", dir.join("run/v.sock"))
            .env("VALK_EXTRACTOR", model.to_str().unwrap())
            .env_remove("VALK_SESSION")
            .env_remove("VALK_CONTEXT")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    valk(&[
        "new",
        "--cwd",
        repo.to_str().unwrap(),
        "--",
        agent.to_str().unwrap(),
    ]);
    let start = Instant::now();
    let mut listed = String::new();
    while start.elapsed() < Duration::from_secs(10) && !listed.contains("pkill") {
        std::thread::sleep(Duration::from_millis(100));
        listed = valk(&["decisions"]);
    }
    assert!(
        listed.contains("#1  gotcha     Kill exact pids, never pkill -f (proposed)"),
        "{listed}"
    );
    let shown = valk(&["decisions", "show", "1"]);
    assert!(
        shown.contains("by: valkyrie")
            && shown.contains("From a failure: `pkill -f 'valk daemon'` → Terminated"),
        "{shown}"
    );
    std::thread::sleep(Duration::from_millis(1500));
    let calls = std::fs::read_to_string(out.join("calls")).unwrap();
    assert_eq!(calls.matches("---").count(), 1, "{calls}");
    assert!(
        calls.contains("Call: pkill -f 'valk daemon'")
            && calls.contains("Next that worked: kill 4242")
            && calls.contains("seen_before=\"0\""),
        "{calls}"
    );
    let state = dir.join("state/valkyrie/context");
    let sigs = std::fs::read_dir(&state)
        .unwrap()
        .flatten()
        .map(|e| e.path().join("failures.jsonl"))
        .find(|p| p.exists())
        .expect("failures recorded");
    let sigs = std::fs::read_to_string(sigs).unwrap();
    assert_eq!(sigs.lines().count(), 2, "{sigs}");
    assert!(
        sigs.contains("\"call\":\"1\"") && sigs.contains("\"decision\":1"),
        "{sigs}"
    );
    drop(daemon);
}

/// ADR-0010 §3–5: a skill is listed by title, read in full, rewritten when a
/// session that followed it fails, distilled from a session on request, and
/// exported where Claude Code loads it.
#[test]
fn skills_are_indexed_patched_learned_and_exported() {
    if Command::new("script").arg("--version").output().is_err() {
        eprintln!("skipped: needs util-linux script for a human's terminal");
        return;
    }
    let dir = std::env::temp_dir().join(format!("valkyrie-skills-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let daemon = Daemon { dir: dir.clone() };
    let (bin, repo, out) = (dir.join("bin"), dir.join("repo"), dir.join("out"));
    for d in [&bin, &repo.join(".git"), &out, &dir.join("agent")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::create_dir_all(dir.join("run")).unwrap();
    let exec: std::fs::Permissions = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(
        dir.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    std::os::unix::fs::symlink(BIN, bin.join("valk")).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // The model: the retro's reply first (a rewrite of skill #1), then learn's.
    let model = bin.join("model");
    let calls = out.join("calls");
    std::fs::write(
        &model,
        format!(
            "#!/bin/sh\ncat >> {calls}\necho '---' >> {calls}\n\
             if [ $(grep -c -- --- {calls}) = 1 ]; then\n\
             echo '{{\"result\":\"{{\\\"decisions\\\":[{{\\\"moment\\\":1,\\\"patch\\\":1,\\\"title\\\":\\\"Cut a release\\\",\\\"body\\\":\\\"1. Set the upstream.\\\\n2. Push the tag.\\\"}}]}}\"}}'\n\
             else\n\
             echo '{{\"result\":\"{{\\\"decisions\\\":[{{\\\"kind\\\":\\\"skill\\\",\\\"title\\\":\\\"Verify a release build\\\",\\\"body\\\":\\\"1. Build.\\\"}}]}}\"}}'\n\
             fi\n",
            calls = calls.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&model, exec.clone()).unwrap();

    // The agent read skill #1, then `git push --tags` failed and a push with
    // the upstream set worked.
    let transcript = out.join("t.jsonl");
    let call = |id: &str, command: &str| {
        serde_json::json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":id,"name":"Bash","input":{"command":command}}]}}).to_string()
    };
    let result = |id: &str, error: Option<&str>| {
        serde_json::json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id,"is_error":error.is_some(),"content":error.unwrap_or("ok")}]}}).to_string()
    };
    let lines = [
        serde_json::json!({"type":"user","message":{"content":"cut the release"}}).to_string(),
        call("1", "valk decisions show 1"),
        result("1", None),
        call("2", "git push --tags"),
        result("2", Some("Exit code 128\nfatal: no upstream branch")),
        call("3", "git push -u origin main --tags"),
        result("3", None),
    ];
    std::fs::write(&transcript, lines.join("\n") + "\n").unwrap();
    let hook = |event: &str| {
        format!(
            "echo '{{\"hook_event_name\":\"{event}\",\"source\":\"startup\",\"session_id\":\"c1\",\"tool_name\":\"Bash\",\"transcript_path\":\"{}\"}}' | valk hook claude\n",
            transcript.display()
        )
    };
    let agent = dir.join("agent/claude");
    std::fs::write(
        &agent,
        format!(
            "#!/bin/sh\n{}sleep 0.3\n{}sleep 0.3\n{}sleep 30\n",
            hook("SessionStart"),
            hook("PostToolUseFailure"),
            hook("Stop"),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&agent, exec).unwrap();

    let env = |c: &mut Command| {
        c.current_dir(&repo)
            .env("PATH", &path)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("VALK_SOCKET", dir.join("run/v.sock"))
            .env("VALK_EXTRACTOR", model.to_str().unwrap())
            .env_remove("VALK_SESSION")
            .env_remove("VALK_CONTEXT");
    };
    let valk = |args: &[&str]| {
        let mut c = Command::new(BIN);
        c.args(args);
        env(&mut c);
        let out = c.output().unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    // The skill itself, from a human: active at once.
    let mut human = Command::new("setsid");
    human.args([
        "-f",
        "script",
        "-qec",
        &format!(
            "printf '1. Push the tag.\\n' | valk decide --kind skill 'Cut a release' - > {} 2>&1",
            out.join("human").display()
        ),
        "/dev/null",
    ]);
    env(&mut human);
    human
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    assert!(human.status().unwrap().success());
    assert!(wait_for(&out.join("human")).contains("#1 recorded"));

    // Listed by title for agents, its steps a `decisions show` away.
    let preview = valk(&["decisions", "preview"]);
    assert!(
        preview.contains("decisions show <id>`:\n- #1 Cut a release\n")
            && !preview.contains("Push the tag"),
        "{preview}"
    );

    let id = valk(&[
        "new",
        "--cwd",
        repo.to_str().unwrap(),
        "--",
        agent.to_str().unwrap(),
    ]);
    let id = id.trim();
    let start = Instant::now();
    let mut listed = String::new();
    while start.elapsed() < Duration::from_secs(10) && !listed.contains("#2") {
        std::thread::sleep(Duration::from_millis(100));
        listed = valk(&["decisions"]);
    }
    assert!(
        listed.contains("#2  skill      Cut a release (proposed)"),
        "{listed}"
    );
    let shown = valk(&["decisions", "show", "2"]);
    assert!(
        shown.contains("supersedes: 1")
            && shown.contains("1. Set the upstream.\n2. Push the tag.")
            && shown.contains(
                "Rewritten after a failure: `git push --tags` → fatal: no upstream branch"
            ),
        "{shown}"
    );
    let retro = std::fs::read_to_string(&calls).unwrap();
    assert!(
        retro.contains("<skill number=\"1\" title=\"Cut a release\">\n1. Push the tag.\n</skill>"),
        "{retro}"
    );

    let learned = valk(&["decisions", "learn", id]);
    assert!(
        learned.contains("#3 proposed: Verify a release build"),
        "{learned}"
    );
    let calls = std::fs::read_to_string(&calls).unwrap();
    let learn = calls.split("---\n").nth(1).unwrap();
    assert!(
        learn.contains("You: cut the release\n> Bash: valk decisions show 1\n")
            && learn.contains("! (exit 128) | fatal: no upstream branch\n"),
        "{learn}"
    );
    assert!(valk(&["decisions", "show", "3"]).contains("Learned from the session"));

    valk(&["decisions", "export"]);
    let exported =
        std::fs::read_to_string(repo.join(".claude/skills/cut-a-release/SKILL.md")).unwrap();
    assert!(
        exported.starts_with(
            "---\nname: cut-a-release\ndescription: \"Cut a release\"\n---\n\n1. Push the tag.\n"
        ),
        "{exported}"
    );
    drop(daemon);
}

/// DESIGN §6.5: an agent hears, with its next prompt, what the other agents in the
/// same repository are doing, once, and which files they both edited.
#[test]
fn agents_hear_about_their_siblings() {
    let dir = std::env::temp_dir().join(format!("valkyrie-siblings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let daemon = Daemon { dir: dir.clone() };
    let (bin, repo, out) = (dir.join("bin"), dir.join("repo"), dir.join("out"));
    for d in [&bin, &repo.join(".git"), &out] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::set_permissions(
        dir.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    std::os::unix::fs::symlink(BIN, bin.join("valk")).unwrap();
    let repo_s = repo.display().to_string();
    let hook = |payload: serde_json::Value| format!("echo '{payload}' | valk hook claude\n");
    let edit = |file: &str| {
        hook(
            serde_json::json!({"hook_event_name":"PostToolUse","tool_name":"Edit","cwd":repo_s,
                                "tool_input":{"file_path":format!("{repo_s}/{file}")}}),
        )
    };
    let start =
        hook(serde_json::json!({"hook_event_name":"SessionStart","source":"startup","cwd":repo_s}));
    let ask = |name: &str| {
        let payload =
            serde_json::json!({"hook_event_name":"UserPromptSubmit","prompt":"go on","cwd":repo_s});
        format!(
            "echo '{payload}' | valk context-hook claude > {}\n",
            out.join(name).display()
        )
    };
    // Two agents: Valkyrie knows them by program name, so each is `claude` in its
    // own directory.
    let script = |name: &str, body: String| {
        let d = bin.join(name);
        std::fs::create_dir_all(&d).unwrap();
        let path = d.join("claude");
        std::fs::write(&path, format!("#!/bin/sh\n{body}sleep 30\n")).unwrap();
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        path
    };
    // `a` says when its hooks are in, and `b` asks only then: a fixed sleep lost
    // the race on a loaded CI runner, where each `valk hook` starts slowly.
    let a_ready = out.join("a-ready");
    let a = script(
        "a",
        format!(
            "{start}{}{}touch {}\n",
            hook(
                serde_json::json!({"hook_event_name":"UserPromptSubmit","prompt":"refactor the auth module","cwd":repo_s})
            ),
            edit("src/auth.rs") + &edit("src/db.rs"),
            a_ready.display(),
        ),
    );
    let b = script(
        "b",
        format!(
            "{start}{}while [ ! -e {} ]; do sleep 0.1; done\n{}{}echo done > {}\n",
            edit("src/auth.rs"),
            a_ready.display(),
            ask("first"),
            ask("second"),
            out.join("b-done").display()
        ),
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let valk = |args: &[&str]| {
        let out = Command::new(BIN)
            .args(args)
            .current_dir(&repo)
            .env("PATH", &path)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("VALK_SOCKET", dir.join("run/v.sock"))
            .env_remove("VALK_SESSION")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    // Started as a human would (see agents_only_propose_and_never_review): a session
    // an agent started doesn't have its prompts repeated.
    let human_new = |name: &str, script: &Path| {
        let status = Command::new("setsid")
            .args(["-f", "script", "-qec"])
            .arg(format!(
                "valk new --name {name} --cwd {repo_s} -- {} > /dev/null",
                script.display()
            ))
            .arg("/dev/null")
            .current_dir(&repo)
            .env("PATH", &path)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("VALK_SOCKET", dir.join("run/v.sock"))
            .env_remove("VALK_SESSION")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    };
    valk(&["ls"]);
    human_new("auth-work", &a);
    std::thread::sleep(Duration::from_millis(300));
    human_new("other", &b);
    wait_for(&out.join("b-done"));
    let first: serde_json::Value = serde_json::from_str(&wait_for(&out.join("first"))).unwrap();
    assert_eq!(
        first["hookSpecificOutput"]["hookEventName"],
        "UserPromptSubmit"
    );
    let text = first["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(text.contains("- auth-work (claude, "), "{text}");
    assert!(
        text.contains("asked “refactor the auth module”; editing src/db.rs, src/auth.rs"),
        "{text}"
    );
    assert!(
        text.contains("You have both edited: src/auth.rs (auth-work)"),
        "{text}"
    );
    assert!(!text.contains("- other"), "{text}");
    // Nothing new since: nothing said.
    assert_eq!(std::fs::read_to_string(out.join("second")).unwrap(), "");
    drop(daemon);
}

/// DESIGN §6.5: `valk handoff` resumes a session from its transcript, with the
/// model's summary, and `--to` starts an agent on it.
#[test]
fn handoff_resumes_a_session_for_another_agent() {
    let dir = std::env::temp_dir().join(format!("valkyrie-handoff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let daemon = Daemon { dir: dir.clone() };
    let (bin, repo, out) = (dir.join("bin"), dir.join("repo"), dir.join("out"));
    for d in [&bin, &repo.join(".git"), &out] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::set_permissions(
        dir.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    std::os::unix::fs::symlink(BIN, bin.join("valk")).unwrap();
    let exe = |path: &Path, body: String| {
        std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    };
    let transcript = out.join("t.jsonl");
    let lines = [
        serde_json::json!({"type":"user","message":{"content":"Add OAuth login"}}),
        serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"text","text":"Callback route done."},
            {"type":"tool_use","name":"Edit","input":{"file_path":format!("{}/src/auth.rs", repo.display())}}]}}),
    ];
    std::fs::write(
        &transcript,
        lines
            .iter()
            .map(|l| l.to_string() + "\n")
            .collect::<String>(),
    )
    .unwrap();
    // The session's agent: names its transcript, then waits. Started by --to, it
    // saves the message it was started with (its last argument).
    exe(
        &bin.join("claude"),
        format!(
            "for a in \"$@\"; do last=$a; done\n\
             case \"$last\" in Read*) printf '%s' \"$last\" > {got};; esac\n\
             echo '{{\"hook_event_name\":\"SessionStart\",\"source\":\"startup\",\"session_id\":\"x\",\"transcript_path\":\"{t}\"}}' | valk hook claude\n\
             sleep 30\n",
            got = out.join("got").display(),
            t = transcript.display()
        ),
    );
    let model = bin.join("model");
    exe(
        &model,
        "cat > /dev/null\necho '- Callback done; next: logout.'\n".into(),
    );
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let valk = |args: &[&str], summary: bool| {
        let mut cmd = Command::new(BIN);
        cmd.args(args)
            .current_dir(&repo)
            .env("PATH", &path)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("VALK_SOCKET", dir.join("run/v.sock"))
            .env_remove("VALK_SESSION");
        if summary {
            cmd.env("VALK_EXTRACTOR", &model);
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    let id = valk(
        &[
            "new",
            "--name",
            "web",
            "--",
            bin.join("claude").to_str().unwrap(),
        ],
        false,
    );
    let id = id.trim();
    std::thread::sleep(Duration::from_millis(500));

    let plain = valk(&["handoff", id, "--no-summary"], false);
    assert!(plain.starts_with("# Handoff from web (claude, "), "{plain}");
    assert!(
        plain.contains("## Goal (the first ask)\nAdd OAuth login"),
        "{plain}"
    );
    assert!(
        plain.contains("## Where it left off\nCallback route done."),
        "{plain}"
    );
    assert!(
        plain.contains("## Files it changed\n- src/auth.rs"),
        "{plain}"
    );
    let summarized = valk(&["handoff", id], true);
    assert!(
        summarized.contains("## Where it left off\n- Callback done; next: logout."),
        "{summarized}"
    );

    valk(&["handoff", id, "--no-summary", "--to", "claude"], false);
    let got = wait_for_any(&out.join("got"));
    let file = got
        .strip_prefix("Read the handoff in ")
        .and_then(|rest| rest.strip_suffix(" and carry on that work."))
        .unwrap_or_else(|| panic!("{got}"));
    let saved = std::fs::read_to_string(file).unwrap();
    assert!(saved.starts_with("# Handoff from web"), "{saved}");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(daemon);
}

/// Waits for a file to exist and hold something.
fn wait_for_any(path: &Path) -> String {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.is_empty()
        {
            return text;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("{} never written", path.display());
}

/// DESIGN §6.4: a decision about a file that's since been rewritten is flagged for
/// review, and saying it still holds clears that.
#[test]
fn decisions_about_rewritten_files_are_flagged() {
    if Command::new("script").arg("--version").output().is_err() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("valkyrie-stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let daemon = Daemon { dir: dir.clone() };
    let (bin, repo, out) = (dir.join("bin"), dir.join("repo"), dir.join("out"));
    for d in [&bin, &repo.join("src"), &out] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::set_permissions(
        dir.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    std::os::unix::fs::symlink(BIN, bin.join("valk")).unwrap();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(&repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(repo.join("src/auth.rs"), "fn a() {}\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "one"]);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // A human, as in agents_only_propose_and_never_review.
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
    let o = |name: &str| out.join(name).display().to_string();
    human(&format!(
        "valk decide -k pattern 'Keep src/auth.rs free of IO' 'Pure logic only.' > {} 2>&1",
        o("decide")
    ));
    assert!(wait_for(&out.join("decide")).contains("#1 recorded"));
    human(&format!("valk decisions show 1 > {} 2>&1", o("show")));
    assert!(
        wait_for(&out.join("show")).contains("anchors: src/auth.rs"),
        "{}",
        wait_for(&out.join("show"))
    );
    human(&format!("valk decisions health > {} 2>&1", o("before")));
    assert!(
        wait_for(&out.join("before")).contains("healthy"),
        "{}",
        wait_for(&out.join("before"))
    );

    std::fs::write(repo.join("src/auth.rs"), "fn b() {}\n".repeat(300)).unwrap();
    git(&["commit", "-qam", "rewrite"]);
    human(&format!("valk decisions health > {} 2>&1", o("after")));
    let after = wait_for(&out.join("after"));
    assert!(
        after.contains(
            "#1 may be out of date: src/auth.rs changed a lot since it was confirmed (+300/-1)"
        ),
        "{after}"
    );
    human(&format!(
        "valk decisions confirm 1 > {} 2>&1 && valk decisions health > {} 2>&1",
        o("confirm"),
        o("cleared")
    ));
    let cleared = wait_for(&out.join("cleared"));
    assert!(cleared.contains("healthy"), "{cleared}");
    drop(daemon);
}

/// DESIGN §6.5: `valk mcp` speaks MCP over stdio, and what an agent proposes
/// through it waits on review.
#[test]
fn mcp_serves_and_proposes_decisions() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("valkyrie-mcp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let daemon = Daemon { dir: dir.clone() };
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::set_permissions(
        dir.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let mut child = Command::new(BIN)
        .arg("mcp")
        .current_dir(&repo)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("VALK_SOCKET", dir.join("run/v.sock"))
        .env_remove("VALK_SESSION")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let requests = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"propose_decision","arguments":{"title":"Use pnpm","body":"npm breaks the lockfile.","kind":"constraint"}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_decisions","arguments":{"query":"pnpm","include_inactive":true}}}"#,
        r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_decision","arguments":{"id":9}}}"#,
        r#"{"jsonrpc":"2.0","id":6,"method":"nope"}"#,
    ];
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let replies: Vec<serde_json::Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(replies.len(), 6, "{replies:?}");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "valkyrie");
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 5);
    let text = |i: usize| {
        replies[i]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(text(2).starts_with("Proposed #1: Use pnpm"), "{}", text(2));
    // A proposal isn't shown to agents until the user reviews it.
    assert!(!text(3).contains("Use pnpm"), "{}", text(3));
    assert_eq!(replies[4]["result"]["isError"], true);
    assert_eq!(replies[5]["error"]["code"], -32601);
    drop(daemon);
}

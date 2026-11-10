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

/// Decision `id` as the daemon stored it. `valk decisions show` withholds a
/// proposal's body from agents, and a test may run under one.
fn stored(dir: &Path, repo: &Path, id: u32) -> String {
    let store = valkyrie_context::Store::new(dir.join("state/valkyrie/context"));
    let all = store.load(&valkyrie_context::project::root(repo));
    let d = all.iter().find(|d| d.id == id);
    valkyrie_context::render(d.unwrap_or_else(|| panic!("no decision #{id}: {all:?}")))
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
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
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
             valk decisions show 1 > {shown} 2>&1\n\
             env -u VALK_SESSION valk decide 'No env' > {e} 2>&1\n\
             (valk decide 'Forked' > {f} 2>&1 &)\n\
             valk new --cwd {repo} -- sh -c \"valk decide 'Laundered' > {l} 2>&1; sleep 30\" > /dev/null\n\
             valk web pair > {p} 2>&1\n\
             echo done > {d}\n\
             sleep 30\n",
            repo = repo.display(),
            a = o("agent"),
            s = o("self-accept"),
            shown = o("agent-show"),
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
    // An agent sees what waits on the user, never the unreviewed text; the user does.
    let shown = read("agent-show");
    assert!(
        shown.contains("# Agent idea")
            && shown.contains("isn't shown to agents")
            && !shown.contains("from the agent"),
        "{shown}"
    );
    human(&format!("valk decisions show 1 > {} 2>&1", o("human-show")));
    assert!(
        read("human-show").contains("from the agent"),
        "{}",
        read("human-show")
    );

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
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
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
             printf '%s\\n' '{{\"result\":\"{{\\\"decisions\\\":[{{\\\"kind\\\":\\\"constraint\\\",\\\"title\\\":\\\"Use pnpm, never npm\\\",\\\"body\\\":\\\"The repo is pnpm.\\\"}}]}}\"}}'\n",
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
    let shown = stored(&dir, &repo, 1);
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

/// ADR-0011: a turn that ended after a tool failed gets a retro. The failed call
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
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
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
             printf '%s\\n' '{{\"result\":\"{{\\\"decisions\\\":[{{\\\"moment\\\":1,\\\"kind\\\":\\\"gotcha\\\",\\\"title\\\":\\\"Kill exact pids, never pkill -f\\\",\\\"body\\\":\\\"pkill -f matches the agent shell.\\\"}}]}}\"}}'\n",
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
    let shown = stored(&dir, &repo, 1);
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

/// ADR-0011 §3–5: a skill is listed by title, read in full, rewritten when a
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
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
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
             printf '%s\\n' '{{\"result\":\"{{\\\"decisions\\\":[{{\\\"moment\\\":1,\\\"patch\\\":1,\\\"title\\\":\\\"Cut a release\\\",\\\"body\\\":\\\"1. Set the upstream.\\\\n2. Push the tag.\\\"}}]}}\"}}'\n\
             elif [ $(grep -c -- --- {calls}) = 2 ]; then\n\
             printf '%s\\n' '{{\"result\":\"{{\\\"decisions\\\":[{{\\\"kind\\\":\\\"skill\\\",\\\"title\\\":\\\"Verify a release build\\\",\\\"body\\\":\\\"1. Build.\\\"}}]}}\"}}'\n\
             else\n\
             printf '%s\\n' '{{\"result\":\"{{\\\"decisions\\\":[{{\\\"kind\\\":\\\"skill\\\",\\\"title\\\":\\\"Ship the web bundle\\\",\\\"body\\\":\\\"1. npm run build.\\\"}}]}}\"}}'\n\
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
            "#!/bin/sh\n{}sleep 0.3\n{}sleep 0.3\n{}sleep 0.3\n{}sleep 30\n",
            hook("SessionStart"),
            hook("PostToolUseFailure"),
            hook("Stop"),
            // Each hook is its own process, so on a slow machine the Stop may land
            // before the failure: the next turn's end reads it then, as in use.
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
        "{listed}\n--- model calls:\n{}\n--- daemon log:\n{}",
        std::fs::read_to_string(out.join("calls")).unwrap_or_default(),
        std::fs::read_to_string(dir.join("state/valkyrie/daemon.log")).unwrap_or_default()
    );
    let shown = stored(&dir, &repo, 2);
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
    assert!(stored(&dir, &repo, 3).contains("Learned from the session"));

    // The agent read skill #1: that's recorded, so it isn't retired as unused.
    let used = std::fs::read_dir(dir.join("state/valkyrie/context"))
        .unwrap()
        .flatten()
        .map(|e| e.path().join("skill-use.json"))
        .find(|p| p.exists())
        .expect("skill use recorded");
    let used: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(used).unwrap()).unwrap();
    assert!(
        used["reads"]["1"].is_u64() && used["sessions"]["c1"].is_u64(),
        "{used}"
    );

    // A prompt about a skill points the agent at it.
    let mut hook = Command::new(BIN);
    hook.arg("context-hook").arg("claude");
    env(&mut hook);
    hook.env("VALK_SESSION", id)
        .env("VALK_CONTEXT", dir.join("state/valkyrie/context"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    let mut child = hook.spawn().unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            serde_json::json!({"hook_event_name":"UserPromptSubmit","prompt":"ok cut the release now","cwd":repo})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
    let said = String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap();
    assert!(
        said.contains("Skill #1 “Cut a release” looks like what this asks for")
            && said.contains("decisions show 1"),
        "{said}"
    );

    // A long job you approved: the daemon asks for the skill in it, unasked.
    let long = out.join("t2.jsonl");
    let mut lines = vec![
        serde_json::json!({"type":"user","message":{"content":"build the web bundle"}}).to_string(),
    ];
    for i in 0..16 {
        let n = format!("w{i}");
        lines.push(call(&n, "npm run build"));
        lines.push(result(&n, None));
    }
    lines.push(
        serde_json::json!({"type":"user","message":{"content":"Perfect, commit and push"}})
            .to_string(),
    );
    std::fs::write(&long, lines.join("\n") + "\n").unwrap();
    let agent2 = dir.join("agent2/claude");
    std::fs::create_dir_all(agent2.parent().unwrap()).unwrap();
    let hook2 = |event: &str| {
        format!(
            "echo '{{\"hook_event_name\":\"{event}\",\"source\":\"startup\",\"session_id\":\"c2\",\"transcript_path\":\"{}\"}}' | valk hook claude\n",
            long.display()
        )
    };
    std::fs::write(
        &agent2,
        format!(
            "#!/bin/sh\n{}sleep 0.3\n{}sleep 0.3\n{}sleep 30\n",
            hook2("SessionStart"),
            hook2("Stop"),
            // The same approval ends another turn: asked once.
            hook2("Stop"),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&agent2, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    valk(&[
        "new",
        "--cwd",
        repo.to_str().unwrap(),
        "--",
        agent2.to_str().unwrap(),
    ]);
    let start = Instant::now();
    let mut listed = String::new();
    while start.elapsed() < Duration::from_secs(10) && !listed.contains("#4") {
        std::thread::sleep(Duration::from_millis(100));
        listed = valk(&["decisions"]);
    }
    assert!(
        listed.contains("#4  skill      Ship the web bundle (proposed)"),
        "{listed}"
    );
    assert!(stored(&dir, &repo, 4).contains("after you said “Perfect, commit and push”"));
    std::thread::sleep(Duration::from_millis(1000));
    let asked = std::fs::read_to_string(out.join("calls")).unwrap();
    assert_eq!(asked.matches("---").count(), 3, "{asked}");

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
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
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
    // Either order: each `valk hook` is its own connection, and the daemon can read
    // the second edit first. Newest-first is activity.rs's unit test to pin.
    assert!(
        text.contains("asked “refactor the auth module”; editing src/db.rs, src/auth.rs")
            || text.contains("asked “refactor the auth module”; editing src/auth.rs, src/db.rs"),
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
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
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
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
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
    assert_eq!(replies[1]["result"]["tools"].as_array().unwrap().len(), 9);
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

/// ADR-0013 §2/§4 and ADR-0016 §2: a stop that leaves work uncommitted, answered
/// by a routine reply that the next stop shows was carried out, counts toward the
/// user's habit. Two such replies in two sessions, in two repositories, propose
/// it in the user scope, for the user to accept.
#[test]
fn routine_replies_in_two_sessions_propose_a_ship_habit() {
    let dir = std::env::temp_dir().join(format!("valkyrie-habit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let daemon = Daemon { dir: dir.clone() };
    let (bin, out) = (dir.join("bin"), dir.join("out"));
    for d in [&bin, &out, &dir.join("run")] {
        std::fs::create_dir_all(d).unwrap();
    }
    let exec: std::fs::Permissions = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(
        dir.join("run"),
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    std::os::unix::fs::symlink(BIN, bin.join("valk")).unwrap();
    let git = |repo: &Path, args: &[&str]| {
        let ok = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    };

    // The model: records each prompt, says the reply asked for a commit.
    let model = bin.join("model");
    std::fs::write(
        &model,
        format!(
            "#!/bin/sh\ncat >> {calls}\necho '---' >> {calls}\necho commit\n",
            calls = out.join("calls").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&model, exec.clone()).unwrap();

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let valk = |cwd: &Path, args: &[&str]| {
        let out = Command::new(BIN)
            .args(args)
            .current_dir(cwd)
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
    let store = valkyrie_context::Store::new(dir.join("state/valkyrie/context"));
    let user = valkyrie_context::project::user();
    let now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    };

    for (n, name) in ["api", "web"].iter().enumerate() {
        let repo = dir.join(name);
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);
        git(&repo, &["checkout", "--quiet", "-b", "fix"]);
        std::fs::write(repo.join("a.txt"), "fixed").unwrap();
        // The agent stops with the fix uncommitted; you say "commit it"; it
        // commits and stops again.
        let hook = |event: &str, extra: &str| {
            format!(
                "echo '{{\"hook_event_name\":\"{event}\",\"session_id\":\"c{n}\",\"cwd\":\"{}\"{extra}}}' | valk hook claude\n",
                repo.display()
            )
        };
        let agent = bin.join(format!("claude-{name}"));
        std::fs::write(
            &agent,
            format!(
                "#!/bin/sh\n{}{}sleep 1.5\n{}sleep 0.3\ngit -c user.name=t -c user.email=t@t commit --quiet -am fix\n{}sleep 30\n",
                hook("SessionStart", ",\"source\":\"startup\""),
                hook("Stop", ""),
                hook("UserPromptSubmit", ",\"prompt\":\"looks good, commit it\""),
                hook("Stop", ""),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&agent, exec.clone()).unwrap();
        // Named `claude` so the session is taken for Claude's.
        let claude = bin.join("claude");
        let _ = std::fs::remove_file(&claude);
        std::os::unix::fs::symlink(&agent, &claude).unwrap();
        // One name for both: sessions are told apart by more than their names.
        valk(
            &repo,
            &[
                "new",
                "--name",
                "fix",
                "--cwd",
                repo.to_str().unwrap(),
                "--",
                claude.to_str().unwrap(),
            ],
        );
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(15) && store.replies(now()).len() < n + 1 {
            std::thread::sleep(Duration::from_millis(100));
        }
        let replies = store.replies(now());
        assert_eq!(replies.len(), n + 1, "{replies:?}");
        let r = &replies[n];
        assert_eq!(
            (r.state.as_str(), r.step, r.quote.as_deref()),
            (
                "uncommitted",
                Some(valkyrie_context::policy::Until::Commit),
                Some("looks good, commit it")
            ),
        );
        // One session's word isn't a habit yet.
        if n == 0 {
            std::thread::sleep(Duration::from_millis(300));
            assert!(store.load(&user).is_empty());
        }
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) && store.load(&user).is_empty() {
        std::thread::sleep(Duration::from_millis(100));
    }
    let habits = store.load(&user);
    assert_eq!(habits.len(), 1, "{habits:?}");
    let h = &habits[0];
    assert_eq!(h.status, valkyrie_proto::DecisionStatus::Proposed);
    assert_eq!(h.title, "Commit finished work");
    assert!(
        h.body.starts_with("template: ship\nuntil: commit\n"),
        "{}",
        h.body
    );
    assert!(
        h.body
            .contains("> “looks good, commit it” (web, uncommitted → commit)"),
        "{}",
        h.body
    );
    let calls = std::fs::read_to_string(out.join("calls")).unwrap();
    assert_eq!(calls.matches("---").count(), 2, "{calls}");
    assert!(
        calls.contains("<reply>\nlooks good, commit it\n</reply>"),
        "{calls}"
    );
    drop(daemon);
}

/// ADR-0016 §3: what agents run before a push that went through, and CI checked
/// with their own tools after it, in two sessions, are proposed as the
/// repository's `checks` and `ci: agent`.
#[test]
fn what_runs_before_pushes_in_two_sessions_proposes_the_repositorys_facts() {
    let dir = std::env::temp_dir().join(format!("valkyrie-facts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let daemon = Daemon { dir: dir.clone() };
    let (bin, repo) = (dir.join("bin"), dir.join("repo"));
    for d in [&bin, &repo.join(".git"), &dir.join("run")] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
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
    let valk = |args: &[&str]| {
        let out = Command::new(BIN)
            .args(args)
            .current_dir(&repo)
            .env("PATH", &path)
            .env("XDG_STATE_HOME", dir.join("state"))
            .env("VALK_SOCKET", dir.join("run/v.sock"))
            .env_remove("VALK_SESSION")
            .env_remove("VALK_CONTEXT")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    let store = valkyrie_context::Store::new(dir.join("state/valkyrie/context"));
    let root = valkyrie_context::project::root(&repo);
    let tool = |name: &str, input: &str| {
        format!(
            "echo '{{\"hook_event_name\":\"PostToolUse\",\"session_id\":\"c\",\"cwd\":\"{}\",\"tool_name\":\"{name}\",\"tool_input\":{input}}}' | valk hook claude\nsleep 0.2\n",
            repo.display()
        )
    };
    for n in 0..2 {
        let agent = bin.join(format!("agent-{n}"));
        std::fs::write(
            &agent,
            format!(
                "#!/bin/sh\n{}{}{}{}{}sleep 30\n",
                tool("Bash", r#"{"command":"cargo test -q 2>&1 | tail -3"}"#),
                tool("Bash", r#"{"command":"ls"}"#),
                tool("Bash", r#"{"command":"git push -u origin fix"}"#),
                tool("mcp__bamboo__get_build", r#"{"plan":"APP"}"#),
                tool("Bash", r#"{"command":"cargo build"}"#),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&agent, exec.clone()).unwrap();
        let claude = bin.join("claude");
        let _ = std::fs::remove_file(&claude);
        std::os::unix::fs::symlink(&agent, &claude).unwrap();
        valk(&[
            "new",
            "--cwd",
            repo.to_str().unwrap(),
            "--",
            claude.to_str().unwrap(),
        ]);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10)
            && store
                .facts(
                    &root,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                )
                .len()
                < 2 * (n + 1)
        {
            std::thread::sleep(Duration::from_millis(100));
        }
        if n == 0 {
            std::thread::sleep(Duration::from_millis(300));
            assert!(store.load(&root).is_empty(), "one session isn't enough");
        }
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) && store.load(&root).len() < 2 {
        std::thread::sleep(Duration::from_millis(100));
    }
    let mut found = store.load(&root);
    found.sort_by_key(|d| d.id);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(
        found
            .iter()
            .all(|d| d.status == valkyrie_proto::DecisionStatus::Proposed)
    );
    let bodies: Vec<&str> = found.iter().map(|d| d.body.as_str()).collect();
    assert!(
        bodies
            .iter()
            .any(|b| b.starts_with("template: ship\nchecks: cargo test -q\n")),
        "{bodies:?}"
    );
    assert!(
        bodies
            .iter()
            .any(|b| b.starts_with("template: ship\nci: agent\n")
                && b.contains("`mcp__bamboo__get_build`")),
        "{bodies:?}"
    );
    drop(daemon);
}

/// A prompt gets the decisions it bears on, once a session (ADR-0014 §3): the
/// hook reads the store and the serving log itself, no daemon.
#[test]
fn a_prompt_gets_the_decisions_it_bears_on_once_a_session() {
    let dir = std::env::temp_dir().join(format!("valkyrie-prompt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let base = dir.join("context");
    for d in [&dir, &base] {
        std::fs::create_dir_all(d).unwrap();
        std::fs::set_permissions(d, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    }
    let store = valkyrie_context::Store::new(&base);
    let human = valkyrie_proto::Provenance {
        by: "human".into(),
        ..Default::default()
    };
    for (title, body) in [
        (
            "Run shellcheck on shell scripts before pushing",
            "CI fails on SC2015.",
        ),
        (
            "Pair devices with a one-time code",
            "Never a request endpoint.",
        ),
        (
            "Rain animates under reduced motion",
            "It's ambient, not motion.",
        ),
        ("Tags never move once published", "Edge is the rolling one."),
    ] {
        let new = valkyrie_proto::NewDecision {
            cwd: repo.clone(),
            title: title.into(),
            body: body.into(),
            kind: valkyrie_proto::DecisionKind::Gotcha,
            propose: false,
            supersedes: None,
            review_every: None,
            session: None,
            scope: None,
        };
        store.decide(&new, human.clone(), 1).unwrap();
    }
    let hook = |session: &str, prompt: &str| -> String {
        let mut c = Command::new(BIN);
        c.args(["context-hook", "claude"])
            .current_dir(&repo)
            .env("VALK_SESSION", "1")
            .env("VALK_CONTEXT", &base)
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env_remove("VALK_SOCKET")
            .env_remove("VALK_SESSION_HOOKS")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let mut child = c.spawn().unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                serde_json::json!({"hook_event_name":"UserPromptSubmit","session_id":session,
                                   "prompt":prompt,"cwd":repo})
                .to_string()
                .as_bytes(),
            )
            .unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    };
    let ask = "shellcheck fails on the install scripts";
    let said = hook("a", ask);
    let out: serde_json::Value = serde_json::from_str(said.trim()).expect("one JSON object");
    let added = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        added.contains("Run shellcheck on shell scripts") && added.contains("SC2015"),
        "{added}"
    );
    assert!(!added.contains("Pair devices"), "{added}");
    // Once a session: asked again, nothing; another session gets it.
    assert_eq!(hook("a", ask).trim(), "");
    assert!(hook("b", ask).contains("Run shellcheck"));
    // Without the agent's session id there's no telling what it had: nothing.
    assert_eq!(hook("", ask).trim(), "");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ADR-0014 §2: the first `Read`, `Edit` or `Write` of a file in a decision's
/// scope adds its body, once a session; the start block's bodies count as had.
#[test]
fn a_file_gets_the_decisions_scoped_to_it_once_a_session() {
    let dir = std::env::temp_dir().join(format!("valkyrie-scope-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::create_dir_all(repo.join("web/src")).unwrap();
    std::fs::write(repo.join("web/src/proto.ts"), "").unwrap();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let base = dir.join("context");
    for d in [&dir, &base] {
        std::fs::create_dir_all(d).unwrap();
        std::fs::set_permissions(d, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    }
    let store = valkyrie_context::Store::new(&base);
    let human = valkyrie_proto::Provenance {
        by: "human".into(),
        ..Default::default()
    };
    for (title, body, kind, scope) in [
        (
            "Update the TS types with every wire change",
            "Hand-copied, nothing checks them.",
            valkyrie_proto::DecisionKind::Gotcha,
            vec!["web/src/*.ts"],
        ),
        (
            "Alt chords for one-step keys",
            "Not a prefix key.",
            valkyrie_proto::DecisionKind::Decision,
            vec!["crates/valkyrie-tui/**"],
        ),
        (
            "Never hand-edit web/dist",
            "It's built.",
            valkyrie_proto::DecisionKind::Constraint,
            vec!["web/**"],
        ),
    ] {
        let new = valkyrie_proto::NewDecision {
            cwd: repo.clone(),
            title: title.into(),
            body: body.into(),
            kind,
            propose: false,
            supersedes: None,
            review_every: None,
            session: None,
            scope: Some(scope.into_iter().map(String::from).collect()),
        };
        store.decide(&new, human.clone(), 1).unwrap();
    }
    let hook = |payload: serde_json::Value| -> String {
        let mut c = Command::new(BIN);
        c.args(["context-hook", "claude"])
            .current_dir(&repo)
            .env("VALK_SESSION", "1")
            .env("VALK_CONTEXT", &base)
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_STATE_HOME", dir.join("state"))
            .env_remove("VALK_SOCKET")
            .env_remove("VALK_SESSION_HOOKS")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        let mut child = c.spawn().unwrap();
        use std::io::Write;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    };
    let start = |session: &str| {
        hook(
            serde_json::json!({"hook_event_name":"SessionStart","session_id":session,
                                "cwd":repo}),
        )
    };
    let tool = |session: &str, tool: &str, file: &str| {
        hook(
            serde_json::json!({"hook_event_name":"PreToolUse","session_id":session,
                                "tool_name":tool,"tool_input":{"file_path":file},"cwd":repo}),
        )
    };
    let proto = repo.join("web/src/proto.ts");
    let proto = proto.to_str().unwrap();
    start("a");
    let said = tool("a", "Read", proto);
    let out: serde_json::Value = serde_json::from_str(said.trim()).expect("one JSON object");
    assert_eq!(out["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    let added = out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        added.starts_with("Decisions about `web/src/proto.ts`")
            && added.contains("Update the TS types")
            && added.contains("Hand-copied"),
        "{added}"
    );
    // The constraint's body was in the start block already, and the TUI's
    // decision is about other files.
    assert!(
        !added.contains("hand-edit") && !added.contains("Alt chords"),
        "{added}"
    );
    // Once a session, whichever tool touches it next.
    assert_eq!(tool("a", "Edit", proto).trim(), "");
    // Relative, and not written yet: placed by where it would go.
    assert!(tool("a", "Write", "crates/valkyrie-tui/src/new.rs").contains("Alt chords"));
    // A shell command never gets a scope's decisions, and a session without
    // its start block (an agent that missed it) gets the constraint too.
    assert_eq!(tool("b", "Bash", proto).trim(), "");
    let b = tool("b", "Read", proto);
    assert!(
        b.contains("Update the TS types") && b.contains("hand-edit"),
        "{b}"
    );
    // Logged as served per file.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let served = store.served(&repo, now);
    assert!(
        served
            .iter()
            .any(|s| s.session == "a" && s.layer == valkyrie_context::served::Layer::File),
        "{served:?}"
    );
    // Outside the project, nothing.
    assert_eq!(tool("a", "Read", "/etc/hostname").trim(), "");
    // A subagent's read, under its parent's session id, gets nothing and
    // doesn't use up the parent's serve.
    let sub = hook(
        serde_json::json!({"hook_event_name":"PreToolUse","session_id":"c",
        "agent_id":"sub-1","tool_name":"Read","tool_input":{"file_path":proto},"cwd":repo}),
    );
    assert_eq!(sub.trim(), "");
    assert!(tool("c", "Read", proto).contains("Update the TS types"));
    let _ = std::fs::remove_dir_all(&dir);
}

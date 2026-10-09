//! The context layer with the real binary (DESIGN §6, ADR-0007): an agent can only
//! propose decisions, corrections become proposals, agents hear about each other,
//! and handoffs.
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

    assert!(valk(&["decisions", "auto", "off"]).starts_with("off"));
    assert!(valk(&["decisions", "auto"]).starts_with("off"));
    assert!(valk(&["decisions", "auto", "on"]).starts_with("on"));
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
    let a = script(
        "a",
        format!(
            "{start}{}{}",
            hook(
                serde_json::json!({"hook_event_name":"UserPromptSubmit","prompt":"refactor the auth module","cwd":repo_s})
            ),
            edit("src/auth.rs") + &edit("src/db.rs"),
        ),
    );
    let b = script(
        "b",
        format!(
            "{start}{}sleep 1\n{}{}echo done > {}\n",
            edit("src/auth.rs"),
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
    valk(&[
        "new",
        "--name",
        "auth-work",
        "--cwd",
        &repo_s,
        "--",
        a.to_str().unwrap(),
    ]);
    valk(&[
        "new",
        "--name",
        "other",
        "--cwd",
        &repo_s,
        "--",
        b.to_str().unwrap(),
    ]);
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
             case \"$last\" in \"#\"*) printf '%s' \"$last\" > {got};; esac\n\
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
    assert!(got.starts_with("# Handoff from web"), "{got}");
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

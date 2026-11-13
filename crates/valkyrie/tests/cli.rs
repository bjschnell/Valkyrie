//! The real binary: `valk hook` contract (ADR-0005) and `valk setup codex`.
#![cfg(unix)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_valk");

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("valkyrie-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Removes a test's directory. A session's process can outlive the daemon by a
/// moment and write into it while it's being removed (`DirectoryNotEmpty`, #131),
/// so this tries again for a while. If it still fails, it names what was left.
fn remove_test_dir(dir: &Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) if Instant::now() >= deadline => {
                let left: Vec<_> = walk(dir)
                    .into_iter()
                    .map(|p| p.display().to_string())
                    .collect();
                panic!("can't remove {}: {e}; left: {left:?}", dir.display());
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        }
        out.push(path);
    }
    out
}

/// A daemon a test started itself, killed when the test ends however it ends: an
/// assert failing before the test's own `kill` used to leave it running for good.
struct KillOnDrop(std::process::Child);

impl std::ops::Deref for KillOnDrop {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for KillOnDrop {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        // Already gone when the test killed it itself.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn valk(socket: &Path, state: &Path) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.env("VALK_SOCKET", socket)
        .env("XDG_STATE_HOME", state)
        .env("XDG_CONFIG_HOME", state.join("config"))
        .env_remove("VALK_SESSION")
        .env_remove("VALK_SESSION_HOOKS")
        .env_remove("VALK_PROJECT_CONTEXT");
    cmd
}

fn hook(mut cmd: Command, stdin: &[u8]) -> Output {
    let mut child = cmd
        .args(["hook", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A hook may exit without reading its input; the exit status and output are
    // what's tested, so a write it outlived (EPIPE) is no failure.
    match child.stdin.take().unwrap().write_all(stdin) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        written => written.unwrap(),
    }
    child.wait_with_output().unwrap()
}

fn assert_silent_success(out: &Output) {
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "hook printed to stdout: {out:?}");
    assert!(out.stderr.is_empty(), "hook printed to stderr: {out:?}");
}

/// The agent writes the whole payload before it waits, so the hook reads all of
/// its stdin on every path, even ones that do nothing: a payload larger than the
/// pipe buffer would otherwise hit EPIPE in the agent.
#[test]
fn hook_reads_its_whole_input_even_when_it_does_nothing_with_it() {
    let dir = temp("drain");
    let socket = dir.join("run/none.sock");
    let big = vec![b' '; 1 << 20];
    let outside = valk(&socket, &dir);
    // Arguments that don't parse (empty --socket from the environment).
    let mut bad_args = valk(&socket, &dir);
    bad_args.env("VALK_SOCKET", "").env("VALK_SESSION", "1");
    let mut bad_session = valk(&socket, &dir);
    bad_session.env("VALK_SESSION", "not a number");
    for (case, mut cmd) in [
        ("outside a session", outside),
        ("bad arguments", bad_args),
        ("bad VALK_SESSION", bad_session),
    ] {
        let mut child = cmd
            .args(["hook", "claude"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let written = child.stdin.take().unwrap().write_all(&big);
        let out = child.wait_with_output().unwrap();
        assert!(
            written.is_ok(),
            "{case}: hook left its input unread: {written:?}"
        );
        assert_silent_success(&out);
    }
    remove_test_dir(&dir);
}

#[test]
fn hook_is_silent_and_succeeds_outside_valkyrie_and_without_a_daemon() {
    let dir = temp("silent");
    let socket = dir.join("run/none.sock");
    // Not in a session: a no-op whatever the input.
    assert_silent_success(&hook(valk(&socket, &dir), b"not json"));
    // In a session but the daemon is gone, or the input is garbage: still silent, 0.
    let mut cmd = valk(&socket, &dir);
    cmd.env("VALK_SESSION", "1");
    assert_silent_success(&hook(cmd, br#"{"hook_event_name":"Stop"}"#));
    let mut cmd = valk(&socket, &dir);
    cmd.env("VALK_SESSION", "1");
    assert_silent_success(&hook(cmd, b"\xff garbage"));
    // Arguments that don't parse (empty --socket from the environment): still silent.
    let mut cmd = valk(&socket, &dir);
    cmd.env("VALK_SOCKET", "").env("VALK_SESSION", "1");
    assert_silent_success(&hook(cmd, b"{}"));
    // That path exits without reading stdin, so a payload larger than the pipe
    // buffer always outlives it: the write then fails with EPIPE every time.
    let mut cmd = valk(&socket, &dir);
    cmd.env("VALK_SOCKET", "").env("VALK_SESSION", "1");
    assert_silent_success(&hook(cmd, &vec![b' '; 1 << 20]));
    remove_test_dir(&dir);
}

fn statusline(mut cmd: Command, then: &str, stdin: &[u8]) -> Output {
    let mut child = cmd
        .args(["statusline", "--then", then])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

/// The user's own status line draws as it would without Valkyrie: the same input,
/// its output and its exit, daemon or none, arguments that parse or not.
#[test]
fn statusline_runs_the_users_own_and_tells_the_daemon_the_quota() {
    let dir = temp("statusline");
    let none = dir.join("run/none.sock");
    let payload = br#"{"model":{"id":"x"},"rate_limits":{"five_hour":{"used_percentage":81,"resets_at":4102444800},"seven_day":{"used_percentage":12,"resets_at":4102444800}}}"#;
    let theirs = "printf 'hud:'; cat; exit 3";
    let out = statusline(valk(&none, &dir), theirs, payload);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert_eq!(out.stdout, [&b"hud:"[..], payload].concat());
    let mut cmd = valk(&none, &dir);
    cmd.env("VALK_SOCKET", "");
    let out = statusline(cmd, theirs, b"{}");
    assert_eq!(out.stdout, b"hud:{}", "{out:?}");

    let socket = dir.join("run/s.sock");
    let _daemon = KillOnDrop(
        valk(&socket, &dir)
            .arg("daemon")
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "daemon never bound");
        std::thread::sleep(Duration::from_millis(20));
    }
    let out = statusline(valk(&socket, &dir), "cat", payload);
    assert_eq!(out.stdout, payload, "{out:?}");
    // The daemon keeps what it heard, for after a restart.
    let kept = dir.join("valkyrie/quota.json");
    while !kept.exists() {
        assert!(Instant::now() < deadline, "quota never kept");
        std::thread::sleep(Duration::from_millis(20));
    }
    let quota: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&kept).unwrap()).unwrap();
    assert_eq!(
        quota["limits"],
        serde_json::json!([
            {"name": "five_hour", "used": 81, "resets_at": 4102444800u64},
            {"name": "seven_day", "used": 12, "resets_at": 4102444800u64},
        ])
    );
    drop(_daemon);
    remove_test_dir(&dir);
}

#[test]
fn hook_reaches_the_daemon_quickly() {
    let dir = temp("e2e");
    let socket = dir.join("run/o.sock");
    let mut daemon = KillOnDrop(
        valk(&socket, &dir)
            .arg("daemon")
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let new = (0..100)
        .find_map(|_| {
            let out = valk(&socket, &dir)
                .args(["new", "--", "sleep", "30"])
                .output()
                .unwrap();
            if out.status.success() {
                return Some(out);
            }
            std::thread::sleep(Duration::from_millis(20));
            None
        })
        .expect("daemon did not start");
    let id = String::from_utf8(new.stdout).unwrap().trim().to_string();

    let payload = br#"{"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"cargo test"}}"#;
    let mut times = Vec::new();
    for _ in 0..30 {
        let mut cmd = valk(&socket, &dir);
        cmd.env("VALK_SESSION", &id);
        let start = Instant::now();
        let out = hook(cmd, payload);
        times.push(start.elapsed());
        assert_silent_success(&out);
    }
    times.sort();
    let median = times[times.len() / 2];
    eprintln!(
        "hook round trip: median {median:?}, max {:?}",
        times[times.len() - 1]
    );
    assert!(median < Duration::from_millis(50), "median {median:?}");

    let ls = valk(&socket, &dir).arg("ls").output().unwrap();
    let ls = String::from_utf8(ls.stdout).unwrap();
    assert!(ls.contains("needs input"), "{ls}");
    assert!(ls.contains("Permission: Bash cargo test"), "{ls}");

    let _ = valk(&socket, &dir).args(["kill", &id]).output();
    daemon.kill().unwrap();
    daemon.wait().unwrap();

    // The recording replays to the same states it recorded.
    let log = std::fs::read_dir(dir.join("valkyrie/sessions"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().ends_with(".events.jsonl"))
        .unwrap();
    let out = Command::new(BIN).arg("replay").arg(&log).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let report = String::from_utf8(out.stdout).unwrap();
    assert!(report.contains("needs input"), "{report}");
    assert!(report.contains("0 mismatches"), "{report}");
    remove_test_dir(&dir);
}

/// The desktop app's setup (ADR-0020 §4): no daemon is asked to vouch, since no
/// pairing code is printed, and the service is told the one local address.
#[test]
fn setup_web_local_stays_on_this_machine() {
    let dir = temp("web-local");
    let setup = |extra: &[&str]| {
        valk(&dir.join("none.sock"), &dir)
            .args(["setup", "web", "--local", "--dry-run", "--exe", "/opt/valk"])
            .args(extra)
            .output()
            .unwrap()
    };

    let out = setup(&[]);
    assert!(out.status.success(), "{out:?}");
    let unit = String::from_utf8(out.stdout).unwrap();
    assert!(unit.contains("http://127.0.0.1:8790"), "{unit}");

    let out = setup(&["--listen", "127.0.0.1:9001"]);
    let unit = String::from_utf8(out.stdout).unwrap();
    assert!(unit.contains("http://127.0.0.1:9001"), "{unit}");

    // Never beyond the machine, and never under another name.
    let out = setup(&["--listen", "0.0.0.0:8790"]);
    assert!(!out.status.success());
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("127.0.0.1 only"), "{err}");
    let out = setup(&["--url", "https://box.example.ts.net"]);
    assert!(!out.status.success());
    remove_test_dir(&dir);
}

#[test]
fn setup_codex_installs_backs_up_and_removes() {
    let dir = temp("codex");
    let hooks = dir.join("hooks.json");
    std::fs::write(
        &hooks,
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"notify-send done"}]}]}}"#,
    )
    .unwrap();
    let setup = |extra: &[&str]| {
        let out = Command::new(BIN)
            .env("CODEX_HOME", &dir)
            .args(["setup", "codex", "--exe", "/opt/valk"])
            .args(extra)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };

    let dry = setup(&["--dry-run"]);
    assert!(
        dry.contains("'/opt/valk' hook codex 2>/dev/null || true"),
        "{dry}"
    );
    assert!(!std::fs::read_to_string(&hooks).unwrap().contains("valk"));

    assert!(setup(&[]).contains("/hooks"));
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hooks).unwrap()).unwrap();
    assert_eq!(doc["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(
        doc["hooks"]["Interrupt"][0]["hooks"][0]["command"],
        "'/opt/valk' hook codex 2>/dev/null || true"
    );
    assert!(setup(&[]).contains("already up to date"));
    let backups = std::fs::read_dir(&dir)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".bak-")
        })
        .count();
    assert_eq!(backups, 1);

    setup(&["--remove"]);
    // Removing from a hooks.json that doesn't exist creates nothing.
    let empty = dir.join("empty");
    let out = Command::new(BIN)
        .env("CODEX_HOME", &empty)
        .args(["setup", "codex", "--remove"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(!empty.join("hooks.json").exists());
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hooks).unwrap()).unwrap();
    assert_eq!(
        doc,
        serde_json::json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"notify-send done"}]}]}})
    );
    remove_test_dir(&dir);
}

/// A `claude` typed in a Valkyrie shell gets no `--settings`, so the context hook
/// also lives in Claude Code's own settings, put there and taken out by `valk`.
#[test]
fn setup_claude_installs_once_keeps_the_rest_and_removes() {
    let dir = temp("claude-settings");
    let settings = dir.join("settings.json");
    let theirs = r#"{"model":"opus","hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"theirs"}]}]}}"#;
    std::fs::write(&settings, theirs).unwrap();
    let setup = |exe: &str, extra: &[&str]| {
        let out = Command::new(BIN)
            .env("CLAUDE_CONFIG_DIR", &dir)
            .args(["setup", "claude", "--exe", exe])
            .args(extra)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    let read = || -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap()
    };
    let ours = |exe: &str| format!("'{exe}' context-hook claude --ambient 2>/dev/null || true");
    let observer = |exe: &str| format!("'{exe}' hook claude --ambient 2>/dev/null || true");

    assert!(setup("/opt/valk", &["--dry-run"]).contains(&ours("/opt/valk")));
    assert_eq!(std::fs::read_to_string(&settings).unwrap(), theirs);

    setup("/opt/valk", &[]);
    let doc = read();
    assert_eq!(doc["model"], "opus");
    let starts = doc["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(starts.len(), 2, "{doc}");
    assert_eq!(starts[1]["hooks"][0]["command"], observer("/opt/valk"));
    assert_eq!(starts[1]["hooks"][1]["command"], ours("/opt/valk"));
    assert_eq!(
        doc["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        observer("/opt/valk")
    );
    assert_eq!(
        doc["hooks"]["UserPromptSubmit"][0]["hooks"][1]["command"],
        ours("/opt/valk")
    );
    assert!(setup("/opt/valk", &[]).contains("already up to date"));

    // A binary that moved replaces the hook; it is not added beside it.
    setup("/elsewhere/valk", &[]);
    let doc = read();
    assert_eq!(doc["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
    assert!(!doc.to_string().contains("/opt/valk"));

    // The file as it was before the first change is kept, once.
    let backup = dir.join("settings.json.valkyrie-bak");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), theirs);

    setup("/elsewhere/valk", &["--remove"]);
    assert_eq!(
        read(),
        serde_json::from_str::<serde_json::Value>(theirs).unwrap()
    );

    // Taking it out where there is no settings file creates none.
    let empty = dir.join("empty");
    let out = Command::new(BIN)
        .env("CLAUDE_CONFIG_DIR", &empty)
        .args(["setup", "claude", "--remove"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(!empty.join("settings.json").exists());
    remove_test_dir(&dir);
}

/// The Context setting switches the hook off wherever it runs, and the copy in
/// Claude Code's own settings steps aside where the session carries one already.
#[test]
fn context_hook_follows_the_setting_and_the_sessions_own_hooks() {
    let dir = temp("context-switch");
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::create_dir_all(dir.join("config/valkyrie")).unwrap();
    let run = |args: &[&str], envs: &[(&str, &str)]| {
        let mut cmd = valk(&dir.join("run/none.sock"), &dir);
        cmd.env("VALK_CONTEXT", dir.join("context"))
            .env("VALK_SESSION", "1");
        for (k, v) in envs {
            cmd.env(k, v);
        }
        let mut child = cmd
            .arg("context-hook")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let payload = format!(
            r#"{{"hook_event_name":"SessionStart","cwd":"{}"}}"#,
            repo.display()
        );
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let speaks = |out: &Output| {
        assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
        !out.stdout.is_empty()
    };
    assert!(speaks(&run(&["claude"], &[])), "on by default");
    assert!(speaks(&run(&["claude", "--ambient"], &[])));
    // Where the session was launched with the hook, the ambient copy is quiet.
    let hooked = [("VALK_SESSION_HOOKS", "1")];
    assert!(speaks(&run(&["claude"], &hooked)));
    assert!(!speaks(&run(&["claude", "--ambient"], &hooked)));

    std::fs::write(dir.join("config/valkyrie/settings.toml"), "context = off\n").unwrap();
    assert!(!speaks(&run(&["claude"], &[])));
    assert!(!speaks(&run(&["claude", "--ambient"], &[])));
    // The environment wins over the file, as for every setting.
    assert!(speaks(&run(&["claude"], &[("VALK_PROJECT_CONTEXT", "on")])));
    remove_test_dir(&dir);
}

/// The daemon keeps the hook in Claude Code's settings while Context is on, and
/// marks the sessions it launched an agent in with its hooks itself.
#[test]
fn daemon_installs_the_hook_and_marks_the_agents_it_hooked() {
    let dir = temp("daemon-claude");
    let socket = dir.join("run/o.sock");
    let (claude_dir, bin) = (dir.join("claude"), dir.join("bin"));
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    // Stands in for claude: records what its environment says, then exits.
    let stub = bin.join("claude");
    std::fs::write(
        &stub,
        format!(
            "#!/bin/sh\nprintf '%s' \"${{VALK_SESSION_HOOKS:-unset}}\" > {}/env-claude\n",
            dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut daemon = KillOnDrop(
        valk(&socket, &dir)
            .env("CLAUDE_CONFIG_DIR", &claude_dir)
            .arg("daemon")
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let spawn = |command: &[&str]| {
        (0..100)
            .find_map(|_| {
                let out = valk(&socket, &dir)
                    .args(["new", "--"])
                    .args(command)
                    .output()
                    .unwrap();
                if out.status.success() {
                    return Some(out);
                }
                std::thread::sleep(Duration::from_millis(20));
                None
            })
            .expect("daemon did not start")
    };
    spawn(&[stub.to_str().unwrap()]);
    let shell = format!(
        "printf '%s' \"${{VALK_SESSION_HOOKS:-unset}}\" > {}/env-sh",
        dir.display()
    );
    spawn(&["sh", "-c", &shell]);
    let read = |name: &str| {
        let file = dir.join(name);
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if let Ok(text) = std::fs::read_to_string(&file)
                && !text.is_empty()
            {
                return text;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("{name} never written");
    };
    assert_eq!(read("env-claude"), "1", "an agent launched with its hooks");
    assert_eq!(
        read("env-sh"),
        "unset",
        "a shell has none to be quiet about"
    );
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(claude_dir.join("settings.json")).unwrap())
            .unwrap();
    assert!(
        doc["hooks"]["SessionStart"][0]["hooks"][1]["command"]
            .as_str()
            .unwrap()
            .ends_with("context-hook claude --ambient 2>/dev/null || true"),
        "{doc}"
    );
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    remove_test_dir(&dir);
}

/// The observer hook in Claude Code's own settings reports a typed `claude` to the
/// daemon, and stays quiet where the session carries the hook itself.
#[test]
fn ambient_observer_reports_a_typed_claude_once() {
    let dir = temp("ambient-observer");
    let socket = dir.join("run/o.sock");
    let mut daemon = KillOnDrop(
        valk(&socket, &dir)
            .arg("daemon")
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let new = (0..100)
        .find_map(|_| {
            let out = valk(&socket, &dir)
                .args(["new", "--", "sleep", "30"])
                .output()
                .unwrap();
            if out.status.success() {
                return Some(out);
            }
            std::thread::sleep(Duration::from_millis(20));
            None
        })
        .expect("daemon did not start");
    let id = String::from_utf8(new.stdout).unwrap().trim().to_string();
    let send = |hooked: bool| {
        let mut cmd = valk(&socket, &dir);
        cmd.env("VALK_SESSION", &id);
        if hooked {
            cmd.env("VALK_SESSION_HOOKS", "1");
        }
        let payload = br#"{"hook_event_name":"PreToolUse","tool_name":"Bash"}"#;
        let mut child = cmd
            .args(["hook", "claude", "--ambient"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(payload).unwrap();
        assert_silent_success(&child.wait_with_output().unwrap());
    };
    let reported = || {
        let sessions = dir.join("valkyrie/sessions");
        std::fs::read_dir(sessions)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .ends_with(&format!("-{id}.events.jsonl"))
            })
            .map(|e| std::fs::read_to_string(e.path()).unwrap())
            .map(|log| log.matches(r#""k":"hook""#).count())
            .sum::<usize>()
    };
    send(true);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        reported(),
        0,
        "a session with its own hooks hears none twice"
    );
    send(false);
    let start = Instant::now();
    while reported() == 0 && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(reported(), 1);
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    remove_test_dir(&dir);
}

/// The M1 accuracy gate (DESIGN §14.6) on recorded real sessions with hand labels.
/// Recorded by an earlier build, so drift from the recording is expected; the labels
/// are the ground truth.
#[test]
fn recorded_sessions_meet_the_accuracy_gate() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut checked = 0;
    for entry in std::fs::read_dir(&fixtures).unwrap() {
        let path = entry.unwrap().path();
        let name = path.to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".events.jsonl") else {
            continue;
        };
        let out = Command::new(BIN)
            .arg("replay")
            .arg(&path)
            .arg("--labels")
            .arg(format!("{stem}.labels.jsonl"))
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        let report = String::from_utf8(out.stdout).unwrap();
        let accuracy: f64 = report
            .lines()
            .find_map(|l| l.strip_prefix("accuracy vs labels: "))
            .and_then(|l| l.strip_suffix('%'))
            .unwrap()
            .parse()
            .unwrap();
        assert!(accuracy >= 95.0, "{stem}: {accuracy}%\n{report}");
        checked += 1;
    }
    assert!(checked > 0);
}

/// SSH-resume contract: an auto-started daemon leads its own session (a dropped
/// connection can't hang it up) and its default socket lives under the state dir,
/// named for this host, not under `$XDG_RUNTIME_DIR` (deleted at logout, often unset
/// over SSH).
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "reads /proc")]
fn auto_started_daemon_is_detached_and_found_without_a_runtime_dir() {
    /// Kills the daemon's whole session even when an assertion fails first.
    struct Reap(Option<u32>, PathBuf);
    impl Drop for Reap {
        fn drop(&mut self) {
            if let Some(pid) = self.0 {
                let _ = Command::new("kill")
                    .args(["-KILL", "--", &format!("-{pid}")])
                    .status();
                for _ in 0..100 {
                    if !Path::new(&format!("/proc/{pid}")).exists() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            let _ = std::fs::remove_dir_all(&self.1);
        }
    }
    let dir = temp("detach");
    let mut reap = Reap(None, dir.clone());
    let run = |args: &[&str]| {
        let out = Command::new(BIN)
            .args(args)
            .env_remove("VALK_SOCKET")
            .env_remove("XDG_RUNTIME_DIR")
            .env_remove("VALK_SESSION")
            .env("XDG_STATE_HOME", &dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    run(&["new", "--name", "keep", "--", "sleep", "30"]);
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap();
    let socket = dir.join(format!("valkyrie/run/{}.sock", host.trim()));
    let daemon = std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
        .find(|pid| {
            std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|c| {
                let c = String::from_utf8_lossy(&c);
                c.contains(dir.to_str().unwrap()) && c.contains("daemon")
            })
        })
        .expect("daemon process");
    reap.0 = Some(daemon);
    assert!(socket.exists(), "no socket at {}", socket.display());
    // /proc/<pid>/stat: "pid (comm) state ppid pgrp session ..."
    let stat = std::fs::read_to_string(format!("/proc/{daemon}/stat")).unwrap();
    let fields: Vec<&str> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    assert_eq!(
        fields[3],
        daemon.to_string(),
        "not a session leader: {stat}"
    );
    assert!(run(&["ls"]).contains("keep"));
}

/// ADR-0006: `valk upgrade` re-execs the daemon and every session carries on:
/// same process, same screen, input and exit still work. A binary that doesn't speak
/// the handoff format is refused before anything is frozen.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "reads /proc")]
fn upgrade_keeps_sessions_and_refuses_a_foreign_binary() {
    let dir = temp("upgrade");
    let socket = dir.join("run/o.sock");
    let run = |args: &[&str]| {
        let out = valk(&socket, &dir).args(args).output().unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).into_owned()
                + &String::from_utf8_lossy(&out.stderr),
        )
    };
    let ok = |args: &[&str]| {
        let (success, text) = run(args);
        assert!(success, "{args:?}: {text}");
        text
    };
    // The daemon in the foreground, so the test owns and reaps it.
    let mut daemon = KillOnDrop(
        valk(&socket, &dir)
            .arg("daemon")
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let wait_dump = |want: &str| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let text = ok(&["dump", "1"]);
            if text.contains(want) {
                return text;
            }
            assert!(Instant::now() < deadline, "no {want:?} in:\n{text}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let script = r#"n=0; while read -r line; do n=$((n+1)); echo "got#$n: $line"; done; exit 7"#;
    ok(&["new", "--", "bash", "--norc", "-c", script]);
    ok(&["send", "1", r"one\r"]);
    wait_dump("got#1: one");

    let (refused, text) = run(&["upgrade", "--exe", "/bin/true"]);
    assert!(!refused && text.contains("handoff format"), "{text}");
    ok(&["send", "1", r"two\r"]);
    wait_dump("got#2: two");

    let text = ok(&["upgrade", "--exe", BIN]);
    assert!(
        text.contains("generation 1") && text.contains("1/1 sessions kept"),
        "{text}"
    );
    assert!(
        daemon.try_wait().unwrap().is_none(),
        "the daemon process itself must live on"
    );
    let screen = wait_dump("got#2: two");
    assert!(
        screen.contains("got#1: one"),
        "screen not rebuilt:\n{screen}"
    );
    // The counter continuing proves it is the same bash process, not a restart.
    ok(&["send", "1", r"three\r"]);
    wait_dump("got#3: three");
    ok(&["send", "1", r"\x04"]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ok(&["ls"]).contains("exited 7") {
        assert!(Instant::now() < deadline, "exit not seen after the handoff");
        std::thread::sleep(Duration::from_millis(20));
    }
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A foreground daemon on its own socket, killed with the test however it ends.
struct Daemon {
    child: std::process::Child,
    socket: PathBuf,
    dir: PathBuf,
}

impl Daemon {
    fn start(name: &str) -> Daemon {
        Daemon::start_with(name, &[])
    }

    fn start_with(name: &str, args: &[&str]) -> Daemon {
        let dir = temp(name);
        let socket = dir.join("run/o.sock");
        let child = valk(&socket, &dir)
            .arg("daemon")
            .args(args)
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "daemon did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        Daemon { child, socket, dir }
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = valk(&self.socket, &self.dir).args(args).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "{args:?}: {text}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        text
    }

    fn wait_for(&self, args: &[&str], want: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let text = self.ok(args);
            if text.contains(want) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "no {want:?} in {args:?}:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Whether the daemon says it is a development one (`Hello::dev`).
    fn dev(&self) -> bool {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (client, _) = valkyrie_proto::client::Client::connect(&self.socket)
                .await
                .unwrap();
            client.hello_info().await.unwrap().dev
        })
    }

    /// Pids of the daemon's children whose command line contains `marker`.
    fn children(&self, marker: &str) -> Vec<u32> {
        let ppid = self.child.id().to_string();
        std::fs::read_dir("/proc")
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<u32>().ok())
            .filter(|pid| {
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
                let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
                stat.rsplit_once(')')
                    .and_then(|(_, rest)| rest.split_whitespace().nth(1))
                    == Some(ppid.as_str())
                    && String::from_utf8_lossy(&cmd).contains(marker)
            })
            .collect()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// ADR-0010: only `valk daemon --dev` lets browsers in with the dev token. The
/// mode crosses an upgrade from the image handing off, so a dev daemon stays one and
/// an upgrade can't make a daemon you use into one.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "upgrade hands off on unix")]
fn only_a_dev_daemon_says_so_and_an_upgrade_keeps_its_mode() {
    for (name, args, dev) in [
        ("dev-mode", &["--dev"][..], true),
        ("plain-mode", &[][..], false),
    ] {
        let d = Daemon::start_with(name, args);
        assert_eq!(d.dev(), dev, "{name} before the upgrade");
        let text = d.ok(&["upgrade", "--exe", BIN]);
        assert!(text.contains("generation 1"), "{text}");
        assert_eq!(d.dev(), dev, "{name} after the upgrade");
    }
}

/// After a handoff `Hello` names the new binary, which clients follow, and `valk
/// upgrade` names the connected clients that came back on another build.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "upgrade hands off on unix")]
fn upgrade_names_the_clients_left_on_an_old_build() {
    use std::os::unix::fs::MetadataExt;
    use valkyrie_proto::ClientBuild;
    use valkyrie_proto::client::Client;
    let d = Daemon::start("left-behind");
    let bin = std::fs::metadata(BIN).unwrap();
    // Live pids: `valk upgrade` stops waiting for one that is gone.
    let mut stale = Command::new("sleep").arg("30").spawn().unwrap();
    let following = ClientBuild {
        kind: "tui".into(),
        pid: std::process::id(),
        protocol: valkyrie_proto::PROTOCOL,
        binary: Some((bin.dev(), bin.ino())),
    };
    let behind = ClientBuild {
        kind: "web".into(),
        pid: stale.id(),
        binary: Some((0, 0)),
        ..following.clone()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    // Clients that reconnect after the handoff, as the TUI and `valk web` do.
    for build in [following, behind] {
        let socket = d.socket.clone();
        rt.spawn(async move {
            loop {
                if let Ok((client, mut pushes)) = Client::connect(&socket).await
                    && client.hello_as(build.clone()).await.is_ok()
                {
                    while pushes.recv().await.is_some() {}
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
    }
    let hello = || {
        rt.block_on(async {
            let (client, _) = Client::connect(&d.socket).await.unwrap();
            client.hello_info().await.unwrap()
        })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while rt.block_on(async {
        let (client, _) = Client::connect(&d.socket).await.unwrap();
        client.clients().await.unwrap().len()
    }) < 2
    {
        assert!(Instant::now() < deadline, "clients did not connect");
        std::thread::sleep(Duration::from_millis(20));
    }

    let out = valk(&d.socket, &d.dir)
        .env("VALK_FOLLOW_WAIT_MS", "2000")
        .args(["upgrade", "--exe", BIN])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{text}");
    assert!(text.contains("generation 1"), "{text}");
    assert!(text.contains("waiting for 2 clients to follow"), "{text}");
    let behind = format!(
        "valk web (pid {}) still runs another valk than the daemon",
        stale.id()
    );
    assert!(text.contains(&behind), "{text}");
    assert!(
        !text.contains(&format!("pid {})", std::process::id())),
        "{text}"
    );
    let _ = stale.kill();
    let _ = stale.wait();
    assert_eq!(
        hello().exe,
        Some(valkyrie_proto::canonical(Path::new(BIN)).unwrap())
    );
}

/// A `valk web` with no terminal and no parents (double-forked and `setsid`) may
/// have been started by anyone, an agent included, so it must not print a pairing
/// link: the bridge counts as human only for the devices already paired.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "uses setsid(1)")]
fn a_detached_web_server_prints_no_pairing_link() {
    let d = Daemon::start("detached-web");
    let out = d.dir.join("web.out");
    // The shell leaves at once, so the server is an orphan in a session of its own.
    let pid = Command::new("sh")
        .arg("-c")
        .arg(r#"setsid "$0" web --listen 127.0.0.1:0 >"$1" 2>&1 </dev/null & echo $!"#)
        .arg(BIN)
        .arg(&out)
        .env("VALK_SOCKET", &d.socket)
        .env("XDG_STATE_HOME", &d.dir)
        .env("XDG_CONFIG_HOME", d.dir.join("config"))
        .env_remove("VALK_SESSION")
        .output()
        .unwrap();
    let pid = String::from_utf8_lossy(&pid.stdout).trim().to_owned();
    let deadline = Instant::now() + Duration::from_secs(10);
    let text = loop {
        let text = std::fs::read_to_string(&out).unwrap_or_default();
        if text.contains("pair") {
            break text;
        }
        assert!(
            Instant::now() < deadline,
            "web server said nothing:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = Command::new("kill").arg(&pid).status();
    assert!(!text.contains("#pair="), "printed a pairing link:\n{text}");
    assert!(text.contains("no pairing code"), "{text}");
}

/// Review finding: input queued for a program that isn't reading (its tty buffer is
/// full) used to be dropped by the exec. It must cross the handoff in full.
#[test]
fn upgrade_keeps_input_the_program_has_not_read_yet() {
    let d = Daemon::start("upgrade-input");
    d.ok(&[
        "new",
        "--",
        "bash",
        "--norc",
        "-c",
        "stty raw -echo; sleep 2; head -c 300000 | wc -c; exec sleep 100",
    ]);
    std::thread::sleep(Duration::from_millis(300));
    let chunk = "x".repeat(100_000);
    for _ in 0..3 {
        d.ok(&["send", "1", &chunk]);
    }
    let text = d.ok(&["upgrade", "--exe", BIN]);
    assert!(text.contains("1/1 sessions kept"), "{text}");
    d.wait_for(&["dump", "1"], "300000");
}

/// Review findings: a session whose transcript is gone must still be adopted (blank
/// screen, live program), and a killed program that ignores SIGHUP must not survive
/// the handoff or be left unreaped.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "reads /proc")]
fn upgrade_survives_a_lost_transcript_and_finishes_pending_kills() {
    let d = Daemon::start("upgrade-edges");
    d.ok(&[
        "new",
        "--",
        "bash",
        "--norc",
        "-c",
        "trap '' HUP; exec -a stubborn-marker sleep 1000",
    ]);
    let script = r#"while read -r line; do echo "got: $line"; done"#;
    d.ok(&["new", "--", "bash", "--norc", "-c", script]);
    let stubborn = d.children("stubborn-marker");
    assert_eq!(stubborn.len(), 1, "{stubborn:?}");
    d.ok(&["kill", "1"]);
    for entry in std::fs::read_dir(d.dir.join("valkyrie/sessions")).unwrap() {
        let path = entry.unwrap().path();
        if path.to_string_lossy().ends_with("-2.raw") {
            std::fs::remove_file(path).unwrap();
        }
    }
    let text = d.ok(&["upgrade", "--exe", BIN]);
    assert!(text.contains("1/1 sessions kept"), "{text}");
    d.ok(&["send", "2", r"still here\r"]);
    d.wait_for(&["dump", "2"], "got: still here");
    let deadline = Instant::now() + Duration::from_secs(3);
    while Path::new(&format!("/proc/{}", stubborn[0])).exists() {
        assert!(
            Instant::now() < deadline,
            "killed program survived the handoff (or is a zombie)"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Review finding: killed sessions were kept alive by the handoff's kill list, so each
/// kill leaked its PTY fds and writer thread. A killed, reaped session must free them.
#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "reads /proc")]
fn killed_sessions_release_their_fds() {
    let d = Daemon::start("kill-fds");
    let fds = || {
        std::fs::read_dir(format!("/proc/{}/fd", d.child.id()))
            .unwrap()
            .count()
    };
    d.ok(&["ls"]);
    let baseline = fds();
    for _ in 0..10 {
        d.ok(&["new", "--", "sleep", "1000"]);
    }
    assert!(
        fds() >= baseline + 20,
        "sessions should hold fds: {} vs {baseline}",
        fds()
    );
    for id in 1..=10 {
        d.ok(&["kill", &id.to_string()]);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while fds() > baseline + 2 {
        assert!(
            Instant::now() < deadline,
            "{} fds after killing everything, {baseline} before",
            fds()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn sessions_are_named_after_their_directory_until_renamed() {
    let dir = temp("rename");
    let socket = dir.join("run/o.sock");
    let proj = dir.join("myproj");
    std::fs::create_dir_all(&proj).unwrap();
    let mut daemon = KillOnDrop(
        valk(&socket, &dir)
            .arg("daemon")
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let new = (0..100)
        .find_map(|_| {
            let out = valk(&socket, &dir)
                .args(["new", "--cwd"])
                .arg(&proj)
                .args(["--", "sleep", "30"])
                .output()
                .unwrap();
            if out.status.success() {
                return Some(out);
            }
            std::thread::sleep(Duration::from_millis(20));
            None
        })
        .expect("daemon did not start");
    let id = String::from_utf8(new.stdout).unwrap().trim().to_string();
    let name = || {
        let ls = valk(&socket, &dir).arg("ls").output().unwrap();
        let ls = String::from_utf8(ls.stdout).unwrap();
        ls.split_whitespace().nth(1).unwrap().to_string()
    };
    assert_eq!(name(), "myproj");
    let rename = |args: &[&str]| {
        let out = valk(&socket, &dir)
            .args(["rename", &id])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    rename(&["api-auth"]);
    assert_eq!(name(), "api-auth");
    // Blank goes back to the directory's name, as does no name at all.
    rename(&["  "]);
    assert_eq!(name(), "myproj");
    rename(&["web"]);
    rename(&[]);
    assert_eq!(name(), "myproj");

    // An unnamed shell follows its `cd`.
    let other = dir.join("otherproj");
    std::fs::create_dir_all(&other).unwrap();
    let out = valk(&socket, &dir)
        .args(["new", "--cwd"])
        .arg(&proj)
        .args(["--", "sh"])
        .output()
        .unwrap();
    let sh = String::from_utf8(out.stdout).unwrap().trim().to_string();
    let cd = format!("cd {}\\r", other.display());
    let out = valk(&socket, &dir)
        .args(["send", &sh, &cd])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let followed = (0..50).any(|_| {
        std::thread::sleep(Duration::from_millis(100));
        let ls = valk(&socket, &dir).arg("ls").output().unwrap();
        let ls = String::from_utf8(ls.stdout).unwrap();
        ls.lines()
            .any(|l| l.split_whitespace().take(2).eq([sh.as_str(), "otherproj"]))
    });
    assert!(followed, "the shell's name did not follow its cd");
    let _ = valk(&socket, &dir).args(["kill", &sh]).output();

    let _ = valk(&socket, &dir).args(["kill", &id]).output();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    remove_test_dir(&dir);
}

/// `valk status` (backlog #117): an agent publishes a short status on its own
/// session, a new one replaces it, `--clear` and its TTL take it down, and outside
/// a session it has to be told which.
#[test]
fn an_agent_publishes_a_status_that_expires() {
    let dir = temp("status");
    let socket = dir.join("run/o.sock");
    let mut daemon = KillOnDrop(
        valk(&socket, &dir)
            .arg("daemon")
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let new = (0..100)
        .find_map(|_| {
            let out = valk(&socket, &dir)
                .args(["new", "--cwd"])
                .arg(&dir)
                .args(["--", "sleep", "30"])
                .output()
                .unwrap();
            if out.status.success() {
                return Some(out);
            }
            std::thread::sleep(Duration::from_millis(20));
            None
        })
        .expect("daemon did not start");
    let id = String::from_utf8(new.stdout).unwrap().trim().to_string();
    // As the agent in that session would run it.
    let status = |args: &[&str]| {
        let out = valk(&socket, &dir)
            .env("VALK_SESSION", &id)
            .arg("status")
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    let shown = || {
        let ls = valk(&socket, &dir).arg("ls").output().unwrap();
        let ls = String::from_utf8(ls.stdout).unwrap();
        ls.lines()
            .find_map(|l| l.trim().strip_prefix("▸ ").map(str::to_owned))
    };
    assert_eq!(shown(), None);
    status(&["tests 41/50"]);
    assert_eq!(shown().as_deref(), Some("tests 41/50"));
    status(&["tests\n42/50"]);
    assert_eq!(shown().as_deref(), Some("tests 42/50"));
    status(&["--clear"]);
    assert_eq!(shown(), None);

    status(&["deploying", "--ttl", "1"]);
    assert_eq!(shown().as_deref(), Some("deploying"));
    let gone = (0..40).any(|_| {
        std::thread::sleep(Duration::from_millis(100));
        shown().is_none()
    });
    assert!(gone, "the status outlived its --ttl");

    // Outside a session it needs --session.
    let out = valk(&socket, &dir).args(["status", "x"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--session"));
    let out = valk(&socket, &dir)
        .args(["status", "built", "--session", &id])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(shown().as_deref(), Some("built"));

    let _ = valk(&socket, &dir).args(["kill", &id]).output();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    remove_test_dir(&dir);
}

/// ADR-0007: the context hook answers Claude Code and Codex alike at session start,
/// and stays silent outside a session and on events it has nothing for.
#[test]
fn context_hook_gives_decisions_at_session_start_only() {
    let dir = temp("context-hook");
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let run = |agent: &str, session: bool, payload: String| {
        let mut cmd = valk(&dir.join("run/none.sock"), &dir);
        cmd.env("VALK_CONTEXT", dir.join("context"));
        if session {
            cmd.env("VALK_SESSION", "1");
        }
        let mut child = cmd
            .args(["context-hook", agent])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let start = |event: &str| {
        format!(
            r#"{{"hook_event_name":"{event}","cwd":"{}"}}"#,
            repo.display()
        )
    };
    for agent in ["claude", "codex"] {
        let out = run(agent, true, start("SessionStart"));
        assert!(out.status.success() && out.stderr.is_empty(), "{out:?}");
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "SessionStart");
        let text = v["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(text.contains("decide --kind"), "{text}");
        assert_silent_success(&run(agent, false, start("SessionStart")));
    }
    assert_silent_success(&run("codex", true, start("Stop")));
    assert_silent_success(&run("aider", true, start("SessionStart")));
    remove_test_dir(&dir);
}

/// `valk … | head -1`: a reader that leaves early must end valk quietly, the way it
/// ends `cat`, not with a "failed printing to stdout: Broken pipe" panic (#33).
#[test]
fn a_closed_stdout_ends_valk_quietly() {
    use std::os::unix::process::ExitStatusExt;
    let dir = temp("epipe");
    let (reader, writer) = std::io::pipe().unwrap();
    // Every write valk makes now fails with EPIPE, deterministically.
    drop(reader);
    let out = valk(&dir.join("sock"), &dir)
        .arg("render")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude-long-tool.raw"))
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.is_empty(), "valk complained: {stderr}");
    assert_eq!(out.status.signal(), Some(libc::SIGPIPE), "{:?}", out.status);
    let _ = std::fs::remove_dir_all(&dir);
}

/// `valk pane`: an agent in a session splits its own tab, drives the new pane and
/// reads it back, resizes and closes it, and can't close the pane it runs in.
#[test]
fn an_agent_drives_the_panes_of_its_tab() {
    let dir = temp("pane");
    let socket = dir.join("run/o.sock");
    let mut daemon = KillOnDrop(
        valk(&socket, &dir)
            .arg("daemon")
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let new = (0..100)
        .find_map(|_| {
            let out = valk(&socket, &dir)
                .args(["new", "--cwd"])
                .arg(&dir)
                .args(["--", "sh"])
                .output()
                .unwrap();
            if out.status.success() {
                return Some(out);
            }
            std::thread::sleep(Duration::from_millis(20));
            None
        })
        .expect("daemon did not start");
    let me = String::from_utf8(new.stdout).unwrap().trim().to_string();
    // As the agent in that session would run it.
    let pane = |args: &[&str]| {
        valk(&socket, &dir)
            .env("VALK_SESSION", &me)
            .env("SHELL", "/bin/sh")
            .arg("pane")
            .args(args)
            .output()
            .unwrap()
    };
    let ok = |out: Output| {
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };
    let list = || -> Vec<serde_json::Value> {
        serde_json::from_str(&ok(pane(&["list", "--json"]))).unwrap()
    };

    let solo = list();
    assert_eq!(solo.len(), 1);
    assert_eq!(solo[0]["where"], "whole tab");
    assert_eq!(solo[0]["me"], true);

    let other = ok(pane(&["split", "-d", "right"])).trim().to_string();
    let panes = list();
    let wheres: Vec<_> = panes
        .iter()
        .map(|p| {
            (
                p["id"].to_string(),
                p["where"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        wheres,
        [
            (me.clone(), "left".to_string()),
            (other.clone(), "right".to_string())
        ]
    );
    assert_eq!(panes[1]["me"], false);
    // Canonical: macOS's temp dir is under /var, a link to /private/var.
    let cwd = std::fs::canonicalize(panes[1]["cwd"].as_str().unwrap()).unwrap();
    assert_eq!(
        cwd,
        std::fs::canonicalize(&dir).unwrap(),
        "the split's directory"
    );

    ok(pane(&["run", &other, "echo pane-$((40+2))"]));
    ok(pane(&[
        "wait",
        &other,
        "--match",
        "pane-42",
        "--timeout",
        "10",
    ]));
    assert!(ok(pane(&["read", &other])).contains("pane-42"));
    let tail = ok(pane(&["read", &other, "-n", "1"]));
    assert_eq!(tail.lines().count(), 1, "{tail:?}");
    let timed_out = pane(&["wait", &other, "--match", "never-there", "--timeout", "0"]);
    assert_eq!(timed_out.status.code(), Some(124));
    ok(pane(&[
        "wait",
        &other,
        "--state",
        "stopped",
        "--timeout",
        "10",
    ]));

    // History above, a mostly blank screen: the last lines are the ones written.
    ok(pane(&[
        "run",
        &other,
        "seq 1 60; printf '\\\\033[2J\\\\033[H'; echo after-$((1+1))",
    ]));
    ok(pane(&[
        "wait",
        &other,
        "--match",
        "after-2",
        "--timeout",
        "10",
    ]));
    let tail = ok(pane(&["read", &other, "-n", "30"]));
    let tail: Vec<_> = tail.lines().collect();
    // More than the 24-row screen holds, so history comes along.
    assert_eq!(tail.len(), 30, "{tail:?}");
    assert_eq!(tail[28], "after-2", "{tail:?}");

    ok(pane(&["resize", &other, "30"]));
    let cols = list()[1]["area"]["cols"].as_u64().unwrap();
    assert!(
        (295..=305).contains(&cols),
        "right pane is {cols}/1000 wide"
    );

    let refused = pane(&["close", &me]);
    assert!(!refused.status.success());
    assert!(pane(&["run", "999", "x"]).status.code() != Some(0));
    // A shell that exits leaves the list at once; waiting on it still succeeds.
    let gone = ok(pane(&["split", "-d", "down"])).trim().to_string();
    ok(pane(&["run", &gone, "exit"]));
    ok(pane(&[
        "wait",
        &gone,
        "--state",
        "exited",
        "--timeout",
        "10",
    ]));
    // `prompt`: the pane's agent reports through hooks, fired here by hand.
    let agent = ok(pane(&["split", "-d", "down"])).trim().to_string();
    let fire = |event: &str| {
        let payload = serde_json::json!({"hook_event_name": event, "cwd": dir}).to_string();
        let mut cmd = valk(&socket, &dir);
        cmd.env("VALK_SESSION", &agent);
        assert_silent_success(&hook(cmd, payload.as_bytes()));
    };
    let prompt = |args: &[&str]| {
        valk(&socket, &dir)
            .env("VALK_SESSION", &me)
            .args(["pane", "prompt"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    // Hooks return before the daemon applies them: wait for each state to show.
    let reach = |state: &str| ok(pane(&["wait", &agent, "--state", state, "--timeout", "10"]));
    // The last turn's `done` is on show; only the new turn's end may count.
    fire("UserPromptSubmit");
    reach("working");
    fire("Stop");
    reach("done");
    let mut waiting = prompt(&[&agent, "echo prompt-$((6*7))", "--wait", "--timeout", "20"]);
    ok(pane(&[
        "wait",
        &agent,
        "--match",
        "prompt-42",
        "--timeout",
        "10",
    ]));
    fire("UserPromptSubmit");
    reach("working");
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        waiting.try_wait().unwrap().is_none(),
        "returned before the turn ended"
    );
    fire("Stop");
    let out = waiting.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let said = String::from_utf8(out.stdout).unwrap();
    assert!(
        said.starts_with("done") || said.starts_with("idle"),
        "{said:?}"
    );

    // A turn that stops on a question fails at once, and the next prompt is refused.
    let waiting = prompt(&[&agent, "echo ask-$((6*7))", "--wait", "--timeout", "20"]);
    ok(pane(&[
        "wait",
        &agent,
        "--match",
        "ask-42",
        "--timeout",
        "10",
    ]));
    fire("UserPromptSubmit");
    let ask = serde_json::json!({
        "hook_event_name": "Notification",
        "notification_type": "permission_prompt",
        "message": "Claude needs your permission to use Bash",
        "cwd": dir,
    })
    .to_string();
    let mut cmd = valk(&socket, &dir);
    cmd.env("VALK_SESSION", &agent);
    assert_silent_success(&hook(cmd, ask.as_bytes()));
    let out = waiting.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("needs input"),
        "{out:?}"
    );
    let refused = prompt(&[&agent, "true"]).wait_with_output().unwrap();
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("needs a human first"),
        "{refused:?}"
    );

    // A pane whose program reports nothing: the prompt never lands.
    let started = Instant::now();
    let out = prompt(&[&other, "true", "--wait"])
        .wait_with_output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(started.elapsed() < Duration::from_secs(15));
    ok(pane(&["close", &agent]));

    ok(pane(&["close", &other]));
    let left = list();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["where"], "whole tab");

    let _ = valk(&socket, &dir).args(["kill", &me]).output();
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    remove_test_dir(&dir);
}

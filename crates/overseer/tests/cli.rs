//! The real binary: `overseer hook` contract (ADR-0005) and `overseer setup codex`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_overseer");

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("overseer-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn overseer(socket: &Path, state: &Path) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.env("OVERSEER_SOCKET", socket)
        .env("XDG_STATE_HOME", state)
        .env_remove("OVERSEER_SESSION");
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
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

fn assert_silent_success(out: &Output) {
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty(), "hook printed to stdout: {out:?}");
    assert!(out.stderr.is_empty(), "hook printed to stderr: {out:?}");
}

#[test]
fn hook_is_silent_and_succeeds_outside_overseer_and_without_a_daemon() {
    let dir = temp("silent");
    let socket = dir.join("run/none.sock");
    // Not in a session: a no-op whatever the input.
    assert_silent_success(&hook(overseer(&socket, &dir), b"not json"));
    // In a session but the daemon is gone, or the input is garbage: still silent, 0.
    let mut cmd = overseer(&socket, &dir);
    cmd.env("OVERSEER_SESSION", "1");
    assert_silent_success(&hook(cmd, br#"{"hook_event_name":"Stop"}"#));
    let mut cmd = overseer(&socket, &dir);
    cmd.env("OVERSEER_SESSION", "1");
    assert_silent_success(&hook(cmd, b"\xff garbage"));
    // Arguments that don't parse (empty --socket from the environment): still silent.
    let mut cmd = overseer(&socket, &dir);
    cmd.env("OVERSEER_SOCKET", "").env("OVERSEER_SESSION", "1");
    assert_silent_success(&hook(cmd, b"{}"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn hook_reaches_the_daemon_quickly() {
    let dir = temp("e2e");
    let socket = dir.join("run/o.sock");
    let mut daemon = overseer(&socket, &dir)
        .arg("daemon")
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let new = (0..100)
        .find_map(|_| {
            let out = overseer(&socket, &dir)
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
        let mut cmd = overseer(&socket, &dir);
        cmd.env("OVERSEER_SESSION", &id);
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

    let ls = overseer(&socket, &dir).arg("ls").output().unwrap();
    let ls = String::from_utf8(ls.stdout).unwrap();
    assert!(ls.contains("needs input"), "{ls}");
    assert!(ls.contains("Permission: Bash cargo test"), "{ls}");

    let _ = overseer(&socket, &dir).args(["kill", &id]).output();
    daemon.kill().unwrap();
    daemon.wait().unwrap();

    // The recording replays to the same states it recorded.
    let log = std::fs::read_dir(dir.join("overseer/sessions"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().ends_with(".events.jsonl"))
        .unwrap();
    let out = Command::new(BIN).arg("replay").arg(&log).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let report = String::from_utf8(out.stdout).unwrap();
    assert!(report.contains("needs input"), "{report}");
    assert!(report.contains("0 mismatches"), "{report}");
    std::fs::remove_dir_all(&dir).unwrap();
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
            .args(["setup", "codex", "--exe", "/opt/overseer"])
            .args(extra)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    };

    let dry = setup(&["--dry-run"]);
    assert!(
        dry.contains("'/opt/overseer' hook codex 2>/dev/null || true"),
        "{dry}"
    );
    assert!(
        !std::fs::read_to_string(&hooks)
            .unwrap()
            .contains("overseer")
    );

    assert!(setup(&[]).contains("/hooks"));
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&hooks).unwrap()).unwrap();
    assert_eq!(doc["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(
        doc["hooks"]["Interrupt"][0]["hooks"][0]["command"],
        "'/opt/overseer' hook codex 2>/dev/null || true"
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
    std::fs::remove_dir_all(&dir).unwrap();
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

//! The real binary: `valk hook` contract (ADR-0005) and `valk setup codex`.

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

fn valk(socket: &Path, state: &Path) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.env("VALK_SOCKET", socket)
        .env("XDG_STATE_HOME", state)
        .env_remove("VALK_SESSION");
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
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn hook_reaches_the_daemon_quickly() {
    let dir = temp("e2e");
    let socket = dir.join("run/o.sock");
    let mut daemon = valk(&socket, &dir)
        .arg("daemon")
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
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

/// SSH-resume contract: an auto-started daemon leads its own session (a dropped
/// connection can't hang it up) and its default socket lives under the state dir,
/// named for this host, not under `$XDG_RUNTIME_DIR` (deleted at logout, often unset
/// over SSH).
#[test]
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
    let mut daemon = valk(&socket, &dir)
        .arg("daemon")
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
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
        let dir = temp(name);
        let socket = dir.join("run/o.sock");
        let child = valk(&socket, &dir)
            .arg("daemon")
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

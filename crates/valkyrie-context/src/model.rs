//! The small model the context layer asks (DESIGN §6.3, decided 2026-10-08):
//! `claude -p --model haiku`, locked down, on the user's own Claude login.

use anyhow::{Context, Result, ensure};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// No tools, no hooks, no MCP servers, no settings or CLAUDE.md, no saved session:
/// the model only reads the prompt. Your own login, unlike `--bare`, which never
/// reads it.
const LOCKED_DOWN: &[&str] = &[
    "claude",
    "-p",
    "--safe-mode",
    "--model",
    "haiku",
    "--tools",
    "",
    "--no-session-persistence",
    "--disable-slash-commands",
    "--strict-mcp-config",
    "--setting-sources",
    "",
    "--settings",
    r#"{"disableAllHooks":true}"#,
    "--output-format",
    "json",
    "--system-prompt",
];

/// The command line for a call with `system` as its system prompt. In a debug
/// build, `$VALK_EXTRACTOR` (whitespace-separated) replaces the model: tests put a
/// stand-in there. Never in a release build, where an agent that happened to start
/// the daemon could otherwise route every exchange to a command of its choosing.
pub fn command(system: &str) -> Vec<String> {
    let stand_in = std::env::var("VALK_EXTRACTOR")
        .ok()
        .filter(|_| cfg!(debug_assertions));
    match stand_in {
        Some(cmd) if !cmd.trim().is_empty() => cmd.split_whitespace().map(str::to_owned).collect(),
        _ => LOCKED_DOWN
            .iter()
            .map(|s| s.to_string())
            .chain([system.to_owned()])
            .collect(),
    }
}

/// Runs `command` in `dir` with `prompt` on stdin and returns the answer: `result`
/// of Claude's JSON output, else stdout as it is. Killed past `timeout`.
pub fn ask(command: &[String], prompt: &str, dir: &Path, timeout: Duration) -> Result<String> {
    let (program, args) = command.split_first().context("no model command")?;
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        // Not one of Valkyrie's sessions: no hooks reaching the daemon.
        .env_remove("VALK_SESSION")
        .env_remove("VALK_SOCKET")
        .env_remove("VALK_CONTEXT")
        .env_remove("CLAUDECODE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Its own group, so a timeout kills whatever it started too.
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().with_context(|| format!("run {program}"))?;
    let group = child.id() as i32;
    let kill_group = || kill_tree(group);
    let mut stdin = child.stdin.take().unwrap();
    let prompt = prompt.to_owned();
    let writer = std::thread::spawn(move || stdin.write_all(prompt.as_bytes()));
    let mut stdout = child.stdout.take().unwrap();
    let (sent, read) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = sent.send(stdout.read_to_end(&mut out).map(|_| out));
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() > timeout {
            kill_group();
            let _ = child.wait();
            anyhow::bail!("{program} took over {}s", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // Something it started may hold stdout open after it exits; don't wait on that.
    let left = timeout
        .saturating_sub(start.elapsed())
        .max(Duration::from_secs(1));
    let out = match read.recv_timeout(left) {
        Ok(out) => out?,
        Err(_) => {
            kill_group();
            anyhow::bail!("{program} left its output open");
        }
    };
    let _ = writer.join();
    ensure!(status.success(), "{program} exited {status}");
    let text = String::from_utf8_lossy(&out).into_owned();
    Ok(serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v["result"].as_str().map(str::to_owned))
        .unwrap_or(text))
}

/// Kills `pid` and whatever it started.
#[cfg(unix)]
fn kill_tree(group: i32) {
    // SAFETY: signals the group led by our own child.
    unsafe { libc::kill(-group, libc::SIGKILL) };
}

/// Kills `pid` and whatever it started.
#[cfg(windows)]
fn kill_tree(pid: i32) {
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn reads_the_result_or_plain_output_and_gives_up_in_time() {
        let dir = std::env::temp_dir();
        let sh = |script: &str| vec!["sh".to_string(), "-c".into(), script.into()];
        let t = Duration::from_secs(5);
        assert_eq!(
            ask(
                &sh(r#"cat >/dev/null; echo '{"result":"hi"}'"#),
                "x",
                &dir,
                t
            )
            .unwrap(),
            "hi"
        );
        assert_eq!(ask(&sh("cat"), "plain", &dir, t).unwrap(), "plain");
        assert!(ask(&sh("exit 3"), "", &dir, t).is_err());
        let start = Instant::now();
        assert!(ask(&sh("sleep 5"), "", &dir, Duration::from_millis(200)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        // Exits, but a child it left behind keeps stdout open.
        let start = Instant::now();
        assert!(
            ask(
                &sh("sleep 5 & echo hi"),
                "",
                &dir,
                Duration::from_millis(300)
            )
            .is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(3));
    }
}

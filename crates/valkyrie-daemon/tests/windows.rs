//! End-to-end on Windows: a real daemon on a named pipe, real ConPTYs, the public
//! client. The unix suite (`daemon.rs`) leans on `sh`; these use `cmd.exe`.
#![cfg(windows)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use valkyrie_proto::client::{Client, Pushes};
use valkyrie_proto::{Size, SpawnSpec};

async fn start() -> (Client, Pushes, PathBuf) {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "valkyrie-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let socket = dir.join("run").join("o.sock");
    let state = dir.join("state");
    let serve_socket = socket.clone();
    tokio::spawn(async move {
        valkyrie_daemon::run(&serve_socket, &state, std::path::Path::new("valk")).await
    });
    for _ in 0..200 {
        if let Ok((client, pushes)) = Client::connect(&socket).await {
            return (client, pushes, dir);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("daemon did not start");
}

fn cmd(args: &[&str]) -> SpawnSpec {
    SpawnSpec {
        command: ["cmd.exe", "/d", "/q"]
            .iter()
            .chain(args)
            .map(|a| a.to_string())
            .collect(),
        cwd: Some(std::env::temp_dir()),
        name: None,
        size: Size { cols: 80, rows: 20 },
        env: Vec::new(),
    }
}

/// Waits until `dump` satisfies `pred`, returning the last dump.
async fn wait_dump(client: &Client, id: u32, pred: impl Fn(&str) -> bool) -> String {
    let mut text = String::new();
    for _ in 0..500 {
        text = client.dump(id).await.unwrap();
        if pred(&text) {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("screen never matched; last:\n{text}");
}

#[tokio::test]
async fn output_input_and_kill_through_a_conpty() {
    let (client, _pushes, _dir) = start().await;
    let id = client.spawn(cmd(&["/k"])).await.unwrap().id;
    client
        .input(id, b"echo typed-%NUMBER_OF_PROCESSORS%-ok\r".to_vec())
        .unwrap();
    wait_dump(&client, id, |t| {
        t.lines()
            .any(|l| l.starts_with("typed-") && l.ends_with("-ok"))
    })
    .await;

    // Killing ends the program's job, its children with it.
    client.kill(id).await.unwrap();
    for _ in 0..200 {
        let sessions = client.list().await.unwrap();
        if !sessions.iter().any(|s| s.id == id) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("killed session still listed");
}

#[tokio::test]
async fn a_program_that_exits_reports_its_code() {
    let (client, _pushes, _dir) = start().await;
    let id = client.spawn(cmd(&["/c", "exit 3"])).await.unwrap().id;
    for _ in 0..500 {
        let sessions = client.list().await.unwrap();
        if let Some(s) = sessions.iter().find(|s| s.id == id)
            && s.exited.is_some()
        {
            assert_eq!(s.exited, Some(Some(3)));
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("exit never reported");
}

#[tokio::test]
async fn a_second_daemon_on_the_same_socket_is_refused() {
    let (_client, _pushes, dir) = start().await;
    let socket = dir.join("run").join("o.sock");
    let err = valkyrie_daemon::run(&socket, &dir.join("state2"), std::path::Path::new("valk"))
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("already listening"), "{err:#}");
}

#[tokio::test]
async fn the_shell_session_follows_cmds_directory() {
    let (client, _pushes, _dir) = start().await;
    let id = client.spawn(cmd(&["/k"])).await.unwrap().id;
    let target = std::env::temp_dir().join(format!("valkyrie-cd-{}", std::process::id()));
    std::fs::create_dir_all(&target).unwrap();
    let line = format!("cd /d \"{}\"\r", target.display());
    client.input(id, line.into_bytes()).unwrap();
    let canonical = valkyrie_proto::canonical(&target).unwrap();
    for _ in 0..200 {
        let sessions = client.list().await.unwrap();
        let cwd = &sessions.iter().find(|s| s.id == id).unwrap().cwd;
        if valkyrie_proto::canonical(cwd).ok().as_ref() == Some(&canonical) {
            client.kill(id).await.unwrap();
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("session never followed the cd");
}

/// The npm launcher is a batch file, which CreateProcess cannot run directly.
/// Its Node entry point must receive JSON settings and user arguments literally.
#[tokio::test]
async fn npm_agent_launcher_preserves_arguments() {
    let (client, _pushes, dir) = start().await;
    let npm = dir.join("npm with spaces");
    let package = npm.join("node_modules/@anthropic-ai/claude-code");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        npm.join("claude.cmd"),
        r#"@node "%~dp0\node_modules\@anthropic-ai\claude-code\cli.js" %*"#,
    )
    .unwrap();
    std::fs::write(
        package.join("cli.js"),
        "process.stdout.write('\\r\\nARG=' + JSON.stringify(process.argv.at(-1)) + '\\r\\n'); setInterval(() => {}, 1000);",
    )
    .unwrap();
    let arg = "hello \"world\" & 100% café";
    let mut spec = cmd(&[]);
    spec.command = vec![npm.join("claude.cmd").display().to_string(), arg.into()];
    let id = client.spawn(spec).await.unwrap().id;
    let expected = format!("ARG={}", serde_json::to_string(arg).unwrap());
    wait_dump(&client, id, |text| text.contains(&expected)).await;
    client.kill(id).await.unwrap();
}

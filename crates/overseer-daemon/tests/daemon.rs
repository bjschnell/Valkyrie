//! End-to-end: real daemon on a temp socket, real PTYs, the public client.

use overseer_proto::client::{Client, Pushes};
use overseer_proto::{ScreenUpdate, ServerMsg, Size, SpawnSpec};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

async fn start() -> (Client, Pushes, PathBuf) {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "overseer-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let socket = dir.join("run/o.sock");
    let state = dir.join("state");
    let serve_socket = socket.clone();
    tokio::spawn(async move { overseer_daemon::run(&serve_socket, &state).await });
    for _ in 0..100 {
        if let Ok((client, pushes)) = Client::connect(&socket).await {
            return (client, pushes, dir);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("daemon did not start");
}

fn sh(script: &str, size: Size) -> SpawnSpec {
    SpawnSpec {
        command: vec!["sh".into(), "-c".into(), script.into()],
        cwd: Some(std::env::temp_dir()),
        name: None,
        size,
    }
}

const SIZE: Size = Size { cols: 40, rows: 10 };

/// Waits until `dump` satisfies `pred`, returning the last dump.
async fn wait_dump(client: &Client, id: u32, pred: impl Fn(&str) -> bool) -> String {
    let mut text = String::new();
    for _ in 0..200 {
        text = client.dump(id).await.unwrap();
        if pred(&text) {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("screen never matched; last:\n{text}");
}

async fn next_push(pushes: &mut Pushes) -> ServerMsg {
    tokio::time::timeout(Duration::from_secs(3), pushes.recv())
        .await
        .expect("timed out waiting for push")
        .expect("connection closed")
}

#[tokio::test]
async fn attach_starts_with_full_snapshot_of_existing_output() {
    let (client, mut pushes, _dir) = start().await;
    let id = client
        .spawn(sh("printf before-attach; sleep 5", SIZE))
        .await
        .unwrap()
        .id;
    wait_dump(&client, id, |t| t.contains("before-attach")).await;

    client.attach(id, SIZE).await.unwrap();
    let ServerMsg::Screen { update, .. } = next_push(&mut pushes).await else {
        panic!("expected screen")
    };
    assert!(update.full);
    assert_eq!(update.rows.len(), SIZE.rows as usize);
    assert_eq!(update.rows[0].spans[0].text, "before-attach");
    client.kill(id).await.unwrap();
}

#[tokio::test]
async fn input_round_trips_and_diffs_follow() {
    let (client, mut pushes, _dir) = start().await;
    // `R` marks that stty has applied, so only cat (not the tty) echoes.
    let script = "stty raw -echo; printf R; exec cat";
    let id = client.spawn(sh(script, SIZE)).await.unwrap().id;
    wait_dump(&client, id, |t| t == "R\n").await;
    client.attach(id, SIZE).await.unwrap();
    let _snapshot = next_push(&mut pushes).await;

    client.input(id, b"xyz".to_vec()).unwrap();
    let mut last = None;
    for _ in 0..20 {
        if let ServerMsg::Screen { update, .. } = next_push(&mut pushes).await {
            let done = update.cursor.x == 4;
            last = Some(update);
            if done {
                break;
            }
        }
    }
    let update: ScreenUpdate = last.unwrap();
    assert!(!update.full);
    assert_eq!(update.cursor.x, 4);
    assert_eq!(client.dump(id).await.unwrap(), "Rxyz\n");
    client.kill(id).await.unwrap();
}

#[tokio::test]
async fn resize_reaches_the_program() {
    let (client, _pushes, _dir) = start().await;
    let id = client
        .spawn(sh("read x; stty size; sleep 5", SIZE))
        .await
        .unwrap()
        .id;
    client
        .attach(id, Size { cols: 50, rows: 12 })
        .await
        .unwrap();
    client.input(id, b"\r".to_vec()).unwrap();
    let text = wait_dump(&client, id, |t| t.contains("12 50")).await;
    assert!(text.contains("12 50"), "{text}");
    client.kill(id).await.unwrap();
}

#[tokio::test]
async fn attaching_after_exit_still_reports_exit_code() {
    let (client, mut pushes, _dir) = start().await;
    let id = client
        .spawn(sh("printf bye; exit 3", SIZE))
        .await
        .unwrap()
        .id;
    for _ in 0..200 {
        if client.list().await.unwrap()[0].exited.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(client.list().await.unwrap()[0].exited, Some(Some(3)));

    client.attach(id, SIZE).await.unwrap();
    assert!(matches!(
        next_push(&mut pushes).await,
        ServerMsg::Screen { .. }
    ));
    assert_eq!(
        next_push(&mut pushes).await,
        ServerMsg::Exited {
            session: id,
            code: Some(3)
        }
    );
    assert_eq!(client.dump(id).await.unwrap(), "bye\n");
}

#[tokio::test]
async fn kill_removes_session_and_unknown_ids_error() {
    let (client, _pushes, dir) = start().await;
    let id = client.spawn(sh("sleep 30", SIZE)).await.unwrap().id;
    assert_eq!(client.list().await.unwrap().len(), 1);
    client.kill(id).await.unwrap();
    assert!(client.list().await.unwrap().is_empty());
    assert!(client.kill(id).await.is_err());
    assert!(client.attach(999, SIZE).await.is_err());
    let transcripts: Vec<_> = std::fs::read_dir(dir.join("state/sessions"))
        .unwrap()
        .collect();
    assert_eq!(transcripts.len(), 1);
    let name = transcripts[0].as_ref().unwrap().file_name();
    assert!(
        name.to_string_lossy().ends_with(&format!("-{id}.raw")),
        "{name:?}"
    );
}

#[tokio::test]
async fn spawn_failure_is_an_error_reply() {
    let (client, _pushes, _dir) = start().await;
    let mut spec = sh("true", SIZE);
    spec.command = vec!["/nonexistent/agent".into()];
    assert!(client.spawn(spec.clone()).await.is_err());
    spec.command = vec![];
    assert!(client.spawn(spec).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn input_to_a_program_not_reading_stdin_does_not_stall_the_daemon() {
    let (client, _pushes, _dir) = start().await;
    let stuck = client.spawn(sh("sleep 30", SIZE)).await.unwrap().id;
    let live = client
        .spawn(sh("printf ok; sleep 30", SIZE))
        .await
        .unwrap()
        .id;
    // Far more than the tty input buffer holds; a blocking write would wedge a worker.
    for _ in 0..64 {
        client.input(stuck, vec![b'x'; 64 * 1024]).unwrap();
    }
    let text = tokio::time::timeout(
        Duration::from_secs(2),
        wait_dump(&client, live, |t| t.contains("ok")),
    )
    .await
    .expect("daemon stalled");
    assert!(text.contains("ok"));
    client.kill(stuck).await.unwrap();
    client.kill(live).await.unwrap();
}

#[tokio::test]
async fn exit_is_detected_while_a_background_job_holds_the_tty() {
    let (client, _pushes, _dir) = start().await;
    let id = client
        .spawn(sh("sleep 30 & exit 4", SIZE))
        .await
        .unwrap()
        .id;
    let mut exited = None;
    for _ in 0..200 {
        exited = client.list().await.unwrap()[0].exited;
        if exited.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(exited, Some(Some(4)));
    client.kill(id).await.unwrap();
}

#[tokio::test]
async fn kill_takes_down_the_whole_process_group() {
    let (client, _pushes, _dir) = start().await;
    let id = client
        .spawn(sh("sleep 300 & echo bg=$!; wait", SIZE))
        .await
        .unwrap()
        .id;
    let text = wait_dump(&client, id, |t| t.contains("bg=")).await;
    let bg: i32 = text
        .trim()
        .trim_start_matches("bg=")
        .parse()
        .expect("background pid");
    client.kill(id).await.unwrap();
    let alive = |pid| unsafe { libc::kill(pid, 0) == 0 };
    for _ in 0..300 {
        if !alive(bg) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("background job {bg} survived kill");
}

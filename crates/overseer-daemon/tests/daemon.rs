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
    tokio::spawn(async move {
        overseer_daemon::run(&serve_socket, &state, std::path::Path::new("overseer")).await
    });
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
        .filter(|e| e.as_ref().unwrap().path().extension() == Some("raw".as_ref()))
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

// ---- M1: agent state and the attention queue (DESIGN §14) ----

use overseer_proto::{AgentState, AskKind, QueueItem};
use serde_json::json;

/// Waits for a `Queue` push satisfying `pred`, skipping other pushes.
async fn wait_queue(pushes: &mut Pushes, pred: impl Fn(&[QueueItem]) -> bool) -> Vec<QueueItem> {
    let mut last = Vec::new();
    for _ in 0..50 {
        match tokio::time::timeout(Duration::from_secs(3), pushes.recv()).await {
            Ok(Some(ServerMsg::Queue { items })) => {
                if pred(&items) {
                    return items;
                }
                last = items;
            }
            Ok(Some(_)) => {}
            Ok(None) => panic!("connection closed"),
            Err(_) => break,
        }
    }
    panic!("queue never matched; last: {last:?}");
}

fn hook_at(client: &Client, id: u32, t: u64, payload: serde_json::Value) {
    client.hook(id, "claude", t, payload).unwrap();
}

#[tokio::test]
async fn hooks_drive_state_and_the_queue() {
    let (client, mut pushes, _dir) = start().await;
    let id = client.spawn(sh("sleep 30", SIZE)).await.unwrap().id;
    client.watch_queue().await.unwrap();
    wait_queue(&mut pushes, |q| q.is_empty()).await;

    hook_at(
        &client,
        id,
        1,
        json!({"hook_event_name": "UserPromptSubmit"}),
    );
    hook_at(
        &client,
        id,
        2,
        json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash",
               "tool_input": {"command": "cargo test"}}),
    );
    let q = wait_queue(&mut pushes, |q| !q.is_empty()).await;
    assert_eq!(q[0].session, id);
    assert_eq!(q[0].status.state, AgentState::NeedsInput);
    assert_eq!(q[0].status.ask, Some(AskKind::Permission));
    assert_eq!(
        q[0].status.summary.as_deref(),
        Some("Permission: Bash cargo test")
    );
    assert!(q[0].status.hooked);

    // Approval shows up only as the tool finishing.
    hook_at(
        &client,
        id,
        3,
        json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"}),
    );
    wait_queue(&mut pushes, |q| q.is_empty()).await;
    assert_eq!(
        client.list().await.unwrap()[0].status.state,
        AgentState::Working
    );

    hook_at(
        &client,
        id,
        4,
        json!({"hook_event_name": "Stop", "last_assistant_message": "Ran it.\nAll tests pass."}),
    );
    let q = wait_queue(&mut pushes, |q| !q.is_empty()).await;
    assert_eq!(q[0].status.state, AgentState::ReviewReady);
    assert!(
        q[0].status
            .summary
            .as_deref()
            .unwrap()
            .starts_with("All tests pass."),
        "{:?}",
        q[0].status.summary
    );
    client.mark_seen(id, q[0].status.seq).await.unwrap();
    wait_queue(&mut pushes, |q| q.is_empty()).await;
    client.kill(id).await.unwrap();
}

#[tokio::test]
async fn attaching_marks_a_finished_turn_seen_but_not_an_open_question() {
    let (client, _pushes, dir) = start().await;
    let id = client.spawn(sh("sleep 30", SIZE)).await.unwrap().id;
    // A second connection watches, so attach pushes don't interleave with the queue.
    let (watch, mut queue) = Client::connect(&dir.join("run/o.sock")).await.unwrap();
    watch.watch_queue().await.unwrap();

    hook_at(&client, id, 1, json!({"hook_event_name": "Stop"}));
    wait_queue(&mut queue, |q| q.len() == 1).await;
    client.attach(id, SIZE).await.unwrap();
    wait_queue(&mut queue, |q| q.is_empty()).await;
    client.detach().await.unwrap();

    hook_at(
        &client,
        id,
        2,
        json!({"hook_event_name": "PreToolUse", "tool_name": "AskUserQuestion",
               "tool_input": {"questions": [{"question": "Which DB?"}]}}),
    );
    wait_queue(&mut queue, |q| q.len() == 1).await;
    client.attach(id, SIZE).await.unwrap();
    // Still waiting for an answer: it stays queued while attached.
    let s = &client.list().await.unwrap()[0].status;
    assert_eq!(s.state, AgentState::NeedsInput);
    assert!(s.seen);
    assert!(overseer_agents::queued(s));
    client.kill(id).await.unwrap();
}

#[tokio::test]
async fn bell_and_failed_exit_queue_plain_programs() {
    let (client, mut pushes, _dir) = start().await;
    client.watch_queue().await.unwrap();
    let bell = client
        .spawn(sh("printf '\\a'; sleep 30", SIZE))
        .await
        .unwrap()
        .id;
    let q = wait_queue(&mut pushes, |q| q.len() == 1).await;
    assert_eq!(q[0].session, bell);
    assert_eq!(q[0].status.ask, Some(AskKind::Bell));
    assert_eq!(q[0].status.agent, "generic");

    let failed = client.spawn(sh("exit 3", SIZE)).await.unwrap().id;
    let q = wait_queue(&mut pushes, |q| q.len() == 2).await;
    let item = q.iter().find(|i| i.session == failed).unwrap();
    assert_eq!(item.status.state, AgentState::Blocked);
    assert_eq!(item.status.summary.as_deref(), Some("exited with code 3"));
    // Needs input ranks above blocked.
    assert_eq!(q[0].session, bell);
    client.kill(bell).await.unwrap();
}

#[tokio::test]
async fn claude_sessions_get_hook_settings_and_screen_heuristics() {
    let (client, mut pushes, dir) = start().await;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("claude");
    // Echoes what overseer passed it, then shows a folder-trust prompt.
    std::fs::write(
        &fake,
        "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$1\" \"$OVERSEER_SESSION\" \"$OVERSEER_SOCKET\"\n\
         printf 'Quick safety check: Is this a project you created or one you trust?\\n'\nsleep 30\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    client.watch_queue().await.unwrap();
    let mut spec = sh("", SIZE);
    spec.command = vec![fake.to_string_lossy().into_owned()];
    let id = client.spawn(spec).await.unwrap().id;

    let q = wait_queue(&mut pushes, |q| q.len() == 1).await;
    assert_eq!(q[0].status.agent, "claude");
    assert_eq!(q[0].status.ask, Some(AskKind::Screen));
    assert!(!q[0].status.hooked);
    let text = client.dump(id).await.unwrap().replace('\n', "");
    let socket = std::path::absolute(dir.join("run/o.sock")).unwrap();
    assert!(
        text.starts_with(&format!("--settings|{id}|{}", socket.display())),
        "{text}"
    );
    // The displayed command is what the user typed, not the hook-augmented one.
    assert_eq!(client.list().await.unwrap()[0].command.len(), 1);
    client.kill(id).await.unwrap();
}

#[tokio::test]
async fn every_session_writes_a_replayable_event_log() {
    let (client, _pushes, dir) = start().await;
    let id = client
        .spawn(sh("printf hi; sleep 30", SIZE))
        .await
        .unwrap()
        .id;
    wait_dump(&client, id, |t| t.contains("hi")).await;
    hook_at(
        &client,
        id,
        1,
        json!({"hook_event_name": "UserPromptSubmit"}),
    );
    for _ in 0..200 {
        if client.list().await.unwrap()[0].status.state == AgentState::Working {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    client.kill(id).await.unwrap();
    let log = std::fs::read_dir(dir.join("state/sessions"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().ends_with(".events.jsonl"))
        .unwrap();
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let kinds: Vec<&str> = lines.iter().map(|l| l["k"].as_str().unwrap()).collect();
    assert_eq!(kinds[0], "start");
    assert!(kinds.contains(&"out"), "{kinds:?}");
    let hook = lines.iter().find(|l| l["k"] == "hook").unwrap();
    assert_eq!(hook["payload"]["hook_event_name"], "UserPromptSubmit");
    assert_eq!(hook["off"], 2, "points just past `hi` in the transcript");
    let state = lines.iter().find(|l| l["k"] == "state").unwrap();
    assert_eq!(state["status"]["state"], "working");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

// ---- Review regressions ----

#[tokio::test]
async fn a_client_dying_with_unread_frames_still_detaches() {
    let (client, _pushes, dir) = start().await;
    let id = client
        .spawn(sh("while :; do echo tick; sleep 0.01; done", SIZE))
        .await
        .unwrap()
        .id;
    let mut raw = tokio::net::UnixStream::connect(dir.join("run/o.sock"))
        .await
        .unwrap();
    overseer_proto::codec::write_frame(
        &mut raw,
        &overseer_proto::ClientMsg::Attach {
            req: 1,
            session: id,
            size: SIZE,
        },
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(client.list().await.unwrap()[0].clients, 1);
    // Closing with unread data in the receive buffer sends RST: the daemon sees
    // ECONNRESET, not EOF.
    drop(raw);
    for _ in 0..200 {
        if client.list().await.unwrap()[0].clients == 0 {
            client.kill(id).await.unwrap();
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("attach count leaked");
}

#[tokio::test]
async fn hooks_from_a_different_agent_are_ignored_unless_the_session_is_plain() {
    let (client, _pushes, dir) = start().await;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("claude");
    std::fs::write(&fake, "#!/bin/sh\nsleep 30\n").unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut spec = sh("", SIZE);
    spec.command = vec![fake.to_string_lossy().into_owned()];
    let claude = client.spawn(spec).await.unwrap().id;
    let shell = client.spawn(sh("sleep 30", SIZE)).await.unwrap().id;

    // A `codex exec` run by Claude inherits OVERSEER_SESSION.
    let prompt = json!({"hook_event_name": "UserPromptSubmit"});
    client.hook(claude, "codex", 1, prompt.clone()).unwrap();
    client.hook(shell, "codex", 1, prompt.clone()).unwrap();
    client.hook(shell, "nonsense", 2, json!({"hook_event_name": "Stop"})).unwrap();
    let state = |id| {
        let client = client.clone();
        async move {
            let list = client.list().await.unwrap();
            list.into_iter().find(|s| s.id == id).unwrap().status
        }
    };
    for _ in 0..200 {
        if state(shell).await.state == AgentState::Working {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let s = state(shell).await;
    assert_eq!(s.state, AgentState::Working, "unknown agent must not apply");
    assert!(s.hooked);
    let c = state(claude).await;
    assert_eq!(c.state, AgentState::Idle);
    assert!(!c.hooked);
    assert_eq!(client.hello().await.unwrap(), overseer_proto::PROTOCOL);
    client.kill(claude).await.unwrap();
    client.kill(shell).await.unwrap();
}

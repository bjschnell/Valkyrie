//! End-to-end: real daemon on a temp socket, real PTYs, the public client.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use valkyrie_proto::client::{Client, Pushes};
use valkyrie_proto::{ScreenUpdate, ServerMsg, Size, SpawnSpec};

async fn start() -> (Client, Pushes, PathBuf) {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "valkyrie-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    start_in(&dir, "run/o.sock").await;
    let (client, pushes) = Client::connect(&dir.join("run/o.sock")).await.unwrap();
    (client, pushes, dir)
}

/// A daemon with its state in `dir/state`, listening on `dir/<socket>`.
async fn start_in(dir: &std::path::Path, socket: &str) -> (Client, Pushes) {
    let socket = dir.join(socket);
    let state = dir.join("state");
    let serve_socket = socket.clone();
    tokio::spawn(async move {
        valkyrie_daemon::run(&serve_socket, &state, std::path::Path::new("valk")).await
    });
    for _ in 0..100 {
        if let Ok((client, pushes)) = Client::connect(&socket).await {
            return (client, pushes);
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
        env: Vec::new(),
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

/// Scrolling back reads history the screen lost, and a program's copy (OSC 52)
/// reaches the attached client.
#[tokio::test]
async fn scrollback_and_program_copies_reach_the_client() {
    let (client, mut pushes, _dir) = start().await;
    let id = client
        .spawn(sh(
            "seq 1 30; read go; printf '\\033]52;c;aGk=\\007'; sleep 5",
            SIZE,
        ))
        .await
        .unwrap()
        .id;
    wait_dump(&client, id, |t| t.contains("30")).await;
    let text = |rows: &[valkyrie_proto::Row]| -> Vec<String> {
        rows.iter()
            .map(|r| r.spans.iter().map(|s| s.text.as_str()).collect())
            .collect()
    };
    let (from_top, history, rows) = client
        .scrollback(id, valkyrie_proto::ScrollAnchor::Up(99))
        .await
        .unwrap();
    assert_eq!(from_top, 0);
    assert!(history >= 20, "history {history}");
    assert_eq!(text(&rows)[..3], ["1", "2", "3"]);

    client.attach(id, SIZE).await.unwrap();
    client.input(id, b"\r".to_vec()).unwrap();
    loop {
        if let ServerMsg::Clipboard { session, text } = next_push(&mut pushes).await {
            assert_eq!((session, text.as_str()), (id, "hi"));
            break;
        }
    }
    client.kill(id).await.unwrap();
}

/// DESIGN §8.4: an image a program drew while nobody was attached reaches the client
/// that attaches later, after the screen and asking for no reply; one drawn while
/// attached arrives live, at the cursor.
#[tokio::test]
async fn images_reach_clients_live_and_on_attach() {
    let (client, mut pushes, _dir) = start().await;
    let id = client
        .spawn(sh(
            // Like a real image program: no echo, and the daemon's answers (the
            // terminal's `OK`) read raw.
            "stty -echo; printf 'ab\\033_Ga=T,i=4;AAAA\\033\\\\'; read -r go; \
             printf 'xyz\\033_Ga=p,i=4\\033\\\\'; sleep 5",
            SIZE,
        ))
        .await
        .unwrap()
        .id;
    wait_dump(&client, id, |t| t.contains("ab")).await;
    client.attach(id, SIZE).await.unwrap();
    let ServerMsg::Screen { update, .. } = next_push(&mut pushes).await else {
        panic!("expected the screen first")
    };
    assert!(update.full);
    let ServerMsg::Graphics { x, y, data, .. } = next_push(&mut pushes).await else {
        panic!("expected the replayed image")
    };
    assert_eq!(
        (x, y, data.as_str()),
        (2, 0, "\x1b_Ga=T,i=4,q=2;AAAA\x1b\\")
    );
    client.input(id, b"\r".to_vec()).unwrap();
    loop {
        if let ServerMsg::Graphics { x, y, data, .. } = next_push(&mut pushes).await {
            assert_eq!((x, y, data.as_str()), (5, 0, "\x1b_Ga=p,i=4,q=2\x1b\\"));
            break;
        }
    }
    // The image commands never show as text.
    assert!(!client.dump(id).await.unwrap().contains("_G"));
    client.kill(id).await.unwrap();
}

/// Sessions take login-bound variables from the client that spawns them, not from
/// whichever login started the daemon (DESIGN §8.1); `None` unsets one.
#[tokio::test]
async fn spawn_applies_the_clients_login_env() {
    let (client, _pushes, _dir) = start().await;
    let mut spec = sh(r#"printf '%s|%s' "$SSH_AUTH_SOCK" "${HOME-unset}""#, SIZE);
    spec.env = vec![
        ("SSH_AUTH_SOCK".into(), Some("/tmp/agent.test".into())),
        ("HOME".into(), None),
    ];
    let id = client.spawn(spec).await.unwrap().id;
    wait_dump(&client, id, |t| t.trim_end() == "/tmp/agent.test|unset").await;
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

use serde_json::json;
use valkyrie_proto::{AgentState, AskKind, QueueItem};

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
    assert!(valkyrie_agents::queued(s));
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
    // Echoes what Valkyrie passed it, then shows a folder-trust prompt.
    std::fs::write(
        &fake,
        "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$1\" \"$VALK_SESSION\" \"$VALK_SOCKET\"\n\
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
    valkyrie_proto::codec::write_frame(
        &mut raw,
        &valkyrie_proto::ClientMsg::Attach {
            req: 1,
            session: id,
            size: Some(SIZE),
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

    // A `codex exec` run by Claude inherits VALK_SESSION.
    let prompt = json!({"hook_event_name": "UserPromptSubmit"});
    client.hook(claude, "codex", 1, prompt.clone()).unwrap();
    client.hook(shell, "codex", 1, prompt.clone()).unwrap();
    client
        .hook(shell, "nonsense", 2, json!({"hook_event_name": "Stop"}))
        .unwrap();
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
    assert_eq!(client.hello().await.unwrap(), valkyrie_proto::PROTOCOL);
    client.kill(claude).await.unwrap();
    client.kill(shell).await.unwrap();
}

/// DESIGN §8.3: a fresh daemon brings back what the last one hosted. The agent
/// resumes the conversation its hooks named, the shell starts over, and neither a
/// killed session, a program that ended, nor an arbitrary command comes back.
#[tokio::test]
async fn a_fresh_daemon_restores_agents_and_shells() {
    let (client, _pushes, dir) = start().await;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let claude = bin.join("claude");
    std::fs::write(&claude, "#!/bin/sh\nexec sleep 100\n").unwrap();
    std::fs::set_permissions(&claude, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let spawn = |command: &[&str], name: &str| SpawnSpec {
        command: command.iter().map(|s| s.to_string()).collect(),
        cwd: Some(dir.clone()),
        name: Some(name.into()),
        size: SIZE,
        env: Vec::new(),
    };
    let agent = client
        .spawn(spawn(
            &[claude.to_str().unwrap(), "--model", "x", "hello"],
            "agent",
        ))
        .await
        .unwrap()
        .id;
    client.spawn(spawn(&["sh"], "shell")).await.unwrap();
    client
        .spawn(spawn(&["sleep", "100"], "sleeper"))
        .await
        .unwrap();
    let gone = client.spawn(spawn(&["sh"], "gone")).await.unwrap().id;
    client.spawn(spawn(&["true"], "quick")).await.unwrap();
    client
        .hook(
            agent,
            "claude",
            1,
            serde_json::json!({"hook_event_name": "SessionStart", "source": "startup",
                               "session_id": "conv-1"}),
        )
        .unwrap();
    client.kill(gone).await.unwrap();

    let list = valkyrie_daemon::restore_path(&dir.join("state"), &dir.join("run/o.sock"));
    let names = |text: &str| -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(text).unwrap();
        v["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_owned())
            .collect()
    };
    // `quick` ends at once but stays listed for the exit grace, then goes.
    let mut text = String::new();
    for _ in 0..80 {
        text = std::fs::read_to_string(&list).unwrap_or_default();
        if text.contains("conv-1")
            && !text.is_empty()
            && names(&text) == ["agent", "shell", "sleeper"]
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(names(&text), ["agent", "shell", "sleeper"], "{text}");
    assert!(text.contains("conv-1"), "{text}");

    // The next daemon (the first one stands in for a dead one). Lists are per socket,
    // so another socket starts with nothing to restore until given this one's list.
    let next_list = valkyrie_daemon::restore_path(&dir.join("state"), &dir.join("run/next.sock"));
    assert_ne!(next_list, list);
    // Plus one that cannot come back now (its cwd is gone): it stays listed.
    let mut saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&list).unwrap()).unwrap();
    saved["sessions"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!(
        {"name": "lost", "command": ["sh"], "cwd": dir.join("gone").to_str().unwrap()}));
    std::fs::write(&next_list, saved.to_string()).unwrap();
    let (next, _p) = start_in(&dir, "run/next.sock").await;
    let sessions = next.list().await.unwrap();
    let got: Vec<(&str, Vec<&str>)> = sessions
        .iter()
        .map(|s| {
            (
                s.name.as_str(),
                s.command.iter().map(String::as_str).collect(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                "agent",
                vec![
                    claude.to_str().unwrap(),
                    "--resume",
                    "conv-1",
                    "--model",
                    "x"
                ]
            ),
            ("shell", vec!["sh"]),
        ]
    );
    assert!(sessions.iter().all(|s| s.cwd == dir));
    let mut kept = String::new();
    for _ in 0..30 {
        kept = std::fs::read_to_string(&next_list).unwrap_or_default();
        if !kept.is_empty() && names(&kept).len() == 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(names(&kept), ["lost", "agent", "shell"], "{kept}");
    for s in sessions {
        next.kill(s.id).await.unwrap();
    }
    for s in client.list().await.unwrap() {
        let _ = client.kill(s.id).await;
    }
}

#[tokio::test]
async fn exited_shells_and_successes_leave_the_list_failures_stay() {
    let (client, _pushes, _dir) = start().await;
    let shell = SpawnSpec {
        command: vec!["sh".into()],
        ..sh("", SIZE)
    };
    let exited = client.spawn(shell).await.unwrap().id;
    client.spawn(sh("exit 0", SIZE)).await.unwrap();
    let failed = client.spawn(sh("exit 3", SIZE)).await.unwrap().id;
    // `exit` passes on the last command's code; a shell leaves anyway.
    client.input(exited, b"false; exit\r".to_vec()).unwrap();
    let mut ids = Vec::new();
    for _ in 0..100 {
        ids = client.list().await.unwrap().iter().map(|s| s.id).collect();
        if ids == [failed] {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(ids, [failed], "only the failure is still listed");
    // Kept, unlisted, for the restore list's grace; then dropped altogether.
    let mut dropped = false;
    for _ in 0..100 {
        if client.dump(exited).await.is_err() {
            dropped = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dropped, "an exited shell was never dropped");
    client.kill(failed).await.unwrap();
}

#[tokio::test]
async fn move_reorders_the_list() {
    let (client, _pushes, _dir) = start().await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(client.spawn(sh("sleep 30", SIZE)).await.unwrap().id);
    }
    let order =
        |list: Vec<valkyrie_proto::SessionInfo>| list.iter().map(|s| s.id).collect::<Vec<_>>();
    client.move_session(ids[2], 0).await.unwrap();
    assert_eq!(
        order(client.list().await.unwrap()),
        [ids[2], ids[0], ids[1]]
    );
    // Past the end goes last; a new session goes after them all.
    client.move_session(ids[2], 99).await.unwrap();
    let fourth = client.spawn(sh("sleep 30", SIZE)).await.unwrap().id;
    assert_eq!(
        order(client.list().await.unwrap()),
        [ids[0], ids[1], ids[2], fourth]
    );
    assert!(client.move_session(999, 0).await.is_err());
    for id in ids.into_iter().chain([fourth]) {
        client.kill(id).await.unwrap();
    }
}

#[tokio::test]
async fn a_client_that_stops_reading_holds_a_bounded_backlog_and_resyncs() {
    // A TUI whose terminal stalled (SSH from a sleeping laptop) stops taking pushes.
    let (client, mut pushes, _dir) = start().await;
    let id = client
        .spawn(sh("i=0; while :; do i=$((i+1)); echo line $i; done", SIZE))
        .await
        .unwrap()
        .id;
    client.attach(id, SIZE).await.unwrap();
    // Replies still arrive while the pushes pile up.
    let mut waited = 0;
    while !pushes.take_lagged() {
        client.dump(id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        waited += 1;
        assert!(waited < 200, "never lagged");
    }
    let mut held = 0;
    while let Ok(msg) = pushes.try_recv() {
        assert!(matches!(msg, ServerMsg::Screen { .. }), "{msg:?}");
        held += 1;
    }
    assert!(held <= 1024, "held {held}");
    // Attaching again starts over from a full snapshot.
    client.attach(id, SIZE).await.unwrap();
    loop {
        if let ServerMsg::Screen { update, .. } = next_push(&mut pushes).await
            && update.full
        {
            break;
        }
    }
    client.kill(id).await.unwrap();
}

#[tokio::test]
async fn splits_keep_a_tabs_panes_together() {
    use valkyrie_proto::Side;
    let (client, mut pushes, _dir) = start().await;
    let a = client.spawn(sh("sleep 30", SIZE)).await.unwrap().id;
    let b = client.spawn(sh("sleep 30", SIZE)).await.unwrap().id;
    let c = client
        .split(a, Side::Right, sh("sleep 30", SIZE))
        .await
        .unwrap()
        .id;
    let d = client
        .split(c, Side::Down, sh("sleep 30", SIZE))
        .await
        .unwrap()
        .id;
    assert!(
        client
            .split(999, Side::Up, sh("sleep 30", SIZE))
            .await
            .is_err()
    );
    let tabs = || async {
        let (list, layouts) = client.list_tabs().await.unwrap();
        let order: Vec<u32> = list.iter().map(|s| s.id).collect();
        let panes: Vec<Vec<u32>> = layouts.iter().map(|p| p.sessions()).collect();
        (order, panes)
    };
    // A split pane follows the one it split, so the tab's panes stay together.
    assert_eq!(tabs().await, (vec![a, c, d, b], vec![vec![a, c, d]]));

    client.ratio(a, d, 300).await.unwrap();
    assert!(client.ratio(a, b, 300).await.is_err(), "not split");
    // Moves are by tab: any pane moves the whole tab.
    client.move_session(b, 0).await.unwrap();
    assert_eq!(tabs().await.0, [b, a, c, d]);
    client.move_session(d, 0).await.unwrap();
    assert_eq!(tabs().await.0, [a, c, d, b]);
    client.move_session(b, 1).await.unwrap();
    assert_eq!(
        tabs().await.0,
        [a, c, d, b],
        "one tab, so b is already second"
    );

    // One connection watches every pane, each at its size.
    let small = Size { cols: 20, rows: 5 };
    client
        .attach_panes(vec![(a, SIZE), (c, small), (d, small)])
        .await
        .unwrap();
    let mut seen = std::collections::HashMap::new();
    while seen.len() < 3 {
        if let ServerMsg::Screen { session, update } = next_push(&mut pushes).await {
            seen.entry(session).or_insert(update.size);
        }
    }
    assert_eq!((seen[&a], seen[&c], seen[&d]), (SIZE, small, small));

    // A pane that goes leaves the layout; the last one left is a plain tab.
    client.kill(c).await.unwrap();
    assert_eq!(tabs().await, (vec![a, d, b], vec![vec![a, d]]));
    client.kill(d).await.unwrap();
    assert_eq!(tabs().await, (vec![a, b], vec![]));
    for id in [a, b] {
        client.kill(id).await.unwrap();
    }
}

#[tokio::test]
async fn splits_come_back_after_a_restart() {
    use valkyrie_proto::Side;
    let (client, _pushes, dir) = start().await;
    let shell = |name: &str| SpawnSpec {
        command: vec!["sh".into()],
        cwd: Some(dir.clone()),
        name: Some(name.into()),
        size: SIZE,
        env: Vec::new(),
    };
    let a = client.spawn(shell("left")).await.unwrap().id;
    client.spawn(shell("alone")).await.unwrap();
    let right = client
        .split(a, Side::Right, shell("right"))
        .await
        .unwrap()
        .id;
    let list = valkyrie_daemon::restore_path(&dir.join("state"), &dir.join("run/o.sock"));
    let mut text = String::new();
    for _ in 0..50 {
        text = std::fs::read_to_string(&list).unwrap_or_default();
        if text.contains("layouts") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(text.contains("layouts"), "{text}");
    // A reboot ends the shells before the daemon: one that exits leaves the lists
    // at once, but keeps its place in its split on the restore list.
    client.input(right, b"exit\n".to_vec()).unwrap();
    for _ in 0..50 {
        if client.list().await.unwrap().iter().all(|s| s.id != right) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let (listed, layouts) = client.list_tabs().await.unwrap();
    assert!(listed.iter().all(|s| s.id != right) && layouts.is_empty());
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let text = std::fs::read_to_string(&list).unwrap();
    assert!(text.contains("layouts") && text.contains("right"), "{text}");

    let next_list = valkyrie_daemon::restore_path(&dir.join("state"), &dir.join("run/next.sock"));
    std::fs::write(&next_list, &text).unwrap();
    let (next, _p) = start_in(&dir, "run/next.sock").await;
    let (sessions, layouts) = next.list_tabs().await.unwrap();
    let name = |id: u32| sessions.iter().find(|s| s.id == id).unwrap().name.clone();
    let names: Vec<String> = sessions.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names, ["left", "right", "alone"]);
    assert_eq!(layouts.len(), 1);
    let panes: Vec<String> = layouts[0].sessions().into_iter().map(name).collect();
    assert_eq!(panes, ["left", "right"]);
    for s in sessions {
        next.kill(s.id).await.unwrap();
    }
    for s in client.list().await.unwrap() {
        let _ = client.kill(s.id).await;
    }
}

// ---- M2: project decisions (ADR-0007) ----

use valkyrie_proto::{DecisionKind, DecisionStatus, NewDecision, ReviewAction};

/// Waits for a `Proposals` push satisfying `pred`, skipping other pushes.
async fn wait_proposals(pushes: &mut Pushes, pred: impl Fn(&[u32]) -> bool) -> Vec<u32> {
    let mut last = Vec::new();
    for _ in 0..50 {
        match tokio::time::timeout(Duration::from_secs(3), pushes.recv()).await {
            Ok(Some(ServerMsg::Proposals { items })) => {
                let ids: Vec<u32> = items.iter().map(|d| d.id).collect();
                if pred(&ids) {
                    return ids;
                }
                last = ids;
            }
            Ok(Some(_)) => {}
            Ok(None) => panic!("connection closed"),
            Err(_) => break,
        }
    }
    panic!("proposals never matched; last: {last:?}");
}

#[tokio::test]
async fn decisions_are_proposed_pushed_and_reviewed() {
    let (client, mut pushes, dir) = start().await;
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("src")).unwrap();
    let repo = std::fs::canonicalize(repo).unwrap();
    client.watch_queue().await.unwrap();
    wait_proposals(&mut pushes, |p| p.is_empty()).await;

    let new = |title: &str, propose| NewDecision {
        cwd: repo.join("src"),
        title: title.into(),
        body: "why".into(),
        kind: DecisionKind::Constraint,
        propose,
        supersedes: None,
        commit: Some("abc1234".into()),
        session: None,
    };
    // From outside every session: a human, so it's active at once.
    let direct = client.decide(new("Direct", false)).await.unwrap();
    assert_eq!(direct.status, DecisionStatus::Active);
    assert_eq!(direct.project, repo);
    assert_eq!(direct.provenance.by, "human");
    let asked = client.decide(new("Asked", true)).await.unwrap();
    assert_eq!(asked.status, DecisionStatus::Proposed);
    wait_proposals(&mut pushes, |p| p == [asked.id]).await;

    let accepted = client
        .review(repo.clone(), asked.id, ReviewAction::Accept)
        .await
        .unwrap();
    assert_eq!(accepted.status, DecisionStatus::Active);
    wait_proposals(&mut pushes, |p| p.is_empty()).await;
    assert!(
        client
            .review(repo.clone(), asked.id, ReviewAction::Accept)
            .await
            .is_err()
    );

    let listed = client.decisions(Some(repo.join("src"))).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(client.decisions(None).await.unwrap().len(), 2);
    // The files are where the store says, for the hook to read.
    let store = valkyrie_context::Store::new(dir.join("state/context"));
    assert_eq!(store.load(&repo).len(), 2);
}

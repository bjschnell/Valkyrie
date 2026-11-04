//! ADR-0013 §5 with the real binary: an accepted `ship` policy keeps Claude's turn
//! going while the work is short of where the user ships it, once per turn, and is
//! silent otherwise.
#![cfg(unix)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use valkyrie_proto::{DecisionKind, NewDecision, Provenance};

const BIN: &str = env!("CARGO_BIN_EXE_valk");

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

/// What `valk hook claude` prints for a Stop in `repo`, with the store at `store`.
/// No daemon is running: the block can't wait on one.
fn stop(dir: &Path, repo: &Path, active: bool) -> String {
    stop_with(dir, repo, active, None)
}

/// `stop`, with `gh` taken from `bin` first when given.
fn stop_with(dir: &Path, repo: &Path, active: bool, bin: Option<&Path>) -> String {
    run_hook(dir, repo, active, bin, &dir.join("run/none.sock"))
}

/// `stop_with`, with a daemon stand-in on the socket: what the hook printed, and
/// the payload it sent.
fn stop_heard(
    dir: &Path,
    repo: &Path,
    active: bool,
    bin: Option<&Path>,
) -> (String, serde_json::Value) {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    let run = dir.join("run");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = run.join("d.sock");
    let _ = std::fs::remove_file(&socket);
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let daemon = std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut frame = Vec::new();
        conn.read_to_end(&mut frame).unwrap();
        let msg: serde_json::Value = serde_json::from_slice(&frame[4..]).unwrap();
        assert_eq!(msg["t"], "hook", "{msg}");
        msg["payload"].clone()
    });
    let out = run_hook(dir, repo, active, bin, &socket);
    (out, daemon.join().unwrap())
}

fn run_hook(dir: &Path, repo: &Path, active: bool, bin: Option<&Path>, socket: &Path) -> String {
    let mut path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(bin) = bin {
        let dirs = std::iter::once(bin.to_path_buf()).chain(std::env::split_paths(&path));
        path = std::env::join_paths(dirs.collect::<Vec<_>>()).unwrap();
    }
    let mut child = Command::new(BIN)
        .args(["hook", "claude"])
        .env("VALK_SESSION", "1")
        .env("VALK_SOCKET", socket)
        .env("VALK_CONTEXT", dir.join("context"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("PATH", path)
        .env_remove("VALK_SESSION_HOOKS")
        .env_remove("VALK_HOOK_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let payload = serde_json::json!({"hook_event_name": "Stop", "cwd": repo,
        "stop_hook_active": active, "last_assistant_message": "Done."});
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "the hook always exits 0");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn a_ship_policy_keeps_the_turn_going_until_work_is_where_the_user_ships_it() {
    let dir = Dir(std::env::temp_dir().join(format!("valkyrie-policy-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    let repo = dir.0.join("app");
    std::fs::create_dir_all(&repo).unwrap();
    git(
        &dir.0,
        &["init", "--quiet", "--bare", "-b", "main", "origin.git"],
    );
    git(&repo, &["init", "--quiet", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "init"]);
    git(&repo, &["remote", "add", "origin", "../origin.git"]);
    git(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    git(&repo, &["checkout", "--quiet", "-b", "fix"]);
    std::fs::write(repo.join("a.txt"), "fixed").unwrap();

    // No policy: the hook says nothing, whatever the work.
    assert_eq!(stop(&dir.0, &repo, false), "");

    // The user's policy: commit and push.
    let store = valkyrie_context::Store::new(dir.0.join("context"));
    let new = NewDecision {
        cwd: repo.clone(),
        title: "Ship finished work by pushing its branch".into(),
        body: "template: ship\nuntil: push\nchecks: true".into(),
        kind: DecisionKind::Policy,
        propose: false,
        supersedes: None,
        review_every: None,
        session: None,
    };
    let human = Provenance {
        by: "human".into(),
        ..Default::default()
    };
    let id = store.decide(&new, human.clone(), 1).unwrap().id;

    // Uncommitted: blocked, with the template's text and the accepted checks.
    let out = stop(&dir.0, &repo, false);
    let block: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(block["decision"], "block", "{out}");
    let reason = block["reason"].as_str().unwrap();
    assert!(
        reason.starts_with(&format!("Valkyrie policy #{id}, accepted")),
        "{reason}"
    );
    assert!(
        reason.contains("`fix` has changes that aren't committed"),
        "{reason}"
    );
    assert!(
        reason.contains("Run `true`.") && reason.contains("commit and push"),
        "{reason}"
    );

    // Once per turn: the stop after a block goes through.
    assert_eq!(stop(&dir.0, &repo, true), "");

    // Committed but not pushed: still short of a push.
    git(&repo, &["commit", "--quiet", "-am", "fix"]);
    assert!(stop(&dir.0, &repo, false).contains("aren't pushed"));

    // Pushed: where this user ships, so the turn ends.
    git(&repo, &["push", "--quiet", "-u", "origin", "fix"]);
    assert_eq!(stop(&dir.0, &repo, false), "");

    // The default branch is never blocked, even with changes on it.
    git(&repo, &["checkout", "--quiet", "main"]);
    std::fs::write(repo.join("a.txt"), "on main").unwrap();
    assert_eq!(stop(&dir.0, &repo, false), "");

    // A body that isn't a policy is refused when it's recorded.
    let bad = NewDecision {
        body: "ship everything always".into(),
        ..new
    };
    assert!(store.decide(&bad, human, 2).is_err());
}

#[test]
fn the_users_habit_ships_in_a_repository_with_no_policy_of_its_own() {
    let dir = Dir(std::env::temp_dir().join(format!("valkyrie-habit-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    let repo = dir.0.join("app");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "--quiet", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "init"]);
    git(&repo, &["checkout", "--quiet", "-b", "fix"]);
    std::fs::write(repo.join("a.txt"), "fixed").unwrap();

    let store = valkyrie_context::Store::new(dir.0.join("context"));
    let human = Provenance {
        by: "human".into(),
        ..Default::default()
    };
    let policy = |cwd: PathBuf, body: &str| NewDecision {
        cwd,
        title: "Ship".into(),
        body: body.into(),
        kind: DecisionKind::Policy,
        propose: false,
        supersedes: None,
        review_every: None,
        session: None,
    };
    // The habit, recorded once for every project.
    let user = valkyrie_context::project::user();
    let habit = store
        .decide(
            &policy(user, "template: ship\nuntil: commit"),
            human.clone(),
            1,
        )
        .unwrap()
        .id;
    let block = |out: String| -> String {
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["decision"], "block", "{out}");
        v["reason"].as_str().unwrap().to_owned()
    };
    // A repository with nothing of its own: the habit alone, with whatever checks
    // the project uses.
    let reason = block(stop(&dir.0, &repo, false));
    assert!(
        reason.starts_with(&format!("Valkyrie policy your habit #{habit}, accepted")),
        "{reason}"
    );
    assert!(reason.contains("Run the project's checks"), "{reason}");

    // The repository's facts lie over it.
    let facts = store
        .decide(
            &policy(repo.clone(), "template: ship\nchecks: true\nci: gh"),
            human,
            2,
        )
        .unwrap()
        .id;
    let reason = block(stop(&dir.0, &repo, false));
    assert!(
        reason.starts_with(&format!(
            "Valkyrie policy your habit #{habit} with #{facts}, accepted"
        )),
        "{reason}"
    );
    assert!(reason.contains("Run `true`."), "{reason}");

    // Committed: as far as this user ships.
    git(&repo, &["commit", "--quiet", "-am", "fix"]);
    assert_eq!(stop(&dir.0, &repo, false), "");
}

#[test]
fn ci_fix_keeps_the_agent_on_a_red_pull_request_twice_then_lets_it_stop() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Dir(std::env::temp_dir().join(format!("valkyrie-ci-fix-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&dir.0);
    let repo = dir.0.join("app");
    std::fs::create_dir_all(&repo).unwrap();
    git(
        &dir.0,
        &["init", "--quiet", "--bare", "-b", "main", "origin.git"],
    );
    git(&repo, &["init", "--quiet", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "init"]);
    git(&repo, &["remote", "add", "origin", "../origin.git"]);
    git(&repo, &["push", "--quiet", "-u", "origin", "main"]);
    git(&repo, &["checkout", "--quiet", "-b", "fix"]);
    std::fs::write(repo.join("a.txt"), "fixed").unwrap();
    git(&repo, &["commit", "--quiet", "-am", "fix"]);
    git(&repo, &["push", "--quiet", "-u", "origin", "fix"]);

    // A `gh` that answers with whatever the test last wrote.
    let bin = dir.0.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let answer = dir.0.join("gh.json");
    let gh = bin.join("gh");
    std::fs::write(&gh, format!("#!/bin/sh\ncat '{}'\n", answer.display())).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let pr = "https://github.com/o/r/pull/93";
    let red = serde_json::json!({"state": "OPEN", "url": pr, "statusCheckRollup": [
        {"name": "test", "workflowName": "CI", "conclusion": "FAILURE",
         "detailsUrl": "https://github.com/o/r/actions/runs/555/job/1"},
        {"context": "bamboo/PLAN", "state": "FAILURE",
         "targetUrl": "https://bamboo.example.com/browse/PLAN-7"}]});
    std::fs::write(&answer, red.to_string()).unwrap();
    let stop = |active| stop_with(&dir.0, &repo, active, Some(&bin));

    // No `ci` accepted: a red PR is the user's, as before.
    assert_eq!(stop(false), "");

    let store = valkyrie_context::Store::new(dir.0.join("context"));
    let human = Provenance {
        by: "human".into(),
        ..Default::default()
    };
    // The repository's facts alone, with no habit saying how far to ship.
    let facts = store
        .decide(
            &NewDecision {
                cwd: repo.clone(),
                title: "Checks to run before shipping".into(),
                body: "template: ship\nchecks: make check\nci: gh".into(),
                kind: DecisionKind::Policy,
                propose: false,
                supersedes: None,
                review_every: None,
                session: None,
            },
            human,
            1,
        )
        .unwrap()
        .id;
    let block = |out: String| -> String {
        let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(v["decision"], "block", "{out}");
        v["reason"].as_str().unwrap().to_owned()
    };
    let reason = block(stop(false));
    assert!(
        reason.starts_with(&format!("Valkyrie policy #{facts}, accepted")),
        "{reason}"
    );
    assert!(reason.contains("PR #93 (branch `fix`)"), "{reason}");
    assert!(
        reason.contains("`CI / test`: `gh run view 555 --log-failed`"),
        "{reason}"
    );
    assert!(
        reason.contains("`bamboo/PLAN`: https://bamboo.example.com/browse/PLAN-7"),
        "{reason}"
    );
    assert!(reason.contains("run `make check`, push"), "{reason}");
    assert!(reason.contains("block 1 of 2"), "{reason}");
    // Once per turn.
    assert_eq!(stop(true), "");
    // Green, or still running: nothing to fix.
    let green = serde_json::json!({"state": "OPEN", "url": pr, "statusCheckRollup": [
        {"name": "test", "conclusion": "SUCCESS"}, {"name": "lint", "status": "IN_PROGRESS"}]});
    std::fs::write(&answer, green.to_string()).unwrap();
    assert_eq!(stop(false), "");
    // Red again: the second and last block, then the user's.
    std::fs::write(&answer, red.to_string()).unwrap();
    assert!(block(stop(false)).contains("block 2 of 2"));
    // The stop after the last block, still red: through, and marked for the
    // queue as the user's (ADR-0013 §3).
    let (out, payload) = stop_heard(&dir.0, &repo, true, Some(&bin));
    assert_eq!(out, "");
    assert_eq!(payload["valk_ci_fix_spent"]["pr"], pr, "{payload}");
    assert!(payload["valk_continued"].is_null());
    // And every red stop after it.
    let (out, payload) = stop_heard(&dir.0, &repo, false, Some(&bin));
    assert_eq!(out, "");
    assert_eq!(payload["valk_ci_fix_spent"]["blocks"], 2, "{payload}");
    // Green again: an ordinary turn end.
    std::fs::write(&answer, green.to_string()).unwrap();
    let (_, payload) = stop_heard(&dir.0, &repo, true, Some(&bin));
    assert!(payload["valk_ci_fix_spent"].is_null(), "{payload}");
    std::fs::write(&answer, red.to_string()).unwrap();
    // Another pull request starts its own count.
    let other = red.to_string().replace("/pull/93", "/pull/94");
    std::fs::write(&answer, other).unwrap();
    assert!(block(stop(false)).contains("PR #94"));
    // The stop after a first block on it isn't marked: tries are left.
    let (_, payload) = stop_heard(&dir.0, &repo, true, Some(&bin));
    assert!(payload["valk_ci_fix_spent"].is_null(), "{payload}");
}

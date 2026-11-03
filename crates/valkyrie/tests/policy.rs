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
    let mut child = Command::new(BIN)
        .args(["hook", "claude"])
        .env("VALK_SESSION", "1")
        .env("VALK_SOCKET", dir.join("run/none.sock"))
        .env("VALK_CONTEXT", dir.join("context"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
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

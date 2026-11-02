//! Where an agent left its work when its turn ended (ADR-0013 §1): read from git
//! (and `gh` for the pull request) in the directory the agent ran in, so a stop
//! that leaves work short of shipping is seen whatever the agent's reply says.

use crate::worktree::{default_branch, git_out};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use valkyrie_proto::NoWindow;

/// Longest a `gh` lookup may take; the answer is only for the record, so a slow
/// network loses it rather than holding anything up.
const GH_TIMEOUT: Duration = Duration::from_secs(10);

/// How far a stop's work got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Nothing left to ship: the default branch, a detached head, a branch with no
    /// work of its own, or one whose pull request is open and not failing (or
    /// merged, or closed).
    Clean,
    /// A branch with changes not committed.
    Uncommitted,
    /// Committed, not on its upstream (or, with none, not on the default branch).
    Unpushed,
    /// Pushed, with no pull request.
    NoPr,
    /// Its pull request's checks failed.
    PrRed,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Clean => "clean",
            State::Uncommitted => "uncommitted",
            State::Unpushed => "unpushed",
            State::NoPr => "no-pr",
            State::PrRed => "pr-red",
        }
    }
}

/// A branch's pull request, as far as `gh` could tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pr {
    /// `gh` says there's none.
    None,
    Open {
        url: String,
        red: bool,
    },
    /// Merged or closed.
    Done {
        url: String,
    },
    /// No `gh`, not signed in, not a GitHub remote, offline, too slow.
    Unknown,
}

/// The work at a stop in `cwd`. `None` outside a git repository.
pub fn at(cwd: &Path) -> Option<Value> {
    let (state, branch, pr) = state(cwd, &gh_pr)?;
    Some(json!({"state": state.as_str(), "branch": branch, "pr": pr}))
}

/// The state, the branch (when there is one) and the pull request's URL (when
/// one was found), with `pr` looking the pull request up for a branch.
pub fn state(
    cwd: &Path,
    pr: &dyn Fn(&Path, &str) -> Pr,
) -> Option<(State, Option<String>, Option<String>)> {
    git_out(cwd, &["rev-parse", "--is-inside-work-tree"]).ok()?;
    let Ok(branch) = git_out(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"]) else {
        return Some((State::Clean, None, None));
    };
    let default = default_branch(cwd);
    if default.as_deref() == Some(branch.as_str()) {
        return Some((State::Clean, Some(branch), None));
    }
    let dirty = git_out(cwd, &["status", "--porcelain"]).is_ok_and(|s| !s.is_empty());
    if dirty {
        return Some((State::Uncommitted, Some(branch), None));
    }
    let ahead = |range: &str| {
        git_out(cwd, &["rev-list", "--count", range])
            .ok()
            .and_then(|n| n.parse::<u32>().ok())
    };
    let upstream = git_out(
        cwd,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    );
    if upstream.is_err() {
        // Never pushed: work of its own is commits the default branch hasn't got.
        let base = default.and_then(|d| {
            [format!("origin/{d}"), d]
                .into_iter()
                .find(|r| git_out(cwd, &["rev-parse", "--verify", "--quiet", r.as_str()]).is_ok())
        });
        let work = base.and_then(|b| ahead(&format!("{b}..HEAD")));
        let state = if work.unwrap_or(0) > 0 {
            State::Unpushed
        } else {
            State::Clean
        };
        return Some((state, Some(branch), None));
    }
    if ahead("@{upstream}..HEAD").unwrap_or(0) > 0 {
        return Some((State::Unpushed, Some(branch), None));
    }
    let (state, url) = match pr(cwd, &branch) {
        Pr::None => (State::NoPr, None),
        Pr::Open { url, red: true } => (State::PrRed, Some(url)),
        Pr::Open { url, red: false } | Pr::Done { url } => (State::Clean, Some(url)),
        Pr::Unknown => (State::Clean, None),
    };
    Some((state, Some(branch), url))
}

/// Asks `gh` about `branch`'s pull request.
fn gh_pr(cwd: &Path, branch: &str) -> Pr {
    let child = Command::new("gh")
        .no_window()
        .args([
            "pr",
            "view",
            branch,
            "--json",
            "state,url,statusCheckRollup",
        ])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        return Pr::Unknown;
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < GH_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Pr::Unknown;
            }
        }
    }
    let Ok(out) = child.wait_with_output() else {
        return Pr::Unknown;
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return if err.contains("no pull requests found") {
            Pr::None
        } else {
            Pr::Unknown
        };
    }
    serde_json::from_slice::<Value>(&out.stdout).map_or(Pr::Unknown, |v| parse_pr(&v))
}

/// `gh pr view --json state,url,statusCheckRollup`, read.
fn parse_pr(v: &Value) -> Pr {
    let url = v["url"].as_str().unwrap_or("").to_owned();
    if v["state"] != "OPEN" {
        return Pr::Done { url };
    }
    // A check run has a conclusion once it's done; a commit status, a state.
    const FAILED: &[&str] = &[
        "FAILURE",
        "ERROR",
        "TIMED_OUT",
        "CANCELLED",
        "ACTION_REQUIRED",
        "STARTUP_FAILURE",
    ];
    let red = v["statusCheckRollup"].as_array().is_some_and(|checks| {
        checks.iter().any(|c| {
            ["conclusion", "state"]
                .iter()
                .filter_map(|k| c[*k].as_str())
                .any(|s| FAILED.contains(&s))
        })
    });
    Pr::Open { url, red }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A repository on `main` with one commit, and a bare `origin` beside it.
    struct Repo(PathBuf);

    impl Repo {
        fn new(tag: &str) -> Repo {
            let base =
                std::env::temp_dir().join(format!("valkyrie-work-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let app = base.join("app");
            std::fs::create_dir_all(&app).unwrap();
            git(
                &base,
                &["init", "--quiet", "--bare", "-b", "main", "origin.git"],
            );
            git(&app, &["init", "--quiet", "-b", "main"]);
            std::fs::write(app.join("a.txt"), "a").unwrap();
            git(&app, &["add", "."]);
            commit(&app, "init");
            git(&app, &["remote", "add", "origin", "../origin.git"]);
            git(&app, &["push", "--quiet", "-u", "origin", "main"]);
            Repo(base)
        }

        fn app(&self) -> PathBuf {
            self.0.join("app")
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn commit(dir: &Path, message: &str) {
        let args = [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-am",
        ];
        git(dir, &[&args[..], &[message]].concat());
    }

    fn of(dir: &Path, pr: Pr) -> State {
        state(dir, &|_, _| pr.clone()).unwrap().0
    }

    #[test]
    fn follows_work_from_edit_to_pull_request() {
        let repo = Repo::new("flow");
        let app = repo.app();
        // A fresh branch with nothing of its own.
        git(&app, &["checkout", "--quiet", "-b", "fix"]);
        assert_eq!(of(&app, Pr::None), State::Clean);
        // The default branch is never work to ship, even with changes on it.
        git(&app, &["checkout", "--quiet", "main"]);
        std::fs::write(app.join("a.txt"), "changed").unwrap();
        assert_eq!(of(&app, Pr::None), State::Clean);
        git(&app, &["checkout", "--quiet", "fix"]);
        assert_eq!(of(&app, Pr::None), State::Uncommitted);
        commit(&app, "fix");
        assert_eq!(of(&app, Pr::None), State::Unpushed);
        git(&app, &["push", "--quiet", "-u", "origin", "fix"]);
        assert_eq!(of(&app, Pr::None), State::NoPr);
        let url = "https://example.com/pr/1".to_owned();
        let open = |red| Pr::Open {
            url: url.clone(),
            red,
        };
        assert_eq!(of(&app, open(true)), State::PrRed);
        assert_eq!(of(&app, open(false)), State::Clean);
        assert_eq!(of(&app, Pr::Done { url: url.clone() }), State::Clean);
        // Without `gh` nothing is known past the push.
        assert_eq!(of(&app, Pr::Unknown), State::Clean);
        // More commits after the push are unpushed again.
        std::fs::write(app.join("a.txt"), "again").unwrap();
        commit(&app, "more");
        assert_eq!(of(&app, open(false)), State::Unpushed);
        let (_, branch, _) = state(&app, &|_, _| Pr::None).unwrap();
        assert_eq!(branch.as_deref(), Some("fix"));
    }

    #[test]
    fn outside_a_repository_there_is_nothing_to_say() {
        let dir = std::env::temp_dir().join(format!("valkyrie-work-{}-none", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A stray .git above the temp dir would make this a repository (decision #10).
        if git_out(&dir, &["rev-parse", "--is-inside-work-tree"]).is_err() {
            assert!(state(&dir, &|_, _| Pr::None).is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_failed_checks_from_gh() {
        let pr = |state: &str, checks: Value| {
            parse_pr(&json!({"state": state, "url": "u", "statusCheckRollup": checks}))
        };
        let red = |p: Pr| matches!(p, Pr::Open { red: true, .. });
        assert!(red(pr(
            "OPEN",
            json!([{"conclusion": "SUCCESS"}, {"conclusion": "FAILURE"}])
        )));
        assert!(red(pr("OPEN", json!([{"state": "ERROR"}]))));
        assert!(!red(pr(
            "OPEN",
            json!([{"status": "IN_PROGRESS", "conclusion": ""}])
        )));
        assert!(!red(pr("OPEN", json!([]))));
        assert_eq!(
            pr("MERGED", json!([{"conclusion": "FAILURE"}])),
            Pr::Done { url: "u".into() }
        );
    }
}

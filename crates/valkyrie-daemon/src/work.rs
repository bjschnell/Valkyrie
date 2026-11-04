//! Where an agent left its work when its turn ended (ADR-0013 §1): read from git
//! (and `gh` for the pull request) in the directory the agent ran in, so a stop
//! that leaves work short of shipping is seen whatever the agent's reply says.

use crate::worktree::{default_branch, git_out};
use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
pub use valkyrie_context::policy::Check;
use valkyrie_proto::NoWindow;

/// Longest a `gh` lookup may take; the answer is only for the record, so a slow
/// network loses it rather than holding anything up.
pub const GH_TIMEOUT: Duration = Duration::from_secs(10);

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
        /// Its failed checks; none: green or still running.
        failed: Vec<Check>,
    },
    Merged {
        url: String,
    },
    /// Closed without merging.
    Closed {
        url: String,
    },
    /// No `gh`, not signed in, not a GitHub remote, offline, too slow.
    Unknown,
}

impl Pr {
    /// An open pull request with failed checks.
    pub fn red(&self) -> bool {
        matches!(self, Pr::Open { failed, .. } if !failed.is_empty())
    }
}

/// Looks a pull request up from a directory (see `gh_pr`); may block.
pub type PrLookup = std::sync::Arc<dyn Fn(&Path, &str) -> Pr + Send + Sync>;

/// The default branch of the repository `dir` is in (see `default_branch`).
pub fn default_branch_at(dir: &Path) -> Option<String> {
    default_branch(dir)
}

/// The branch checked out in `dir`; `None` on a detached head or outside git.
pub fn current_branch(dir: &Path) -> Option<String> {
    git_out(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()
}

/// The work at a stop in `cwd`. `None` outside a git repository.
pub fn at(cwd: &Path) -> Option<Value> {
    let (state, branch, pr) = state(cwd, &|dir, branch| gh_pr(dir, branch, GH_TIMEOUT))?;
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
        Pr::Open { url, failed } if !failed.is_empty() => (State::PrRed, Some(url)),
        Pr::Open { url, .. } | Pr::Merged { url } | Pr::Closed { url } => (State::Clean, Some(url)),
        Pr::Unknown => (State::Clean, None),
    };
    Some((state, Some(branch), url))
}

/// Asks `gh` about a pull request, giving up after `timeout`: `which` is a
/// branch of the repository `cwd` is in, or the pull request's URL.
pub fn gh_pr(cwd: &Path, which: &str, timeout: Duration) -> Pr {
    let child = Command::new("gh")
        .no_window()
        .args(["pr", "view", which, "--json", "state,url,statusCheckRollup"])
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
            Ok(None) if started.elapsed() < timeout => {
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
    match v["state"].as_str() {
        Some("OPEN") => {}
        Some("MERGED") => return Pr::Merged { url },
        Some("CLOSED") => return Pr::Closed { url },
        _ => return Pr::Unknown,
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
    let failed = v["statusCheckRollup"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| {
            ["conclusion", "state"]
                .iter()
                .filter_map(|k| c[*k].as_str())
                .any(|s| FAILED.contains(&s))
        })
        .map(|c| {
            let text = |keys: &[&str]| {
                keys.iter()
                    .find_map(|k| c[*k].as_str().filter(|s| !s.is_empty()))
                    .unwrap_or("")
                    .to_owned()
            };
            let url = text(&["detailsUrl", "targetUrl"]);
            Check {
                name: match (text(&["workflowName"]), text(&["name", "context"])) {
                    (w, n) if w.is_empty() || n.is_empty() => format!("{w}{n}"),
                    (w, n) => format!("{w} / {n}"),
                },
                run: actions_run(&url),
                url,
            }
        })
        .collect();
    Pr::Open { url, failed }
}

/// The run id in a GitHub Actions link (`…/actions/runs/<id>/job/<job>`), on
/// github.com or a GitHub Enterprise host.
fn actions_run(url: &str) -> Option<u64> {
    let (_, after) = url.split_once("/actions/runs/")?;
    after.split('/').next()?.parse().ok()
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
        let open = |red: bool| Pr::Open {
            url: url.clone(),
            failed: if red {
                vec![Check {
                    name: "ci".into(),
                    url: String::new(),
                    run: None,
                }]
            } else {
                vec![]
            },
        };
        assert_eq!(of(&app, open(true)), State::PrRed);
        assert_eq!(of(&app, open(false)), State::Clean);
        assert_eq!(of(&app, Pr::Merged { url: url.clone() }), State::Clean);
        assert_eq!(of(&app, Pr::Closed { url: url.clone() }), State::Clean);
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
        let red = |p: Pr| p.red();
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
            Pr::Merged { url: "u".into() }
        );
        assert_eq!(pr("CLOSED", json!([])), Pr::Closed { url: "u".into() });
        assert_eq!(pr("", json!([])), Pr::Unknown);
    }

    #[test]
    fn names_each_failed_check_with_the_link_to_its_logs() {
        let rollup = json!([
            {"__typename": "CheckRun", "name": "test (windows)", "workflowName": "CI",
             "conclusion": "FAILURE",
             "detailsUrl": "https://github.com/o/r/actions/runs/41005952796/job/12"},
            {"__typename": "CheckRun", "name": "fmt", "workflowName": "CI",
             "conclusion": "SUCCESS", "detailsUrl": "https://github.com/o/r/actions/runs/1/job/2"},
            // CI outside GitHub Actions reports a commit status (decision #24).
            {"__typename": "StatusContext", "context": "bamboo/PLAN-JOB", "state": "FAILURE",
             "targetUrl": "https://bamboo.example.com/browse/PLAN-JOB-7"},
            {"__typename": "StatusContext", "context": "lint", "state": "ERROR"},
            // GitHub Enterprise runs Actions too.
            {"__typename": "CheckRun", "name": "build", "conclusion": "TIMED_OUT",
             "detailsUrl": "https://git.corp.example/o/r/actions/runs/9/job/3"},
        ]);
        let Pr::Open { failed, .. } =
            parse_pr(&json!({"state": "OPEN", "url": "u", "statusCheckRollup": rollup}))
        else {
            panic!("open");
        };
        assert_eq!(
            failed,
            [
                Check {
                    name: "CI / test (windows)".into(),
                    url: "https://github.com/o/r/actions/runs/41005952796/job/12".into(),
                    run: Some(41005952796),
                },
                Check {
                    name: "bamboo/PLAN-JOB".into(),
                    url: "https://bamboo.example.com/browse/PLAN-JOB-7".into(),
                    run: None,
                },
                Check {
                    name: "lint".into(),
                    url: String::new(),
                    run: None,
                },
                Check {
                    name: "build".into(),
                    url: "https://git.corp.example/o/r/actions/runs/9/job/3".into(),
                    run: Some(9),
                },
            ]
        );
    }
}

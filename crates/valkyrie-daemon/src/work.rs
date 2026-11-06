//! Where an agent left its work when its turn ended (ADR-0013 §1): read from git
//! (and `gh` for the pull request) where the agent's work is, the directory it
//! ran in or its backlog item's worktree (`state_for`), so a stop that leaves work
//! short of shipping is seen whatever the agent's reply says.

use crate::worktree::{default_branch, git_out};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
pub use valkyrie_context::policy::Check;
use valkyrie_proto::{Item, NoWindow};

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
        /// The branch it targets, when GitHub says it conflicts with it. `None`
        /// when it merges cleanly or GitHub hasn't worked that out yet.
        conflicts: Option<String>,
        /// The commit it's at (`headRefOid`); empty when `gh` didn't say.
        head: String,
        /// Every check has finished and GitHub knows whether it merges: what the
        /// read says about red and conflicting is final for this head.
        settled: bool,
        /// Who opened it (`author.login`); empty when `gh` didn't say.
        author: String,
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

/// The commit checked out in `dir`; `None` outside git or before the first commit.
pub fn head_commit(dir: &Path) -> Option<String> {
    git_out(dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok()
}

/// The branch checked out in `dir`; `None` on a detached head or outside git.
pub fn current_branch(dir: &Path) -> Option<String> {
    git_out(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()
}

/// The work at a stop in `cwd` of a session on `item` (see `state_for`). `None`
/// outside a git repository.
pub fn at(cwd: &Path, item: Option<&Item>) -> Option<Value> {
    let (state, branch, pr, _) = state_for(cwd, item, &|dir, which| gh_pr(dir, which, GH_TIMEOUT))?;
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
        if work.unwrap_or(0) == 0 {
            return Some((State::Clean, Some(branch), None));
        }
        // Pushed once and its remote branch since deleted, as the forge does after
        // a merge: a squash merge leaves these commits off the default branch, so
        // its pull request says whether they're done with (#82).
        let tracked = git_out(cwd, &["config", &format!("branch.{branch}.merge")]).is_ok();
        if tracked && let Pr::Merged { url } | Pr::Closed { url } = pr(cwd, &branch) {
            return Some((State::Clean, Some(branch), Some(url)));
        }
        return Some((State::Unpushed, Some(branch), None));
    }
    if ahead("@{upstream}..HEAD").unwrap_or(0) > 0 {
        return Some((State::Unpushed, Some(branch), None));
    }
    let (state, url) = match pr(cwd, &branch) {
        Pr::None => (State::NoPr, None),
        Pr::Open { url, failed, .. } if !failed.is_empty() => (State::PrRed, Some(url)),
        Pr::Open { url, .. } | Pr::Merged { url } | Pr::Closed { url } => (State::Clean, Some(url)),
        Pr::Unknown => (State::Clean, None),
    };
    Some((state, Some(branch), url))
}

/// The work at a stop of a session on `item` (ADR-0013 §1), with the directory it
/// was read in. A session that took another item and works it in a worktree of
/// its own stops with its cwd where it began, so the item says where to look:
/// its launch's checkout, or the worktree its branch is checked out in, and its
/// pull request (by URL, which `gh` finds whatever the branch is called) when the
/// branch read is the item's. Its branch checked out nowhere, or no item: `cwd`.
pub fn state_for(
    cwd: &Path,
    item: Option<&Item>,
    pr: &dyn Fn(&Path, &str) -> Pr,
) -> Option<(State, Option<String>, Option<String>, PathBuf)> {
    let dir = dir(cwd, item);
    let lookup = |dir: &Path, branch: &str| match item {
        Some(Item {
            branch: Some(b),
            pr: Some(url),
            ..
        }) if b == branch => pr(dir, url),
        _ => pr(dir, branch),
    };
    let (state, branch, url) = state(&dir, &lookup)?;
    Some((state, branch, url, dir))
}

/// Where a session on `item` in `cwd` has its work (see `state_for`).
pub fn dir(cwd: &Path, item: Option<&Item>) -> PathBuf {
    item.and_then(|item| item_dir(cwd, item))
        .unwrap_or_else(|| cwd.to_path_buf())
}

/// Where `item`'s branch is checked out, when that's not `cwd`.
fn item_dir(cwd: &Path, item: &Item) -> Option<PathBuf> {
    let branch = item.branch.as_deref()?;
    if current_branch(cwd).as_deref() == Some(branch) {
        return None;
    }
    if let Some(checkout) = item.checkout.as_ref().filter(|c| c.is_dir())
        && current_branch(checkout).as_deref() == Some(branch)
    {
        return Some(checkout.clone());
    }
    let repo = item
        .project
        .as_deref()
        .filter(|p| p.is_dir())
        .unwrap_or(cwd);
    let listed = git_out(repo, &["worktree", "list", "--porcelain", "-z"]).ok()?;
    let want = format!("branch refs/heads/{branch}");
    listed.split("\0\0").find_map(|block| {
        let mut fields = block.split('\0');
        let path = fields.next()?.strip_prefix("worktree ")?;
        fields
            .any(|f| f == want)
            .then(|| PathBuf::from(path))
            .filter(|p| p.is_dir())
    })
}

/// Asks `gh` about a pull request, giving up after `timeout`: `which` is a
/// branch of the repository `cwd` is in, or the pull request's URL.
pub fn gh_pr(cwd: &Path, which: &str, timeout: Duration) -> Pr {
    let child = Command::new("gh")
        .no_window()
        .args([
            "pr",
            "view",
            which,
            "--json",
            "state,url,statusCheckRollup,mergeable,baseRefName,headRefOid,author",
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

/// `gh pr view --json state,url,statusCheckRollup,mergeable,baseRefName,headRefOid,author`,
/// read.
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
    let rollup: Vec<&Value> = v["statusCheckRollup"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let text = |c: &Value, keys: &[&str]| {
        keys.iter()
            .find_map(|k| c[*k].as_str().filter(|s| !s.is_empty()))
            .unwrap_or("")
            .to_owned()
    };
    let name = |c: &Value| match (text(c, &["workflowName"]), text(c, &["name", "context"])) {
        (w, n) if w.is_empty() || n.is_empty() => format!("{w}{n}"),
        (w, n) => format!("{w} / {n}"),
    };
    let is = |c: &Value, which: &[&str]| {
        ["conclusion", "state"]
            .iter()
            .filter_map(|k| c[*k].as_str())
            .any(|s| which.contains(&s))
    };
    // A cancel says nothing of the code, and a push and its pull request each run
    // CI on the same commit: one cancelled (a job that never got a runner, say)
    // while the other passed it isn't red (#83).
    let passed: Vec<String> = rollup
        .iter()
        .filter(|c| is(c, &["SUCCESS"]))
        .map(|c| name(c))
        .collect();
    let failed = rollup
        .iter()
        .filter(|c| is(c, FAILED))
        .filter(|c| !(is(c, &["CANCELLED"]) && passed.contains(&name(c))))
        .map(|c| {
            let url = text(c, &["detailsUrl", "targetUrl"]);
            Check {
                name: name(c),
                run: actions_run(&url),
                url,
            }
        })
        .collect();
    // A check run is done once it's COMPLETED; a commit status while PENDING or
    // EXPECTED isn't.
    let running = v["statusCheckRollup"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|c| {
            c["status"].as_str().is_some_and(|s| s != "COMPLETED")
                || matches!(c["state"].as_str(), Some("PENDING" | "EXPECTED"))
        });
    let settled = !running && matches!(v["mergeable"].as_str(), Some("MERGEABLE" | "CONFLICTING"));
    let conflicts = (v["mergeable"].as_str() == Some("CONFLICTING")).then(|| {
        v["baseRefName"]
            .as_str()
            .filter(|b| !b.is_empty())
            .unwrap_or("its base branch")
            .to_owned()
    });
    Pr::Open {
        url,
        failed,
        conflicts,
        head: v["headRefOid"].as_str().unwrap_or("").to_owned(),
        settled,
        author: v["author"]["login"].as_str().unwrap_or("").to_owned(),
    }
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
            conflicts: None,
            head: String::new(),
            settled: true,
            author: String::new(),
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

    /// A squash merge leaves the branch's commits off the default branch, and the
    /// forge deletes the branch: its upstream is gone, not never pushed (#82).
    #[test]
    fn a_squash_merged_branch_whose_remote_is_gone_is_clean() {
        let repo = Repo::new("squash");
        let app = repo.app();
        git(&app, &["checkout", "--quiet", "-b", "fix"]);
        std::fs::write(app.join("a.txt"), "fix").unwrap();
        commit(&app, "fix");
        git(&app, &["push", "--quiet", "-u", "origin", "fix"]);
        // The merge, done on the forge: one new commit on main, the branch deleted.
        git(&app, &["push", "--quiet", "origin", "--delete", "fix"]);
        git(&app, &["fetch", "--quiet", "--prune", "origin"]);
        let url = "https://example.com/pr/1".to_owned();
        let merged = Pr::Merged { url: url.clone() };
        let (got, branch, pr) = state(&app, &|_, _| merged.clone()).unwrap();
        assert_eq!(got, State::Clean);
        assert_eq!(
            (branch.as_deref(), pr.as_deref()),
            (Some("fix"), Some(url.as_str()))
        );
        assert_eq!(of(&app, Pr::Closed { url: url.clone() }), State::Clean);
        // No PR, or none that's done with: the commits are only here now.
        assert_eq!(of(&app, Pr::None), State::Unpushed);
        assert_eq!(of(&app, Pr::Unknown), State::Unpushed);
        // A branch never pushed isn't asked about, whatever a PR of its name says.
        git(&app, &["checkout", "--quiet", "-b", "fresh"]);
        assert_eq!(of(&app, merged), State::Unpushed);
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

    /// GitHub says whether an open PR merges into its base (backlog #69); only a
    /// sure conflict counts, since `UNKNOWN` means it hasn't worked that out yet.
    #[test]
    fn an_open_pr_says_when_it_conflicts_with_its_base() {
        let conflicts = |v: Value| match parse_pr(&v) {
            Pr::Open { conflicts, .. } => conflicts,
            other => panic!("{other:?}"),
        };
        let pr = |mergeable: &str, base: &str| json!({"state": "OPEN", "url": "u", "mergeable": mergeable, "baseRefName": base});
        assert_eq!(
            conflicts(pr("CONFLICTING", "main")).as_deref(),
            Some("main")
        );
        assert_eq!(
            conflicts(pr("CONFLICTING", "")).as_deref(),
            Some("its base branch")
        );
        assert_eq!(conflicts(pr("MERGEABLE", "main")), None);
        assert_eq!(conflicts(pr("UNKNOWN", "main")), None);
        // An older `gh`, or a reply without the field.
        assert_eq!(conflicts(json!({"state": "OPEN", "url": "u"})), None);
    }

    /// A check cancelled in one run and passed in another on the same commit isn't
    /// red; a failure is, whatever passed beside it (#83).
    #[test]
    fn a_check_cancelled_in_one_run_and_passed_in_another_isnt_red() {
        let failed = |checks: Value| match parse_pr(&json!({
            "state": "OPEN", "url": "u", "statusCheckRollup": checks
        })) {
            Pr::Open { failed, .. } => failed.into_iter().map(|c| c.name).collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        let run = |conclusion: &str| {
            json!({"__typename": "CheckRun", "workflowName": "test",
                   "name": "rust (macos-latest)", "status": "COMPLETED", "conclusion": conclusion})
        };
        assert!(failed(json!([run("CANCELLED"), run("SUCCESS")])).is_empty());
        assert!(failed(json!([run("SUCCESS"), run("CANCELLED")])).is_empty());
        // Cancelled and nothing passed it: still red.
        assert_eq!(
            failed(json!([run("CANCELLED")])),
            ["test / rust (macos-latest)"]
        );
        // A failure stays red, though the other run passed.
        assert_eq!(
            failed(json!([run("FAILURE"), run("SUCCESS")])),
            ["test / rust (macos-latest)"]
        );
        // Another check passing doesn't count.
        let lint = json!({"__typename": "CheckRun", "workflowName": "test",
                          "name": "lint", "status": "COMPLETED", "conclusion": "SUCCESS"});
        assert_eq!(
            failed(json!([run("CANCELLED"), lint])),
            ["test / rust (macos-latest)"]
        );
    }

    /// A read is settled once every check finished and GitHub knows whether it
    /// merges (ADR-0019 §3): only then does it judge a step a policy took.
    #[test]
    fn an_open_pr_is_settled_once_its_checks_ran_and_mergeability_is_known() {
        let settled = |mergeable: &str, checks: Value| match parse_pr(&json!({
            "state": "OPEN", "url": "u", "mergeable": mergeable, "statusCheckRollup": checks
        })) {
            Pr::Open { settled, .. } => settled,
            other => panic!("{other:?}"),
        };
        let done = json!([{"status": "COMPLETED", "conclusion": "SUCCESS"},
                          {"state": "SUCCESS", "context": "bamboo"}]);
        assert!(settled("MERGEABLE", done.clone()));
        assert!(settled("CONFLICTING", json!([])));
        assert!(!settled("UNKNOWN", done));
        assert!(!settled(
            "MERGEABLE",
            json!([{"status": "IN_PROGRESS", "conclusion": ""}])
        ));
        assert!(!settled(
            "MERGEABLE",
            json!([{"state": "PENDING", "context": "bamboo"}])
        ));
    }

    /// An item with `branch` and `pr`, taken: the rest as a take leaves it.
    fn taken(project: &Path, branch: &str, pr: &str) -> Item {
        serde_json::from_value(json!({
            "id": 65, "title": "t", "created": 0, "updated": 0, "taken": [1], "sessions": [1],
            "project": project, "branch": branch, "branch_taken": true, "pr": pr,
        }))
        .unwrap()
    }

    #[test]
    fn a_stop_reads_the_taken_items_worktree_and_pr_not_the_cwd() {
        // The session's cwd: its first item's worktree, whose PR merged (#68).
        let repo = Repo::new("taken");
        let app = repo.app();
        git(&app, &["checkout", "--quiet", "-b", "fix/38"]);
        std::fs::write(app.join("a.txt"), "38").unwrap();
        commit(&app, "38");
        git(&app, &["push", "--quiet", "-u", "origin", "fix/38"]);
        // The item it took next, worked in a second worktree, its PR red.
        let other = repo.0.join("app-65");
        git(
            &app,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "fix/65",
                other.to_str().unwrap(),
                "main",
            ],
        );
        std::fs::write(other.join("a.txt"), "65").unwrap();
        commit(&other, "65");
        git(&other, &["push", "--quiet", "-u", "origin", "fix/65"]);
        let (url38, url65) = ("https://example.com/pr/98", "https://example.com/pr/101");
        let asked = std::cell::RefCell::new(Vec::new());
        let lookup = |dir: &Path, which: &str| {
            asked.borrow_mut().push(which.to_owned());
            match which {
                w if w == "fix/38" || w == url38 => Pr::Merged { url: url38.into() },
                // Only the item's own PR is red; by branch `gh` may not find it
                // (a fork, a renamed branch), which is why the item's URL wins.
                w if w == url65 => {
                    assert_eq!(dir.canonicalize().unwrap(), other.canonicalize().unwrap());
                    Pr::Open {
                        url: url65.into(),
                        failed: vec![Check {
                            name: "ci".into(),
                            url: String::new(),
                            run: None,
                        }],
                        conflicts: None,
                        head: String::new(),
                        settled: true,
                        author: String::new(),
                    }
                }
                _ => Pr::None,
            }
        };
        let item = taken(&app, "fix/65", url65);
        let (state, branch, url, dir) = state_for(&app, Some(&item), &lookup).unwrap();
        assert_eq!(state, State::PrRed);
        assert_eq!(branch.as_deref(), Some("fix/65"));
        assert_eq!(url.as_deref(), Some(url65));
        assert_eq!(dir.canonicalize().unwrap(), other.canonicalize().unwrap());
        assert_eq!(*asked.borrow(), [url65]);
        // No item, or one on the cwd's own branch: the cwd, as before.
        let (state, ..) = state_for(&app, None, &lookup).unwrap();
        assert_eq!(state, State::Clean);
        let here = taken(&app, "fix/38", url38);
        let (state, branch, url, _) = state_for(&app, Some(&here), &lookup).unwrap();
        assert_eq!(
            (state, branch.as_deref(), url.as_deref()),
            (State::Clean, Some("fix/38"), Some(url38))
        );
        // Its branch checked out nowhere: nothing to read there, so the cwd.
        let gone = taken(&app, "fix/99", "https://example.com/pr/9");
        let (_, branch, ..) = state_for(&app, Some(&gone), &lookup).unwrap();
        assert_eq!(branch.as_deref(), Some("fix/38"));
    }
}

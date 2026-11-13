//! Past agent sessions, to resume one (backlog #150). `History` lists the agent
//! sessions of the last weeks that have ended, newest first, from their event logs
//! (the away digest's fold); `Resume` starts one again on its own conversation, in
//! its own directory, adding its worktree back if that was taken away.

use crate::digest::read_runs;
use crate::session::now_ms;
use crate::{Registry, spawn_with};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use valkyrie_context::digest::{self, Known, Run};
use valkyrie_proto::{PastSession, Reply, Size, SpawnSpec};

/// How far back the history reaches: as far as the digest does.
pub const REACH_MS: u64 = digest::LONGEST_MS;

/// How a past session comes back, or why it can't.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    pub command: Vec<String>,
    pub dir: PathBuf,
    /// The directory is gone: add this worktree back first.
    pub regrow: Option<Regrow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Regrow {
    /// Any checkout of the repository, to run git in.
    pub checkout: PathBuf,
    /// The worktree's own top folder, the one that went.
    pub path: PathBuf,
    pub branch: String,
}

/// What a run's resume takes: its agent on its conversation, where it started.
pub(crate) fn plan(run: &Run, roots: &[PathBuf]) -> Result<Plan, String> {
    if run.conversation.is_none() {
        return Err("its agent never named its conversation".into());
    }
    let command = command(run)?;
    let dir = run
        .home
        .clone()
        .ok_or("where it started wasn't written down")?;
    if dir.is_dir() {
        return Ok(Plan {
            command,
            dir,
            regrow: None,
        });
    }
    let Some(branch) = run.branch.clone() else {
        return Err(format!("{} is gone, on no known branch", dir.display()));
    };
    // The daemon wrote down the repository and the worktree: add back exactly
    // that one, and nothing for a folder that wasn't a linked worktree.
    let (path, checkout) = match (&run.repo, &run.worktree) {
        (Some(repo), Some(tree)) if dir.starts_with(tree) && !tree.exists() => {
            if !repo.is_dir() {
                return Err(format!(
                    "{} is gone, and so is {}",
                    dir.display(),
                    repo.display()
                ));
            }
            (tree.clone(), repo.clone())
        }
        (Some(_), _) => {
            return Err(format!("{} is gone, and wasn't a worktree", dir.display()));
        }
        // An older log: Valkyrie's `<repo>-<branch>` naming, checked by the branch.
        (None, _) => {
            let path = top_gone(&dir).ok_or_else(|| format!("{} is gone", dir.display()))?;
            let checkout = checkout_beside(&path, roots, &branch).ok_or_else(|| {
                format!(
                    "{} is gone, and no repository beside it has {branch}",
                    dir.display()
                )
            })?;
            (path, checkout)
        }
    };
    Ok(Plan {
        command,
        dir,
        regrow: Some(Regrow {
            checkout,
            path,
            branch,
        }),
    })
}

/// Its agent's resume command, by the rule search's hits use (#106): its own
/// options when it started as the agent, the bare agent when it ran in a shell.
fn command(run: &Run) -> Result<Vec<String>, String> {
    crate::digest::resume(run, &HashSet::new())
        .ok_or_else(|| "it ran one command, not a conversation to come back to".into())
}

/// The highest folder of `dir` that's gone: the worktree a removal took.
fn top_gone(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .take_while(|p| !p.exists())
        .last()
        .filter(|p| p.parent().is_some_and(Path::is_dir))
        .map(Path::to_path_buf)
}

/// A checkout of the repository a gone worktree `<repo>-<branch>` was beside:
/// a known root, else a folder beside it, named `<repo>`, in git, that has
/// `branch`. The longest name wins: `app-web` over `app` for `app-web-feat-x`.
fn checkout_beside(path: &Path, roots: &[PathBuf], branch: &str) -> Option<PathBuf> {
    let parent = path.parent()?;
    let gone = path.file_name()?.to_string_lossy().into_owned();
    let beside = std::fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path());
    roots
        .iter()
        .filter(|r| r.parent() == Some(parent))
        .cloned()
        .chain(beside)
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| gone.starts_with(&format!("{}-", n.to_string_lossy())))
        })
        .filter(|p| p.is_dir() && valkyrie_context::project::in_git(p))
        .filter(|p| crate::worktree::knows_branch(p, branch))
        .max_by_key(|p| p.as_os_str().len())
}

/// The past sessions among `runs`, newest first: agent sessions with something
/// in them, not running, one per conversation, none whose conversation is running.
pub(crate) fn past(
    runs: &[Run],
    known: &Known,
    running: &HashSet<String>,
    now: u64,
) -> Vec<PastSession> {
    let by_key: BTreeMap<&str, &Run> = runs.iter().map(|r| (r.key.as_str(), r)).collect();
    let mut out: Vec<(PastSession, Option<String>)> = digest::entries(runs, known, now)
        .into_iter()
        .filter(|e| e.session.is_none())
        .filter_map(|entry| {
            let run = by_key.get(entry.key.as_str())?;
            if run
                .conversation
                .as_ref()
                .is_some_and(|c| running.contains(c))
            {
                return None;
            }
            let mut entry = entry;
            // No hook said where it worked (it ended first): where it started.
            if entry.cwd.is_none() && run.home.is_some() {
                entry.cwd = run.home.clone();
                entry.repo = digest::repo_name(entry.cwd.as_deref(), known.roots);
            }
            let (cannot, regrow) = match plan(run, known.roots) {
                Ok(Plan {
                    regrow: Some(r), ..
                }) => {
                    // The checkout found beside the gone worktree names its repository.
                    if let Some(name) = r.checkout.file_name() {
                        entry.repo = name.to_string_lossy().into_owned();
                    }
                    (None, Some(r.branch))
                }
                Ok(_) => (None, None),
                Err(why) => (Some(why), None),
            };
            let past = PastSession {
                started_ms: run.started_ms,
                cannot,
                regrow,
                entry,
            };
            Some((past, run.conversation.clone()))
        })
        .collect();
    out.sort_by(|(a, _), (b, _)| {
        b.entry
            .last_ms
            .cmp(&a.entry.last_ms)
            .then(b.entry.key.cmp(&a.entry.key))
    });
    // A resumed conversation is one session, at its latest.
    let mut seen = HashSet::new();
    out.into_iter()
        .filter(|(_, c)| c.as_ref().is_none_or(|c| seen.insert(c.clone())))
        .map(|(p, _)| p)
        .collect()
}

/// A log key as the daemon names them, `<created ns>-<id>`: nothing that leaves
/// the sessions folder.
fn valid_key(key: &str) -> bool {
    key.split_once('-').is_some_and(|(ns, id)| {
        !ns.is_empty()
            && !id.is_empty()
            && ns.bytes().all(|b| b.is_ascii_digit())
            && id.bytes().all(|b| b.is_ascii_digit())
    })
}

/// Keys being resumed right now, across connections.
static RESUMING: std::sync::Mutex<Option<HashSet<String>>> = std::sync::Mutex::new(None);

struct Busy(String);

impl Busy {
    fn take(key: &str) -> Result<Busy> {
        let mut set = RESUMING.lock().unwrap();
        anyhow::ensure!(
            set.get_or_insert_with(HashSet::new).insert(key.to_owned()),
            "that session is being resumed already"
        );
        Ok(Busy(key.to_owned()))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        if let Some(set) = RESUMING.lock().unwrap().as_mut() {
            set.remove(&self.0);
        }
    }
}

impl Registry {
    /// The agent sessions of the last weeks that have ended, newest first.
    pub(crate) async fn history(&self) -> Result<Reply> {
        let now = now_ms();
        let around = self.around();
        let dir = self.host.transcript_dir.clone();
        let sessions = tokio::task::spawn_blocking(move || {
            let runs = read_runs(&dir, now.saturating_sub(REACH_MS), now, digest::read_run);
            past(&runs, &around.known(), &around.talking, now)
        })
        .await?;
        Ok(Reply::History { sessions })
    }

    /// Starts the past session `key` again, and says whether it started one. Its
    /// conversation still running in a session is answered with that session, so
    /// a second press doesn't fork it.
    pub(crate) async fn resume(
        &self,
        key: &str,
        size: Size,
        env: Vec<(String, Option<String>)>,
    ) -> Result<(Reply, bool)> {
        anyhow::ensure!(valid_key(key), "{key:?} is not a past session");
        // Two clients resuming one session at once would both find it not running.
        let _busy = Busy::take(key)?;
        let path = self.host.transcript_dir.join(format!("{key}.events.jsonl"));
        anyhow::ensure!(path.is_file(), "no past session {key}");
        let roots = self.around().roots;
        let (run, plan) = tokio::task::spawn_blocking(move || {
            let run = digest::read_run(&path, 0, now_ms())?;
            let plan = plan(&run, &roots);
            anyhow::Ok((run, plan))
        })
        .await??;
        if let Some(conversation) = &run.conversation
            && let Some(live) = self
                .all()
                .into_iter()
                .find(|s| !s.exited() && s.conversation().as_ref() == Some(conversation))
        {
            let info = Box::new(live.info());
            return Ok((Reply::Session { info }, false));
        }
        let plan = match plan {
            Ok(plan) => plan,
            Err(why) => bail!("can't resume it: {why}"),
        };
        if let Some(r) = &plan.regrow {
            let r = r.clone();
            tokio::task::spawn_blocking(move || {
                crate::worktree::regrow(&r.checkout, &r.path, &r.branch)
                    .with_context(|| format!("couldn't add back {}", r.path.display()))
            })
            .await??;
            anyhow::ensure!(
                plan.dir.is_dir(),
                "added back the worktree, but {} isn't in it",
                plan.dir.display()
            );
        }
        let spec = SpawnSpec {
            command: plan.command,
            cwd: Some(plan.dir),
            name: run.name.clone(),
            size,
            env,
            worktree: None,
            hidden: false,
        };
        let reply = spawn_with(self, spec, run.conversation.clone())?;
        if let Reply::Session { info } = &reply
            && let Ok(session) = self.get(info.id)
        {
            session.set_topic(run.topic.clone());
        }
        Ok((reply, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    fn run(agent: &str, command: &str, home: &Path) -> Run {
        Run {
            key: "1-1".into(),
            agent: Some(agent.into()),
            command: v(command),
            home: Some(home.to_path_buf()),
            conversation: Some("c1".into()),
            ..Run::default()
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("valkyrie-history-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_agent_resumes_with_its_options_or_bare_from_a_shell() {
        let dir = scratch("plan");
        let r = run("claude", "claude --model opus fix-it", &dir);
        assert_eq!(
            plan(&r, &[]).unwrap(),
            Plan {
                command: v("claude --resume c1 --model opus"),
                dir: dir.clone(),
                regrow: None,
            }
        );
        // Started a shell, ran codex in it.
        let r = run("codex", "fish", &dir);
        assert_eq!(plan(&r, &[]).unwrap().command, v("codex resume c1"));
        // One-shot and nameless runs can't come back.
        assert!(plan(&run("claude", "claude -p hi", &dir), &[]).is_err());
        let mut r = run("claude", "claude", &dir);
        r.conversation = None;
        assert!(plan(&r, &[]).unwrap_err().contains("conversation"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_gone_worktree_is_found_in_the_repository_that_has_its_branch() {
        let dir = scratch("gone");
        // `app` and `app-web` side by side; only `app-web` has `feat/x`.
        let repo = |name: &str, branch: Option<&str>| {
            let r = dir.join(name);
            std::fs::create_dir_all(&r).unwrap();
            git(&r, &["init", "--quiet", "-b", "main"]);
            git(&r, &["commit", "--quiet", "--allow-empty", "-m", "first"]);
            if let Some(b) = branch {
                git(&r, &["branch", b]);
            }
            r
        };
        repo("app", None);
        let web = repo("app-web", Some("feat/x"));
        let gone = dir.join("app-web-feat-x").join("sub");
        let mut r = run("claude", "claude", &gone);
        assert!(plan(&r, &[]).unwrap_err().contains("no known branch"));
        r.branch = Some("feat/x".into());
        assert_eq!(
            plan(&r, &[]).unwrap().regrow,
            Some(Regrow {
                checkout: web.clone(),
                path: dir.join("app-web-feat-x"),
                branch: "feat/x".into(),
            })
        );
        // No repository beside it has the branch.
        let other = Run {
            branch: Some("nope".into()),
            ..r.clone()
        };
        assert!(plan(&other, &[]).unwrap_err().contains("has nope"));
        // A newer log names its repository and worktree: those, whatever is beside.
        let recorded = Run {
            repo: Some(web.clone()),
            worktree: Some(dir.join("app-web-feat-x")),
            ..r.clone()
        };
        assert_eq!(plan(&recorded, &[]).unwrap().regrow.unwrap().checkout, web);
        // A gone folder that wasn't a linked worktree is left gone.
        let plain = Run {
            repo: Some(web.clone()),
            worktree: None,
            ..r
        };
        assert!(plan(&plain, &[]).unwrap_err().contains("wasn't a worktree"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A worktree deleted without git knowing is forgotten, only that one.
    #[test]
    fn regrow_forgets_only_its_own_missing_worktree() {
        let dir = scratch("forget");
        let repo = dir.join("app");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(
            &repo,
            &["commit", "--quiet", "--allow-empty", "-m", "first"],
        );
        for b in ["a", "b"] {
            let tree = dir.join(format!("app-{b}"));
            git(
                &repo,
                &[
                    "worktree",
                    "add",
                    "--quiet",
                    "-b",
                    b,
                    &tree.to_string_lossy(),
                ],
            );
            std::fs::remove_dir_all(&tree).unwrap();
        }
        // Through a link, as macOS's `/var` is to `/private/var`: git wrote the
        // path it resolved, not the one asked with.
        #[cfg(unix)]
        let at = {
            let link = dir.with_extension("link");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(&dir, &link).unwrap();
            link
        };
        #[cfg(not(unix))]
        let at = dir.clone();
        crate::worktree::regrow(&repo, &at.join("app-a"), "a").unwrap();
        assert!(dir.join("app-a/.git").exists());
        let list = std::process::Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let list = String::from_utf8_lossy(&list.stdout);
        assert!(
            list.contains("app-b"),
            "the other one is still listed: {list}"
        );
        #[cfg(unix)]
        let _ = std::fs::remove_file(&at);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(ok.status.success(), "git {args:?}: {ok:?}");
    }

    /// A past session whose worktree was taken away is listed, comes back in its
    /// worktree added back on its branch, and isn't started twice.
    #[cfg(unix)]
    #[tokio::test]
    async fn history_lists_an_ended_session_and_resume_brings_it_back_once() {
        use serde_json::json;
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("e2e");
        let state = dir.join("state");
        std::fs::create_dir_all(state.join("sessions")).unwrap();
        let (registry, _) = crate::new_registry(
            dir.join("v.sock"),
            &state,
            Path::new("/bin/true"),
            1,
            0,
            7,
            false,
        )
        .unwrap();
        // A repository, and its worktree on feat-x, since taken away.
        let repo = dir.join("app");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(
            &repo,
            &["commit", "--quiet", "--allow-empty", "-m", "first"],
        );
        let tree = dir.join("app-feat-x");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "feat-x",
                &tree.to_string_lossy(),
            ],
        );
        git(&repo, &["worktree", "remove", &tree.to_string_lossy()]);
        // A stand-in `claude` that waits.
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let claude = bin.join("claude");
        std::fs::write(&claude, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        let claude = claude.to_string_lossy().into_owned();

        let now = now_ms();
        let key = format!("{}-1", (now - 60_000) * 1_000_000);
        let log: String = [
            json!({"k": "start", "t": now - 60_000, "off": 0, "agent": "claude",
                "command": [claude, "--model", "opus", "fix it"], "cwd": tree, "name": "fixer"}),
            json!({"k": "conversation", "t": now - 59_000, "off": 0, "id": "conv-1"}),
            json!({"k": "hook", "t": now - 50_000, "off": 0, "agent": "claude", "ignored": false,
                "payload": {"cwd": tree, "hook_event_name": "UserPromptSubmit", "prompt": "fix it"}}),
            json!({"k": "work", "t": now - 40_000, "off": 0, "state": "unpushed", "branch": "feat-x"}),
            json!({"k": "exit", "t": now - 30_000, "off": 0, "code": 0}),
        ]
        .into_iter()
        .map(|v| format!("{v}\n"))
        .collect();
        std::fs::write(state.join(format!("sessions/{key}.events.jsonl")), log).unwrap();

        let Reply::History { sessions } = registry.history().await.unwrap() else {
            panic!("not a history");
        };
        assert_eq!(sessions.len(), 1, "{sessions:?}");
        let past = &sessions[0];
        assert_eq!(past.entry.key, key);
        assert_eq!(past.entry.what, "fixer");
        assert_eq!(past.cannot, None);
        assert_eq!(past.regrow.as_deref(), Some("feat-x"));
        assert_eq!(past.entry.repo, "app", "named by the checkout beside it");

        let size = Size { cols: 80, rows: 24 };
        let (Reply::Session { info }, true) =
            registry.resume(&key, size, Vec::new()).await.unwrap()
        else {
            panic!("not started");
        };
        assert!(tree.is_dir(), "the worktree is back");
        assert_eq!(
            info.command,
            vec![
                claude.clone(),
                "--resume".into(),
                "conv-1".into(),
                "--model".into(),
                "opus".into()
            ]
        );
        assert_eq!(info.cwd, tree);
        // Again: the running one, not a second.
        let (Reply::Session { info: again }, false) =
            registry.resume(&key, size, Vec::new()).await.unwrap()
        else {
            panic!("started twice");
        };
        assert_eq!(again.id, info.id);
        // Running now, so out of the history.
        let Reply::History { sessions } = registry.history().await.unwrap() else {
            panic!("not a history");
        };
        assert!(sessions.iter().all(|p| p.entry.key != key), "{sessions:?}");

        assert!(registry.resume("../x-1", size, Vec::new()).await.is_err());
        registry.get(info.id).unwrap().kill();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A session started in a worktree through a link (macOS's `/var`) writes
    /// down that worktree as its cwd spells it, so a resume finds the cwd in it.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_start_line_spells_the_worktree_as_the_cwd_does() {
        let dir = scratch("spell");
        let state = dir.join("state");
        std::fs::create_dir_all(state.join("sessions")).unwrap();
        let (registry, _) = crate::new_registry(
            dir.join("v.sock"),
            &state,
            Path::new("/bin/true"),
            1,
            0,
            7,
            false,
        )
        .unwrap();
        let repo = dir.join("app");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "--quiet", "-b", "main"]);
        git(
            &repo,
            &["commit", "--quiet", "--allow-empty", "-m", "first"],
        );
        let tree = dir.join("app-x");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "x",
                &tree.to_string_lossy(),
            ],
        );
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        let link = dir.with_extension("link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        let cwd = link.join("app-x").join("sub");
        let spec = SpawnSpec {
            command: vec!["sleep".into(), "30".into()],
            cwd: Some(cwd.clone()),
            name: None,
            size: Size { cols: 80, rows: 24 },
            env: Vec::new(),
            worktree: None,
            hidden: false,
        };
        let Reply::Session { info } = spawn_with(&registry, spec, None).unwrap() else {
            panic!("not spawned");
        };
        let session = registry.get(info.id).unwrap();
        let log = state.join(format!("sessions/{}.events.jsonl", session.key()));
        let run = digest::read_run(&log, 0, now_ms()).unwrap();
        session.kill();
        let home = run.home.expect("start cwd");
        let worktree = run.worktree.expect("a linked worktree");
        assert!(home.starts_with(&worktree), "{home:?} in {worktree:?}");
        assert_eq!(worktree.file_name().unwrap(), "app-x");
        assert!(run.repo.is_some());
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_stay_in_the_sessions_folder() {
        assert!(valid_key("1700000000000000000-4"));
        for bad in ["", "1-", "-1", "../x-1", "1-1/..", "1-a", "1"] {
            assert!(!valid_key(bad), "{bad}");
        }
    }
}

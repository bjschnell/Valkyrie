//! The supervisor's watch on pull requests no backlog item has (ADR-0019 §7,
//! backlog #80). The PR poll reads each item's recorded PR; this reads the rest:
//! a live agent session whose branch has an open PR the user opened, found the
//! way `valk todo` finds one, by asking `gh` about the branch. Sessions that
//! outlived a daemon restart are here too, since an item only follows sessions of
//! the daemon's current boot. What's flagged goes through the same steps as an
//! item's, recorded as found by branch so `valk decisions health` can say whether
//! this reaches too far.

use crate::Registry;
use crate::backlog::pr_summary;
use crate::supervise::Watched;
use crate::work::{self, Pr, PrLookup};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use valkyrie_context::project;
use valkyrie_proto::{ItemStatus, SessionId};

/// How long a branch `gh` found no PR for isn't asked about again while its
/// commit stays the same. A PR opened from the browser shows up within this.
const NO_PR_FOR: Duration = Duration::from_secs(30 * 60);

/// One session's place: what to ask `gh` from, and about.
struct Place {
    session: SessionId,
    cwd: PathBuf,
    root: PathBuf,
    branch: String,
    head: String,
}

/// The `gh` user's login, for `watch_branches`; `None` when `gh` can't say
/// (not installed, signed out, offline). Blocks.
pub(crate) fn me() -> Option<String> {
    crate::supervise::gh(
        &std::env::temp_dir(),
        &["api", "user", "--jq", ".login"],
        work::GH_TIMEOUT,
    )
    .ok()
    .map(|s| s.trim().to_owned())
    .filter(|s| !s.is_empty())
}

impl Registry {
    /// Flags idle agent sessions whose branch has an open pull request that `me`
    /// opened and that no backlog item's poll covers, and offers their steps.
    /// Someone else's PR (checked out to review it, say) is never flagged: a
    /// step types into the session. Returns how many sessions it flagged.
    pub(crate) async fn watch_branches(&self, lookup: PrLookup, me: &str) -> usize {
        if !self.supervisor.lock().unwrap().on() {
            return 0;
        }
        // The sessions `ship_merged_prs` reads a PR for already.
        let covered: HashSet<SessionId> = self
            .backlog
            .lock()
            .unwrap()
            .load()
            .into_iter()
            .filter(|i| matches!(i.status, ItemStatus::Open | ItemStatus::Proposed))
            .filter(|i| i.pr.is_some() && i.boot == self.boot)
            .flat_map(|i| i.sessions)
            .collect();
        let sessions: Vec<(SessionId, PathBuf)> = self
            .listed()
            .into_iter()
            .filter(|s| !s.exited() && s.agent_behind().is_some())
            .map(|s| (s.info().id, s.info().cwd))
            .filter(|(id, _)| !covered.contains(id))
            .collect();
        if sessions.is_empty() {
            return 0;
        }
        let places = tokio::task::spawn_blocking(move || {
            sessions
                .into_iter()
                .filter_map(|(session, cwd)| {
                    let branch = work::current_branch(&cwd)?;
                    // Work on the default branch has no PR of its own.
                    if work::default_branch_at(&cwd).as_deref() == Some(branch.as_str()) {
                        return None;
                    }
                    Some(Place {
                        session,
                        root: project::root(&cwd),
                        head: work::head_commit(&cwd).unwrap_or_default(),
                        cwd,
                        branch,
                    })
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        // One `gh` call per branch, however many sessions are on it.
        let mut by_branch: BTreeMap<(PathBuf, String), Vec<Place>> = BTreeMap::new();
        for place in places {
            by_branch
                .entry((place.root.clone(), place.branch.clone()))
                .or_default()
                .push(place);
        }
        let mut flagged = 0;
        for (key, places) in by_branch {
            let first = &places[0];
            let cached = self
                .supervisor
                .lock()
                .unwrap()
                .no_pr
                .get(&key)
                .is_some_and(|(head, at)| *head == first.head && at.elapsed() < NO_PR_FOR);
            if cached {
                continue;
            }
            let ask = lookup.clone();
            let (cwd, branch) = (first.cwd.clone(), key.1.clone());
            let pr = tokio::task::spawn_blocking(move || ask(&cwd, &branch))
                .await
                .unwrap_or(Pr::Unknown);
            let url = match &pr {
                Pr::None => {
                    self.supervisor
                        .lock()
                        .unwrap()
                        .no_pr
                        .insert(key, (first.head.clone(), Instant::now()));
                    continue;
                }
                Pr::Unknown => continue,
                Pr::Open { url, .. } | Pr::Merged { url, .. } | Pr::Closed { url } => url.clone(),
            };
            self.supervisor.lock().unwrap().no_pr.remove(&key);
            // How the steps policies took on it turned out.
            self.judge_steps(&url, &pr);
            if let Pr::Merged { base, author, .. } = &pr {
                // Only the user's own, seen open first: an old merge, or a
                // colleague's PR checked out to review, is no reason to type.
                let news = author == me && self.supervisor.lock().unwrap().open_seen.remove(&url);
                if news {
                    let own: Vec<SessionId> = places.iter().map(|p| p.session).collect();
                    self.announce_merge(&url, base, &own, &key.0, None);
                }
                continue;
            }
            if let Pr::Closed { .. } = &pr {
                self.supervisor.lock().unwrap().open_seen.remove(&url);
            }
            if let Pr::Open { author, .. } = &pr
                && author == me
            {
                self.supervisor
                    .lock()
                    .unwrap()
                    .open_seen
                    .insert(url.clone());
            }
            let Pr::Open {
                failed,
                conflicts,
                author,
                ..
            } = &pr
            else {
                continue;
            };
            if author != me {
                tracing::debug!(pr = url, author, "not flagging someone else's PR");
                continue;
            }
            let Some(summary) = pr_summary(&url, failed, conflicts.as_deref()) else {
                continue;
            };
            for place in &places {
                let Ok(session) = self.get(place.session) else {
                    continue;
                };
                let Some(seq) = session.flag_blocked(&summary) else {
                    continue;
                };
                tracing::info!(
                    session = place.session,
                    pr = url,
                    summary,
                    "flagged a PR found by branch"
                );
                flagged += 1;
                let work = Watched {
                    item: None,
                    dir: place.cwd.clone(),
                    branch: place.branch.clone(),
                    project: Some(place.root.clone()),
                };
                self.offer_step(&work, place.session, seq, &summary, &pr)
                    .await;
            }
        }
        flagged
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::context::{AS_CALLER, Caller};
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;
    use valkyrie_context::steps::Via;
    use valkyrie_proto::{ItemChange, NewItem, Reply, Size, SpawnSpec, StepAction};

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    /// A session on a branch with an open PR of the user's, and no item, is
    /// flagged and offered its step, recorded as found by branch. Sessions on the
    /// default branch, a human's, an item's covered by its own poll, and one on
    /// somebody else's PR aren't; a branch with no PR isn't asked again.
    #[tokio::test]
    async fn sessions_with_a_pr_and_no_item_are_flagged_by_branch() {
        let dir = std::env::temp_dir().join(format!("valkyrie-branches-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let wt = |branch: &str| {
            let path = dir.join(branch.replace('/', "-"));
            let path_s = path.to_str().unwrap().to_owned();
            git(&repo, &["worktree", "add", "-q", "-b", branch, &path_s]);
            path
        };
        let (mine, theirs, tracked, bare) = (
            wt("feat/mine"),
            wt("feat/theirs"),
            wt("feat/tracked"),
            wt("feat/bare"),
        );
        let (registry, _) = crate::new_registry(
            dir.join("v.sock"),
            &dir.join("state"),
            Path::new("/bin/true"),
            1,
            0,
            7,
            false,
        )
        .unwrap();
        let spawn = |cwd: &Path, agent: bool| {
            let spec = SpawnSpec {
                command: vec!["sleep".into(), "30".into()],
                cwd: Some(cwd.to_owned()),
                name: None,
                size: Size { cols: 40, rows: 10 },
                env: Vec::new(),
                worktree: None,
                hidden: false,
            };
            let Reply::Session { info } = crate::spawn(&registry, spec).unwrap() else {
                panic!("no session");
            };
            let s = registry.get(info.id).unwrap();
            if agent {
                s.driven_by("claude");
            }
            s
        };
        let a = spawn(&mine, true);
        let human = spawn(&mine, false);
        let on_main = spawn(&repo, true);
        let review = spawn(&theirs, true);
        let on_item = spawn(&tracked, true);
        let no_pr = spawn(&bare, true);
        // The item's own poll reads this one's PR.
        {
            let b = registry.backlog.lock().unwrap();
            let new = NewItem {
                title: "tracked".into(),
                cwd: Some(repo.clone()),
                ..Default::default()
            };
            let item = b.add(&new, "human", 1).unwrap();
            let open = ItemChange::Status {
                status: ItemStatus::Open,
            };
            b.change(item.id, &open, 1).unwrap();
            let url = "https://github.com/o/r/pull/3";
            b.taken(
                item.id,
                on_item.info().id,
                registry.boot,
                &[],
                None,
                Some(url),
                None,
                1,
            )
            .unwrap();
        }
        let asked = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = asked.clone();
        let lookup: PrLookup = Arc::new(move |_, branch| {
            log.lock().unwrap().push(branch.to_owned());
            let (n, author) = match branch {
                "feat/mine" => (1, "me"),
                "feat/theirs" => (2, "someone"),
                "feat/tracked" => (3, "me"),
                _ => return Pr::None,
            };
            Pr::Open {
                url: format!("https://github.com/o/r/pull/{n}"),
                failed: vec![],
                conflicts: Some("main".into()),
                head: "h1".into(),
                settled: true,
                author: author.into(),
            }
        });
        let flagged = registry.watch_branches(lookup.clone(), "me").await;
        assert_eq!(flagged, 1);
        let mut branches = asked.lock().unwrap().clone();
        branches.sort();
        assert_eq!(
            branches,
            ["feat/bare", "feat/mine", "feat/theirs"],
            "one call per branch"
        );
        let flag = registry.flag_of(&a, a.info().id).expect("flagged");
        assert_eq!(flag.reason, "PR #1 conflicts with main");
        assert_eq!(flag.step.as_ref().unwrap().action, StepAction::MergeBase);
        for s in [&human, &on_main, &review, &on_item, &no_pr] {
            assert!(s.flag().is_none(), "session {} flagged", s.info().id);
        }

        // Left: recorded as found by branch, so health can compare it.
        AS_CALLER.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(|| Caller {
                agent: None,
                session: None,
                bridge: false,
                desktop: false,
            }))
        });
        registry
            .take_step(a.info().id, flag.seq, StepAction::Leave, None)
            .await
            .unwrap();
        let root = project::root(&repo);
        let now = crate::session::now_ms() / 1000;
        let answers = registry.context.lock().unwrap().steps(&root, now);
        assert_eq!(answers.len(), 1);
        assert_eq!(
            (answers[0].via, answers[0].taken),
            (Via::Branch, StepAction::Leave)
        );
        let said = valkyrie_context::steps::reach(&answers, &[]).unwrap();
        assert!(
            said.contains("by branch: 1 answered, 1 left (100%)"),
            "{said}"
        );

        // The branch with no PR isn't asked about again while its commit stays.
        asked.lock().unwrap().clear();
        registry.watch_branches(lookup.clone(), "me").await;
        assert!(!asked.lock().unwrap().contains(&"feat/bare".to_owned()));
        git(&bare, &["commit", "-q", "--allow-empty", "-m", "two"]);
        registry.watch_branches(lookup, "me").await;
        assert!(asked.lock().unwrap().contains(&"feat/bare".to_owned()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// With no item, a PR of the user's that the branch watch saw open and then
    /// merged wakes the session on its branch and tells the repository's other
    /// agents (backlog #121), once. One first seen merged (merged before anyone
    /// watched) and somebody else's say nothing.
    #[tokio::test]
    async fn a_pr_seen_open_then_merged_tells_its_branchs_agents() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = std::env::temp_dir().join(format!("valkyrie-brmerged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "one"]);
        let wt = |branch: &str| {
            let path = dir.join(branch.replace('/', "-"));
            let path_s = path.to_str().unwrap().to_owned();
            git(&repo, &["worktree", "add", "-q", "-b", branch, &path_s]);
            path
        };
        let (mine, old, theirs) = (wt("feat/mine"), wt("feat/old"), wt("feat/theirs"));
        let (registry, _) = crate::new_registry(
            dir.join("v.sock"),
            &dir.join("state"),
            Path::new("/bin/true"),
            1,
            0,
            7,
            false,
        )
        .unwrap();
        registry.supervisor.lock().unwrap().context_on = Some(|| true);
        let spawn = |cwd: &Path| {
            let spec = SpawnSpec {
                command: vec!["sleep".into(), "30".into()],
                cwd: Some(cwd.to_owned()),
                name: None,
                size: Size { cols: 40, rows: 10 },
                env: Vec::new(),
                worktree: None,
                hidden: false,
            };
            let Reply::Session { info } = crate::spawn(&registry, spec).unwrap() else {
                panic!("no session");
            };
            let s = registry.get(info.id).unwrap();
            s.driven_by("claude");
            s
        };
        let (a, b, c, on_main) = (spawn(&mine), spawn(&old), spawn(&theirs), spawn(&repo));
        let merged = Arc::new(AtomicBool::new(false));
        let now = merged.clone();
        let lookup: PrLookup = Arc::new(move |_, branch| {
            let (n, author) = match branch {
                "feat/mine" => (1, "me"),
                "feat/old" => (2, "me"),
                "feat/theirs" => (3, "someone"),
                _ => return Pr::None,
            };
            let url = format!("https://github.com/o/r/pull/{n}");
            if n == 2 || now.load(Ordering::Relaxed) {
                return Pr::Merged {
                    url,
                    base: "main".into(),
                    author: author.into(),
                };
            }
            Pr::Open {
                url,
                failed: vec![],
                conflicts: None,
                head: "h1".into(),
                settled: true,
                author: author.into(),
            }
        });
        let state = dir.join("state/sessions");
        let told = |s: &Arc<crate::session::Session>| -> Vec<bool> {
            std::fs::read_to_string(state.join(format!("{}.events.jsonl", s.key())))
                .unwrap_or_default()
                .lines()
                .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .filter(|v| v["k"] == "merged")
                .map(|v| v["typed"].as_bool().unwrap())
                .collect()
        };
        assert_eq!(registry.watch_branches(lookup.clone(), "me").await, 0);
        assert!([&a, &b, &c, &on_main].iter().all(|s| told(s).is_empty()));
        merged.store(true, Ordering::Relaxed);
        registry.watch_branches(lookup.clone(), "me").await;
        assert_eq!(told(&a), [true]);
        assert!(a.take_merged().is_empty());
        for s in [&b, &c] {
            let news = s.take_merged();
            assert_eq!(news.len(), 1, "{news:?}");
            assert!(news[0].contains("/pull/1"), "{news:?}");
        }
        assert_eq!(told(&on_main), [false]);
        assert!(on_main.take_merged()[0].contains("/pull/1"));
        // Read merged again: not news.
        registry.watch_branches(lookup, "me").await;
        assert_eq!(told(&a), [true]);
        assert!(
            [&b, &c, &on_main]
                .iter()
                .all(|s| s.take_merged().is_empty())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

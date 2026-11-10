//! Telling agents a pull request merged (backlog #121), so nobody waits on the
//! user to say so. The PR's own sessions, sitting idle, get a fixed wrap-up
//! typed in: the merge ended their work. Every other agent in the repository is
//! told on its next prompt that the base moved, without being woken. A session
//! of the PR's that's at work, or with the supervisor or Context setting off,
//! is told on its next prompt too: nothing is typed then.

use crate::Registry;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use valkyrie_context::{backlog, project};
use valkyrie_proto::{Item, SessionId};

impl Registry {
    /// The live agent sessions on `item`'s pull request: those this daemon
    /// launched or that took it, and any running in its checkout (from before a
    /// restart, say). Read before it ships, while the checkout is still on the card.
    pub(crate) fn item_sessions(&self, item: &Item) -> Vec<SessionId> {
        let mut own: Vec<SessionId> = if item.boot == self.boot {
            item.sessions.clone()
        } else {
            Vec::new()
        };
        if let Some(checkout) = &item.checkout {
            for s in self.listed() {
                let id = s.info().id;
                if !s.exited() && s.info().cwd.starts_with(checkout) && !own.contains(&id) {
                    own.push(id);
                }
            }
        }
        own
    }

    /// Tells the agents that `url` merged into `base`: `own` (the PR's sessions)
    /// to wrap up, typed in when they're idle; every other live agent in the
    /// repository at `root`, on its next prompt. Each session is told once.
    pub(crate) fn announce_merge(
        &self,
        url: &str,
        base: &str,
        own: &[SessionId],
        root: &Path,
        item: Option<&Item>,
    ) {
        let typing = {
            let sup = self.supervisor.lock().unwrap();
            sup.on() && sup.context_on.unwrap_or(crate::supervise::context_on)()
        };
        let mut told = HashSet::new();
        for &id in own {
            let Ok(session) = self.get(id) else {
                continue;
            };
            if session.exited() || session.agent_behind().is_none() || !told.insert(id) {
                continue;
            }
            let wrap_up = backlog::merged_own(url, item);
            // Only between turns: never into a turn, or a question to the user.
            let typed = typing && session.resting();
            if typed {
                // A step offered for its CI or a conflict is moot now.
                self.drop_offer(id);
                session.prompt(&wrap_up);
            } else {
                session.merged_news(wrap_up);
            }
            session.log_merged(url, typed);
            tracing::info!(session = id, pr = url, typed, "told an agent its PR merged");
        }
        let line = backlog::merged_other(url, base, item);
        let root = root.to_path_buf();
        for s in self.listed() {
            let id = s.info().id;
            if s.exited() || s.agent_behind().is_none() || told.contains(&id) {
                continue;
            }
            if project::root(&s.info().cwd) != root {
                continue;
            }
            s.merged_news(line.clone());
            s.log_merged(url, false);
        }
    }

    /// The repository an item's work is in, for `announce_merge`.
    pub(crate) fn root_of(item: &Item) -> Option<PathBuf> {
        item.project
            .as_deref()
            .or(item.checkout.as_deref())
            .map(project::root)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use crate::work::{Pr, PrLookup};
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;
    use valkyrie_proto::{ItemChange, ItemStatus, NewItem, Reply, Size, SpawnSpec};

    /// The `merged` lines of a session's event log: whether each was typed.
    fn told(state: &Path, s: &crate::session::Session) -> Vec<bool> {
        std::fs::read_to_string(state.join(format!("sessions/{}.events.jsonl", s.key())))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["k"] == "merged")
            .map(|v| v["typed"].as_bool().unwrap())
            .collect()
    }

    /// A merged PR wakes its item's idle session with the wrap-up, saves it for
    /// one at work, and tells the repository's other agents on their next
    /// prompt; a human's session hears nothing, and the next poll says nothing new.
    #[tokio::test]
    async fn a_merged_pr_tells_its_agents_and_the_repositorys() {
        let dir = std::env::temp_dir().join(format!("valkyrie-merged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        // Its own repository, whatever is above the temp dir (decision #10).
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repo)
                .status()
                .unwrap()
                .success()
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
        registry.supervisor.lock().unwrap().context_on = Some(|| true);
        let state = dir.join("state");
        let told = |s| told(&state, s);
        let spawn = |agent: bool| {
            let spec = SpawnSpec {
                command: vec!["sleep".into(), "30".into()],
                cwd: Some(repo.clone()),
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
        let (idle, busy, other, human) = (spawn(true), spawn(true), spawn(true), spawn(false));
        busy.hook(
            "claude",
            1,
            serde_json::json!({"hook_event_name": "UserPromptSubmit", "prompt": "more"}),
        );
        let url = "https://github.com/o/r/pull/9";
        let id = {
            let b = registry.backlog.lock().unwrap();
            let new = NewItem {
                title: "the work".into(),
                cwd: Some(repo.clone()),
                ..Default::default()
            };
            let item = b.add(&new, "human", 1).unwrap();
            let open = ItemChange::Status {
                status: ItemStatus::Open,
            };
            b.change(item.id, &open, 1).unwrap();
            for s in [&idle, &busy] {
                b.taken(
                    item.id,
                    s.info().id,
                    registry.boot,
                    &[],
                    None,
                    Some(url),
                    None,
                    1,
                )
                .unwrap();
            }
            item.id
        };
        let lookup: PrLookup = Arc::new(|_, pr| Pr::merged(pr));
        assert_eq!(registry.ship_merged_prs(lookup.clone()).await, 1);
        // Idle: typed in, nothing left for its next prompt.
        assert_eq!(told(&idle), [true]);
        assert!(idle.take_merged().is_empty());
        // At work: saved for its next prompt, the same wrap-up.
        assert_eq!(told(&busy), [false]);
        let saved = busy.take_merged();
        assert_eq!(saved.len(), 1);
        assert!(
            saved[0].contains(url) && saved[0].contains(&format!("#{id}")),
            "{saved:?}"
        );
        assert!(saved[0].contains("then stop"), "{saved:?}");
        // Another agent in the repository: the base moved, on its next prompt.
        assert_eq!(told(&other), [false]);
        let news = other.take_merged();
        assert_eq!(news.len(), 1);
        assert!(news[0].starts_with("Merged into `main`"), "{news:?}");
        assert!(news[0].contains(url), "{news:?}");
        assert!(told(&human).is_empty() && human.take_merged().is_empty());
        // Shipped, it isn't read again: nobody is told twice.
        assert_eq!(registry.ship_merged_prs(lookup).await, 0);
        for s in [&idle, &busy, &other] {
            assert_eq!(told(s).len(), 1);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! The automatic sweep of worktrees nobody needs any more (backlog #235), so
//! agents' checkouts and their build output don't fill the disk. Shipping an
//! item takes its worktree away, but not while a session still sits in it, and
//! one stopped with Ctrl+C leaves its shell there; worktrees made outside the
//! backlog went only by hand (`valk worktree prune --yes`). Now and then the
//! daemon looks at every worktree of the repositories it knows of and takes
//! away those that are done:
//!
//! - its backlog item shipped or was dropped (while it's still on the item's
//!   branch), or it's merged into the default branch with commits of its own, or
//!   nothing moved in it for `STALE_SECS` and it's all merged or on `origin`;
//! - and it's clean, unlocked, no open item is on it, and no agent or program
//!   runs in it. A shell left idle at its prompt there is closed first.
//!
//! Never forced, as `worktree::remove` never is: a branch that isn't merged
//! stays, with its commits. `worktree_sweep = off` in `settings.toml` stops it.

use crate::Registry;
use crate::lock::Locked;
use crate::worktree::{self, Linked};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use valkyrie_context::{backlog, project};
use valkyrie_proto::{Item, ItemStatus, SessionId};

/// Between sweeps: each runs `git status` in every worktree.
const EVERY: Duration = Duration::from_secs(10 * 60);
/// Before the first, so sessions a restart brings back are in place to be seen.
const FIRST: Duration = Duration::from_secs(5 * 60);
/// Untouched this long, a worktree `origin` has all of is abandoned.
pub(crate) const STALE_SECS: u64 = 14 * 24 * 60 * 60;
/// Moved this lately, a worktree may be in use from outside any session in it
/// (an agent working there by path, an editor), so it waits.
const RECENT_SECS: u64 = 60 * 60;
/// Ignored files that are only build output or tool state, rebuilt on demand:
/// anything else ignored (`.env`, a local database, an editor's swap file)
/// keeps the worktree, since nobody is asked before it would go.
const REBUILT: [&str; 17] = [
    "target",
    "node_modules",
    "dist",
    "build",
    "gen",
    "out",
    ".omc",
    ".next",
    ".turbo",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
    ".pytest_cache",
    ".mypy_cache",
    "coverage",
    ".gradle",
];
/// A shell left at its prompt this long is idle (see `Session::idle_shell`).
const SHELL_QUIET_MS: u64 = 10 * 60 * 1000;

/// Whether the sweep is on: by default, unless `settings.toml` says
/// `worktree_sweep = off` (or `$VALK_WORKTREE_SWEEP` does).
fn on() -> bool {
    crate::supervise::setting_on("VALK_WORKTREE_SWEEP", "worktree_sweep")
}

/// Sweeps every `EVERY`, from `FIRST` after the daemon starts.
pub(crate) async fn run(registry: Arc<Registry>) {
    tokio::time::sleep(FIRST).await;
    let mut interval = tokio::time::interval(EVERY);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if on() {
            registry.sweep_worktrees(now_secs()).await;
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn canonical(p: &Path) -> PathBuf {
    valkyrie_proto::canonical(p).unwrap_or_else(|_| p.to_path_buf())
}

/// A live session as the sweep sees it.
pub(crate) struct Seen {
    pub id: SessionId,
    /// Where it runs now, and where it started.
    pub cwd: PathBuf,
    pub started_in: PathBuf,
    /// A shell left at its prompt, which may be closed.
    pub idle_shell: bool,
}

/// What becomes of one worktree.
#[derive(Debug, PartialEq)]
pub(crate) enum Verdict {
    Keep(String),
    /// Take it away, for `why`, after closing the idle shells `close`.
    Go {
        why: String,
        close: Vec<SessionId>,
    },
}

/// Judges `wt`, given the backlog items whose checkout it is (`items`) and the
/// live sessions (`sessions`), at `now` (seconds).
pub(crate) fn judge(wt: &Linked, items: &[&Item], sessions: &[Seen], now: u64) -> Verdict {
    let keep = |why: &str| Verdict::Keep(why.to_owned());
    if wt.missing {
        return keep("its folder is gone");
    }
    if let Some(open) = items
        .iter()
        .find(|i| matches!(i.status, ItemStatus::Open | ItemStatus::Proposed))
    {
        return Verdict::Keep(format!("backlog #{} is open on it", open.number));
    }
    if wt.locked {
        return keep("locked");
    }
    if wt.dirty > 0 {
        return Verdict::Keep(format!("has changes ({} files)", wt.dirty));
    }
    let precious: Vec<&str> = wt
        .ignored
        .iter()
        .map(String::as_str)
        .filter(|e| !rebuilt(e))
        .collect();
    if !precious.is_empty() {
        let shown = precious[..precious.len().min(3)].join(", ");
        return Verdict::Keep(format!(
            "has ignored files that aren't build output: {shown}"
        ));
    }
    let landed = wt.merged || wt.squashed;
    let idle_for = wt.moved.map(|at| now.saturating_sub(at));
    if idle_for.is_some_and(|s| s < RECENT_SECS) {
        return keep("moved within the hour");
    }
    // Still on the item's branch: a checkout reused for other work since, or
    // a detached one, is judged as any other.
    let finished = items
        .iter()
        .find(|i| i.branch.is_some() && i.branch == wt.branch);
    let why = if let Some(done) = finished {
        let how = match done.status {
            ItemStatus::Dropped => "dropped",
            _ => "shipped",
        };
        format!("backlog #{} {how}", done.number)
    } else if landed && !wt.no_commits {
        "merged".to_owned()
    } else if (landed || wt.pushed) && idle_for.is_some_and(|s| s >= STALE_SECS) {
        let days = idle_for.unwrap_or(0) / (24 * 60 * 60);
        let held = if landed { "merged" } else { "pushed" };
        format!("untouched for {days} days, all of it {held}")
    } else {
        return keep("not done");
    };
    let mut close = Vec::new();
    for s in sessions {
        if !s.cwd.starts_with(&wt.path) && !s.started_in.starts_with(&wt.path) {
            continue;
        }
        if !s.idle_shell {
            return Verdict::Keep(format!("session {} runs in it", s.id));
        }
        close.push(s.id);
    }
    Verdict::Go { why, close }
}

/// Whether the ignored entry `entry` (`web/node_modules/`, `x.tsbuildinfo`) is
/// only build output.
fn rebuilt(entry: &str) -> bool {
    let name = entry.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    REBUILT.contains(&name) || name.ends_with(".tsbuildinfo")
}

impl Registry {
    /// One sweep at `now` (seconds): takes away every worktree `judge` says is
    /// done, in each repository a backlog item or a live session is in, and takes
    /// the checkout off the cards of finished items whose folder is gone. Returns
    /// what it took away, each with why.
    pub(crate) async fn sweep_worktrees(&self, now: u64) -> Vec<String> {
        let items = self.backlog.locked().load();
        let mut done = Vec::new();
        let mut changed = self.clear_gone(&items);
        let mut repos: Vec<PathBuf> = items
            .iter()
            .filter_map(|i| i.project.clone())
            .chain(self.all().iter().map(|s| s.started_in().to_path_buf()))
            .filter(|p| p.is_dir() && project::in_git(p))
            .map(|p| canonical(&project::root(&p)))
            .collect();
        repos.sort();
        repos.dedup();
        for repo in repos {
            let asked = repo.clone();
            let Ok(Ok(survey)) =
                tokio::task::spawn_blocking(move || worktree::survey(&asked, true)).await
            else {
                continue;
            };
            for wt in survey.linked {
                // Read again for each: a survey can take a while, and an item
                // launched on this checkout meanwhile must count.
                let items = self.backlog.locked().load();
                let mine: Vec<&Item> = items
                    .iter()
                    .filter(|i| {
                        i.checkout
                            .as_deref()
                            .is_some_and(|c| canonical(c) == wt.path)
                    })
                    .collect();
                let sessions = self.seen(now);
                let Verdict::Go { why, close } = judge(&wt, &mine, &sessions, now) else {
                    continue;
                };
                // Nothing moved or changed there since the survey: else closing
                // a shell would come to nothing, as git keeps the worktree.
                if !close.is_empty() {
                    let (path, head) = (wt.path.clone(), wt.head.clone());
                    let same =
                        tokio::task::spawn_blocking(move || worktree::unchanged(&path, &head))
                            .await
                            .unwrap_or(false);
                    if !same {
                        continue;
                    }
                }
                for id in close {
                    if let Err(e) = self.kill_session(id) {
                        tracing::warn!(%id, "couldn't close an idle shell: {e:#}");
                    }
                }
                let repo = survey.repo.clone();
                let path = wt.path.clone();
                let branch = wt.branch.clone();
                let merged_at = (wt.merged || wt.squashed).then(|| wt.head.clone());
                let Ok((removed, said)) = tokio::task::spawn_blocking(move || {
                    worktree::remove(&repo, &path, branch.as_deref(), merged_at.as_deref())
                })
                .await
                else {
                    continue;
                };
                let line = format!("{said} ({why})");
                if !removed {
                    tracing::info!("worktree sweep: {line}");
                    continue;
                }
                tracing::info!("worktree sweep: {line}");
                for item in &mine {
                    self.swept_card(item, &line);
                    changed = true;
                }
                done.push(line);
            }
        }
        if changed {
            self.refresh_items();
        }
        done
    }

    /// The live sessions, each with whether it's a shell idle at its prompt.
    fn seen(&self, now: u64) -> Vec<Seen> {
        let now_ms = now * 1000;
        self.all()
            .into_iter()
            .filter(|s| !s.exited())
            .map(|s| Seen {
                id: s.id,
                cwd: canonical(&s.info().cwd),
                started_in: canonical(s.started_in()),
                idle_shell: s.idle_shell(now_ms, SHELL_QUIET_MS),
            })
            .collect()
    }

    /// Takes the checkout off the cards of shipped and dropped items whose folder
    /// is gone, as `git worktree remove` or a hand's `rm -rf` left them; whether
    /// any changed.
    fn clear_gone(&self, items: &[Item]) -> bool {
        let mut changed = false;
        for item in items {
            let Some(checkout) = &item.checkout else {
                continue;
            };
            if !matches!(item.status, ItemStatus::Shipped | ItemStatus::Dropped)
                || checkout.exists()
            {
                continue;
            }
            match self.backlog.locked().cleared(item.id, now_secs()) {
                Ok(_) => changed = true,
                Err(e) => tracing::warn!(id = item.id, "couldn't clear its checkout: {e:#}"),
            }
        }
        changed
    }

    /// Clears a finished item's checkout once the sweep took it away, with a
    /// note saying so.
    fn swept_card(&self, item: &Item, line: &str) {
        let backlog = self.backlog.locked();
        if let Err(e) = backlog.cleared(item.id, now_secs()) {
            tracing::warn!(id = item.id, "couldn't clear its checkout: {e:#}");
            return;
        }
        let text = format!("Swept: {line}");
        let origin = backlog::NoteOrigin::Brief;
        if let Err(e) = backlog.note(item.id, "valkyrie", &text, origin, now_secs()) {
            tracing::warn!(id = item.id, "couldn't note the sweep: {e:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::process::Command;
    #[cfg(unix)]
    use valkyrie_proto::{ItemChange, NewItem, Reply, Size, SpawnSpec};

    #[cfg(unix)]
    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(ok.status.success(), "git {args:?}: {ok:?}");
    }

    #[cfg(unix)]
    fn has_branch(dir: &Path, branch: &str) -> bool {
        Command::new("git")
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .current_dir(dir)
            .status()
            .unwrap()
            .success()
    }

    /// A worktree beside `main` on a new `branch`, with a commit of its own
    /// unless `commit` is false.
    #[cfg(unix)]
    fn worktree(main: &Path, branch: &str, commit: bool) -> PathBuf {
        let path = main.with_file_name(format!("app-{branch}"));
        git(
            main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                branch,
                &path.to_string_lossy(),
            ],
        );
        if commit {
            std::fs::write(path.join(format!("{branch}.txt")), branch).unwrap();
            git(&path, &["add", "."]);
            git(&path, &["commit", "-q", "-m", branch]);
        }
        path
    }

    fn linked(path: &str) -> Linked {
        Linked {
            path: PathBuf::from(path),
            branch: Some("b".into()),
            head: "h".into(),
            merged: false,
            no_commits: false,
            squashed: false,
            upstream_gone: false,
            locked: false,
            pushed: false,
            moved: Some(0),
            missing: false,
            dirty: 0,
            ignored: Vec::new(),
        }
    }

    fn seen(id: u32, cwd: &str, idle_shell: bool) -> Seen {
        Seen {
            id,
            cwd: PathBuf::from(cwd),
            started_in: PathBuf::from("/elsewhere"),
            idle_shell,
        }
    }

    #[test]
    fn judges_what_is_done_and_what_still_runs() {
        let now = STALE_SECS;
        let mut wt = linked("/r/app-b");
        // Unmerged, unpushed, fresh: not done, however old.
        assert_eq!(judge(&wt, &[], &[], now), Verdict::Keep("not done".into()));
        // Pushed but touched lately: still not done.
        wt.pushed = true;
        wt.moved = Some(now - RECENT_SECS);
        assert_eq!(judge(&wt, &[], &[], now), Verdict::Keep("not done".into()));
        // Pushed and untouched past STALE_SECS: abandoned.
        wt.moved = Some(0);
        assert!(matches!(judge(&wt, &[], &[], now), Verdict::Go { .. }));
        // Merged with commits of its own goes at once; a merged one with none
        // only once it's stale, since it may be a worktree just made.
        let mut merged = linked("/r/app-b");
        merged.merged = true;
        merged.moved = Some(now - 60);
        assert_eq!(
            judge(&merged, &[], &[], now),
            Verdict::Keep("moved within the hour".into())
        );
        merged.moved = Some(now - RECENT_SECS);
        assert!(matches!(judge(&merged, &[], &[], now), Verdict::Go { .. }));
        // Build output goes with it; anything else ignored keeps it.
        merged.ignored = vec!["web/node_modules/".into(), "x.tsbuildinfo".into()];
        assert!(matches!(judge(&merged, &[], &[], now), Verdict::Go { .. }));
        merged.ignored.push(".env".into());
        assert_eq!(
            judge(&merged, &[], &[], now),
            Verdict::Keep("has ignored files that aren't build output: .env".into())
        );
        merged.ignored.clear();
        merged.no_commits = true;
        assert_eq!(
            judge(&merged, &[], &[], now),
            Verdict::Keep("not done".into())
        );
        merged.moved = Some(0);
        assert!(matches!(judge(&merged, &[], &[], now), Verdict::Go { .. }));
        // Changes, a lock, an open item each keep it.
        let mut dirty = linked("/r/app-b");
        dirty.merged = true;
        dirty.dirty = 2;
        assert_eq!(
            judge(&dirty, &[], &[], now),
            Verdict::Keep("has changes (2 files)".into())
        );
        let mut open: Item = serde_json::from_value(serde_json::json!({
            "id": 1, "number": 7, "title": "t", "status": "open", "branch": "b",
            "created": 1, "updated": 1
        }))
        .unwrap();
        let mut shipped = linked("/r/app-b");
        shipped.moved = Some(now - RECENT_SECS);
        assert_eq!(
            judge(&shipped, &[&open], &[], now),
            Verdict::Keep("backlog #7 is open on it".into())
        );
        // A shipped item's goes, merged or not: its branch stays when it isn't.
        open.status = ItemStatus::Shipped;
        let go = judge(&shipped, &[&open], &[], now);
        assert_eq!(
            go,
            Verdict::Go {
                why: "backlog #7 shipped".into(),
                close: Vec::new()
            }
        );
        // Reused for another branch since, or detached, it's judged as any other
        // worktree.
        open.branch = Some("other".into());
        assert_eq!(
            judge(&shipped, &[&open], &[], now),
            Verdict::Keep("not done".into())
        );
        open.branch = Some("b".into());
        shipped.branch = None;
        assert_eq!(
            judge(&shipped, &[&open], &[], now),
            Verdict::Keep("not done".into())
        );
        shipped.branch = Some("b".into());
        // A program in it keeps it; an idle shell there is closed first.
        let busy = [seen(1, "/r/app-b/src", false), seen(2, "/r/app-b", true)];
        assert_eq!(
            judge(&shipped, &[&open], &busy, now),
            Verdict::Keep("session 1 runs in it".into())
        );
        let idle = [seen(2, "/r/app-b", true), seen(3, "/r/app-bb", false)];
        assert_eq!(
            judge(&shipped, &[&open], &idle, now),
            Verdict::Go {
                why: "backlog #7 shipped".into(),
                close: vec![2]
            }
        );
    }

    /// The sweep end to end: a shipped item's worktree waits for the program in
    /// it, then goes with its card cleared; a merged one goes with its branch once
    /// the shell idle in it is closed; changes and open items keep theirs; and a
    /// card whose folder is gone loses it.
    #[cfg(unix)]
    #[tokio::test]
    async fn sweeps_done_worktrees_and_keeps_the_rest() {
        let dir = std::env::temp_dir().join(format!("valkyrie-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let main = dir.join("app");
        std::fs::create_dir_all(&main).unwrap();
        let dir = canonical(&dir);
        let main = dir.join("app");
        git(&main, &["init", "-q", "-b", "main"]);
        git(&main, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let shipped = worktree(&main, "shipped", true);
        let merged = worktree(&main, "merged", true);
        git(&main, &["merge", "-q", "--ff-only", "merged"]);
        let dirty = worktree(&main, "dirty", false);
        std::fs::write(dirty.join("wip.txt"), "wip").unwrap();
        let open = worktree(&main, "open", true);
        let fresh = worktree(&main, "fresh", false);

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
        let spawn = |command: &[&str], cwd: &Path| {
            let spec = SpawnSpec {
                command: command.iter().map(|c| c.to_string()).collect(),
                cwd: Some(cwd.to_path_buf()),
                name: None,
                size: Size { cols: 40, rows: 10 },
                env: Vec::new(),
                worktree: None,
                hidden: false,
            };
            let Reply::Session { info } = crate::spawn(&registry, spec).unwrap() else {
                panic!("no session");
            };
            registry.get(info.id).unwrap()
        };
        let busy = spawn(&["sleep", "30"], &shipped);
        let shell = spawn(&["sh"], &merged);
        let item = |title: &str, status, checkout: &Path, branch: &str| {
            let b = registry.backlog.locked();
            let new = NewItem {
                title: title.into(),
                cwd: Some(main.clone()),
                ..Default::default()
            };
            let id = b.add(&new, "human", 1).unwrap().id;
            let to = |status| ItemChange::Status { status };
            b.change(id, &to(ItemStatus::Open), 1).unwrap();
            let checkout = Some((branch.to_owned(), checkout.to_path_buf()));
            b.launched(id, busy.id, registry.boot, &[], checkout, 1)
                .unwrap();
            if status != ItemStatus::Open {
                b.change(id, &to(status), 1).unwrap();
            }
            id
        };
        let shipped_id = item("shipped", ItemStatus::Shipped, &shipped, "shipped");
        let open_id = item("open", ItemStatus::Open, &open, "open");
        let gone_path = dir.join("app-gone");
        let gone_id = item("gone", ItemStatus::Shipped, &gone_path, "gone");

        // Let the shell reach its prompt and hand back the foreground.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let later = now_secs() + 2 * RECENT_SECS;
        let swept = registry.sweep_worktrees(later).await;
        // The shell idle in the merged one was closed, and it went with its branch.
        assert_eq!(swept.len(), 1, "{swept:?}");
        assert!(swept[0].contains("(merged)"), "{swept:?}");
        assert!(!merged.exists() && !has_branch(&main, "merged"));
        assert!(registry.get(shell.id).is_err(), "the idle shell stays open");
        // `sleep` runs in the shipped one: it stays until that ends.
        assert!(shipped.exists());
        for kept in [&dirty, &open, &fresh] {
            assert!(kept.exists(), "{} went", kept.display());
        }
        let card = |id| registry.backlog.locked().get(id).unwrap();
        assert_eq!(card(gone_id).checkout, None);
        assert_eq!(card(open_id).checkout.as_deref(), Some(open.as_path()));

        registry.kill_session(busy.id).unwrap();
        let swept = registry.sweep_worktrees(later).await;
        assert_eq!(swept.len(), 1, "{swept:?}");
        assert!(swept[0].contains(&format!("(backlog #{shipped_id} shipped)")));
        assert!(!shipped.exists());
        // Not merged: its branch and commit stay.
        assert!(has_branch(&main, "shipped"));
        let card = card(shipped_id);
        assert_eq!(card.checkout, None);
        assert!(
            card.notes.iter().any(|n| n.text.starts_with("Swept: ")),
            "{:?}",
            card.notes
        );

        // Two weeks on, the fresh one with nothing of its own is abandoned too;
        // the dirty one never goes.
        let swept = registry.sweep_worktrees(now_secs() + STALE_SECS + 60).await;
        assert_eq!(swept.len(), 1, "{swept:?}");
        assert!(!fresh.exists() && dirty.exists() && open.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

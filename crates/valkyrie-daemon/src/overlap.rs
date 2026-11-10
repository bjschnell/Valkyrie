//! The supervisor's watch on live agents editing the same files (ADR-0019 §1,
//! backlog #107). Two agents in separate worktrees of one repository only find out
//! when one's pull request conflicts, so each poll compares what they changed:
//! every file that differs from the point its branch left the default branch,
//! committed or not, plus new files git doesn't track yet. Agents that changed a
//! file in common each get a one-line note, "overlaps session X on src/foo.rs",
//! shown beside their state. It flags only, and types nothing into an agent.
//!
//! A poll of `git diff <merge-base>` reads what a merge would collide on, in any
//! worktree, committed or not. A file watch would need a watcher per worktree,
//! ignores for build output, and a diff afterwards anyway to learn what differs
//! from the base.

use crate::Registry;
use crate::activity::plain;
use crate::work;
use crate::worktree::{git_out, git_raw};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use valkyrie_context::project;
use valkyrie_proto::SessionId;

/// Files named per session in a note; the rest are counted.
const SHOWN: usize = 2;
const MAX_NAME: usize = 40;
const MAX_FILE: usize = 80;

/// One live agent's place and what it changed.
#[derive(Debug, Clone)]
struct Place {
    session: SessionId,
    name: String,
    /// The repository's main checkout, shared by its worktrees.
    repo: PathBuf,
    /// This session's own checkout: two sessions in one checkout share every file.
    checkout: PathBuf,
    files: BTreeSet<String>,
}

/// What differs from the default branch in `cwd`'s checkout: files changed since
/// the branch left it (committed, staged or not) and files git doesn't track yet.
/// `None` outside git, on a detached head, on the default branch itself (its work
/// has no pull request to conflict) or when there's no base to compare with.
fn changes(cwd: &Path) -> Option<(PathBuf, PathBuf, BTreeSet<String>)> {
    let checkout =
        std::fs::canonicalize(git_out(cwd, &["rev-parse", "--show-toplevel"]).ok()?).ok()?;
    let branch = work::current_branch(cwd)?;
    let default = work::default_branch_at(cwd)?;
    if branch == default {
        return None;
    }
    // What the branch will merge into: the remote's tip, which moves when others
    // merge, else the local branch.
    let base = [
        format!("refs/remotes/origin/{default}"),
        format!("refs/heads/{default}"),
    ]
    .into_iter()
    .find(|r| git_out(cwd, &["rev-parse", "--verify", "--quiet", r]).is_ok())?;
    let fork = git_out(cwd, &["merge-base", "HEAD", &base]).ok()?;
    let mut files = BTreeSet::new();
    let lists = [
        git_raw(cwd, &["diff", "--name-only", "--no-renames", "-z", &fork]).ok()?,
        git_raw(cwd, &["ls-files", "--others", "--exclude-standard", "-z"]).ok()?,
    ];
    for list in &lists {
        files.extend(
            list.split('\0')
                .filter(|f| !f.is_empty())
                .map(str::to_owned),
        );
    }
    Some((project::root(cwd), checkout, files))
}

/// The note for each session that shares a changed file with another one in its
/// repository, none for the rest. Sessions in one checkout don't count against
/// each other.
fn overlaps(places: &[Place]) -> BTreeMap<SessionId, String> {
    let mut notes = BTreeMap::new();
    for a in places {
        let mut parts = Vec::new();
        for b in places {
            if a.session == b.session || a.repo != b.repo || a.checkout == b.checkout {
                continue;
            }
            let shared: Vec<&String> = a.files.intersection(&b.files).collect();
            if shared.is_empty() {
                continue;
            }
            let mut on: Vec<String> = shared
                .iter()
                .take(SHOWN)
                .map(|f| plain(f, MAX_FILE))
                .collect();
            if shared.len() > SHOWN {
                on.push(format!("{} more", shared.len() - SHOWN));
            }
            parts.push(format!(
                "session {} on {}",
                plain(&b.name, MAX_NAME),
                on.join(", ")
            ));
        }
        if !parts.is_empty() {
            notes.insert(a.session, format!("overlaps {}", parts.join("; ")));
        }
    }
    notes
}

impl Registry {
    /// Notes on every live agent session that changed the same files as another in
    /// its repository, and clears the notes that no longer hold. Returns how many
    /// sessions show one now.
    pub(crate) async fn watch_overlaps(&self) -> usize {
        let sessions: Vec<_> = self
            .listed()
            .into_iter()
            .filter(|s| !s.exited() && s.agent_behind().is_some())
            .collect();
        let asked: Vec<(SessionId, String, PathBuf)> = sessions
            .iter()
            .map(|s| {
                let info = s.info();
                (info.id, info.name, info.cwd)
            })
            .collect();
        let places = tokio::task::spawn_blocking(move || {
            asked
                .into_iter()
                .filter_map(|(session, name, cwd)| {
                    let (repo, checkout, files) = changes(&cwd)?;
                    Some(Place {
                        session,
                        name,
                        repo,
                        checkout,
                        files,
                    })
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        let notes = overlaps(&places);
        let mut changed = false;
        for s in self.listed() {
            changed |= s.set_overlap(notes.get(&s.info().id).cloned());
        }
        if changed {
            self.host.changed.notify_one();
        }
        notes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(session: SessionId, name: &str, repo: &str, checkout: &str, files: &[&str]) -> Place {
        Place {
            session,
            name: name.into(),
            repo: repo.into(),
            checkout: checkout.into(),
            files: files.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    #[test]
    fn a_shared_file_notes_both_sessions() {
        let notes = overlaps(&[
            place(1, "api", "/r", "/r-a", &["src/foo.rs", "src/a.rs"]),
            place(2, "web", "/r", "/r-b", &["src/foo.rs"]),
            place(3, "docs", "/r", "/r-c", &["README.md"]),
        ]);
        assert_eq!(notes[&1], "overlaps session web on src/foo.rs");
        assert_eq!(notes[&2], "overlaps session api on src/foo.rs");
        assert!(!notes.contains_key(&3));
    }

    #[test]
    fn other_repositories_and_one_checkout_dont_count() {
        let notes = overlaps(&[
            place(1, "a", "/r", "/r", &["x.rs"]),
            place(2, "b", "/other", "/other", &["x.rs"]),
            place(3, "c", "/r", "/r", &["x.rs"]),
        ]);
        assert!(notes.is_empty(), "{notes:?}");
    }

    #[test]
    fn a_long_list_is_counted_and_each_session_named() {
        let notes = overlaps(&[
            place(1, "a", "/r", "/r-a", &["1", "2", "3", "4"]),
            place(2, "b", "/r", "/r-b", &["1", "2", "3"]),
            place(3, "c", "/r", "/r-c", &["4"]),
        ]);
        assert_eq!(
            notes[&1],
            "overlaps session b on 1, 2, 1 more; session c on 4"
        );
    }
}

#[cfg(all(test, unix))]
mod watch_tests {
    use std::path::Path;
    use std::process::Command;
    use valkyrie_proto::{Reply, Size, SpawnSpec};

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

    fn write(dir: &Path, file: &str, text: &str) {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// Two worktrees of one repository that changed the same file, one of them
    /// committed and the other not, are each noted with the other's name. A session
    /// on another file, one on the default branch, and a human's shell aren't, and
    /// the note goes once the overlap does. Their state is never touched.
    #[tokio::test]
    async fn agents_changing_the_same_file_are_noted_until_they_stop() {
        let dir = std::env::temp_dir().join(format!("valkyrie-overlap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        write(&repo, "src/foo.rs", "one\n");
        write(&repo, "src/bar.rs", "one\n");
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-q", "-m", "one"]);
        let wt = |branch: &str| {
            let path = dir.join(branch.replace('/', "-"));
            git(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    branch,
                    path.to_str().unwrap(),
                ],
            );
            path
        };
        let (api, web, docs) = (wt("feat/api"), wt("feat/web"), wt("feat/docs"));
        write(&api, "src/foo.rs", "api\n");
        git(&api, &["commit", "-q", "-am", "api"]);
        // Uncommitted, and the same new file as another's would collide on too.
        write(&web, "src/foo.rs", "web\n");
        write(&web, "src/new.rs", "web\n");
        write(&docs, "README.md", "docs\n");

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
        let spawn = |cwd: &Path, name: &str, agent: bool| {
            let spec = SpawnSpec {
                command: vec!["sleep".into(), "30".into()],
                cwd: Some(cwd.to_owned()),
                name: Some(name.into()),
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
        let (a, w, d) = (
            spawn(&api, "api", true),
            spawn(&web, "web", true),
            spawn(&docs, "docs", true),
        );
        let on_main = spawn(&repo, "main", true);
        let human = spawn(&web, "shell", false);
        let state = a.info().status.state;

        assert_eq!(registry.watch_overlaps().await, 2);
        let overlap = |s: &std::sync::Arc<crate::session::Session>| s.info().overlap;
        assert_eq!(
            overlap(&a).as_deref(),
            Some("overlaps session web on src/foo.rs")
        );
        assert_eq!(
            overlap(&w).as_deref(),
            Some("overlaps session api on src/foo.rs")
        );
        for s in [&d, &on_main, &human] {
            assert_eq!(overlap(s), None, "session {}", s.info().id);
        }
        assert_eq!(a.info().status.state, state, "only a note");

        // web puts its change back; the next poll clears both.
        git(&web, &["checkout", "-q", "--", "src/foo.rs"]);
        assert_eq!(registry.watch_overlaps().await, 0);
        assert_eq!((overlap(&a), overlap(&w)), (None, None));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

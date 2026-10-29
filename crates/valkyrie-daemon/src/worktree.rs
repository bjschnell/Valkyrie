//! A git worktree of its own for a new session (DESIGN §4.3), so agents working in
//! one repository at once never edit the same checkout.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use valkyrie_context::project;
use valkyrie_proto::{ItemStatus, NoWindow};

/// A worktree made for a session that hasn't started yet.
#[derive(Debug)]
pub struct Made {
    /// Where the session should start: the same subdirectory of the new checkout
    /// as the asked-for folder is of its own.
    pub start: PathBuf,
    path: PathBuf,
    checkout: PathBuf,
    /// The branch, if this made it.
    branch: Option<String>,
}

impl Made {
    /// Takes the worktree away again, for a session that then failed to start:
    /// nothing has run in it yet.
    pub fn undo(self) {
        let path = self.path.to_string_lossy();
        let _ = git(&self.checkout, &["worktree", "remove", "--force", &path]);
        if let Some(branch) = &self.branch {
            let _ = git(&self.checkout, &["branch", "--quiet", "-D", branch]);
        }
    }
}

/// Adds a worktree of the repository holding `dir`, beside the main checkout as
/// `<repo>-<branch>`. A `branch` that exists is checked out there; a new one starts
/// at the `HEAD` of `dir`'s checkout. An empty `branch` is named after `name`, or
/// `session`, made unique.
pub fn create(dir: &Path, branch: &str, name: Option<&str>) -> Result<Made> {
    // Without `\\?\` on Windows, which git and cmd.exe take for a network path.
    let canonical = |p: &Path| valkyrie_proto::canonical(p).unwrap_or_else(|_| p.to_path_buf());
    let dir = canonical(dir);
    let checkout = canonical(&project::checkout(&dir));
    anyhow::ensure!(
        checkout.join(".git").exists(),
        "{} is not in a git repository",
        dir.display()
    );
    let root = canonical(&project::root(&dir));
    let parent = root
        .parent()
        .context("the repository has no parent folder")?;
    let repo = root.file_name().context("the repository has no name")?;
    let place = |branch: &str| {
        let mut leaf = repo.to_os_string();
        leaf.push(format!("-{}", branch.replace('/', "-")));
        parent.join(leaf)
    };
    let add = |branch: &str, path: &Path, existing: bool| {
        let target = path.to_string_lossy();
        if existing {
            git(&checkout, &["worktree", "add", "--quiet", &target, branch])
        } else {
            let args = ["worktree", "add", "--quiet", "-b", branch, &target, "HEAD"];
            git(&checkout, &args)
        }
    };

    let (branch, path, existing) = if branch.is_empty() {
        let base = slug(name.unwrap_or("session"));
        let mut made = None;
        for n in 1..=100 {
            let branch = match n {
                1 => base.clone(),
                n => format!("{base}-{n}"),
            };
            let path = place(&branch);
            if path.exists() || has_branch(&checkout, &branch) {
                continue;
            }
            match add(&branch, &path, false) {
                Ok(()) => {
                    made = Some((branch, path));
                    break;
                }
                // Another spawn took the name in between: try the next.
                Err(_) if path.exists() || has_branch(&checkout, &branch) => continue,
                Err(e) => return Err(e),
            }
        }
        let (branch, path) = made.context("no free branch name")?;
        (branch, path, false)
    } else {
        valid(&checkout, branch)?;
        let path = place(branch);
        if path.exists() {
            bail!("{} already exists", path.display());
        }
        let existing = has_branch(&checkout, branch);
        add(branch, &path, existing)?;
        (branch.to_owned(), path, existing)
    };

    // From the checkout's top the rest is empty, and `join("")` would add a `/`.
    let start = dir
        .strip_prefix(&checkout)
        .ok()
        .filter(|r| !r.as_os_str().is_empty())
        .map(|r| path.join(r))
        .filter(|d| d.is_dir())
        .unwrap_or_else(|| path.clone());
    Ok(Made {
        start,
        path,
        checkout,
        branch: (!existing).then_some(branch),
    })
}

/// Takes away a worktree (a shipped backlog item's, or a pruned one), and its branch
/// if that's merged; returns whether the worktree went, and what to tell the user.
/// Never forced: git keeps a checkout with changes or untracked files, and a branch
/// with commits not merged here (a squash-merged pull request among them).
/// `merged_at` is the commit the caller itself found merged (see `survey`).
pub fn remove(
    repo: &Path,
    path: &Path,
    branch: Option<&str>,
    merged_at: Option<&str>,
) -> (bool, String) {
    let shown = path.display();
    if !path.is_dir() {
        return (true, format!("{shown} was already gone"));
    }
    if project::checkout(path) == project::checkout(repo) {
        return (false, format!("left {shown} alone: it's the main checkout"));
    }
    if let Err(e) = git(repo, &["worktree", "remove", &path.to_string_lossy()]) {
        return (false, format!("kept {shown}: {e:#}"));
    }
    let Some(branch) = branch.filter(|b| valid(repo, b).is_ok()) else {
        return (true, format!("took away {shown}"));
    };
    // `-d` asks whether the branch is merged into its upstream or this `HEAD`, and
    // the main checkout's `main` often lags `origin/main`, where the pull request
    // merged: it would keep every such branch. So `-D`, but only once the caller's
    // own check passed against `origin/<default>` or `HEAD`, and only while the
    // branch still points at the commit it checked.
    let reference = format!("refs/heads/{branch}");
    let still = |at: &str| {
        git_out(repo, &["rev-parse", "--verify", "--quiet", &reference]).is_ok_and(|t| t == at)
    };
    let flag = match merged_at {
        Some(at) if still(at) => "-D",
        _ => "-d",
    };
    match git(repo, &["branch", "--quiet", flag, branch]) {
        Ok(()) => (true, format!("took away {shown} and branch {branch}")),
        Err(_) => (
            true,
            format!("took away {shown}; kept branch {branch}, which isn't merged here"),
        ),
    }
}

/// Refuses what git couldn't name a branch, and what it would read as an option or
/// expand (`-f`, `@{-1}`) rather than take as a name.
fn valid(checkout: &Path, branch: &str) -> Result<()> {
    let reference = format!("refs/heads/{branch}");
    if branch.starts_with('-') || git(checkout, &["check-ref-format", &reference]).is_err() {
        bail!("{branch:?} is not a valid branch name");
    }
    Ok(())
}

fn has_branch(checkout: &Path, branch: &str) -> bool {
    let reference = format!("refs/heads/{branch}");
    git(checkout, &["rev-parse", "--verify", "--quiet", &reference]).is_ok()
}

/// What a backlog item's checkout holds against the repository's own checkout:
/// its commits, its whole change with what isn't committed yet, and how much
/// isn't. `None` when the checkout is gone or isn't a worktree of `repo`.
pub fn facts(repo: &Path, checkout: &Path) -> Option<valkyrie_proto::ItemFacts> {
    if !checkout.is_dir() || project::checkout(checkout) == project::checkout(repo) {
        return None;
    }
    let out = |dir: &Path, args: &[&str]| git_out(dir, args).ok();
    let main = out(repo, &["rev-parse", "HEAD"])?;
    let range = format!("{main}..HEAD");
    let base = out(checkout, &["merge-base", &main, "HEAD"])?;
    Some(valkyrie_proto::ItemFacts {
        branch: out(checkout, &["rev-parse", "--abbrev-ref", "HEAD"])?,
        ahead: out(checkout, &["rev-list", "--count", &range])?
            .parse()
            .ok()?,
        commits: out(checkout, &["log", "-n", "10", "--format=%h %s", &range])?
            .lines()
            .map(str::to_owned)
            .collect(),
        diff: out(checkout, &["diff", "--shortstat", &base])?,
        dirty: out(checkout, &["status", "--porcelain"])?.lines().count() as u32,
    })
}

/// A linked worktree of a repository, as git sees it.
#[derive(Debug)]
pub struct Linked {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: String,
    /// Its `HEAD` is in one of `Survey::against`.
    pub merged: bool,
    /// Its branch's upstream is gone: the remote deleted it, as a merged pull
    /// request does, squashed ones included.
    pub upstream_gone: bool,
    pub locked: bool,
    /// Its folder is gone, though git still lists it.
    pub missing: bool,
    /// Files changed or untracked but not committed.
    pub dirty: u32,
}

#[derive(Debug)]
pub struct Survey {
    /// The main checkout.
    pub repo: PathBuf,
    /// What merged is judged against: `origin/<default>` (with `remote`) and the main
    /// checkout's `HEAD`.
    pub against: Vec<String>,
    /// Every linked worktree; never the main checkout.
    pub linked: Vec<Linked>,
}

/// The linked worktrees of the repository holding `dir`, each checked for being
/// merged: its `HEAD` an ancestor of `origin`'s default branch (with `remote`) or of
/// the main checkout's `HEAD`. A squash-merged branch isn't.
pub fn survey(dir: &Path, remote: bool) -> Result<Survey> {
    let canonical = |p: &Path| valkyrie_proto::canonical(p).unwrap_or_else(|_| p.to_path_buf());
    anyhow::ensure!(
        project::checkout(dir).join(".git").exists(),
        "{} is not in a git repository",
        dir.display()
    );
    let repo = canonical(&project::root(dir));
    let mut against = Vec::new();
    let mut tips = Vec::new();
    if remote {
        let default = git_out(
            &repo,
            &[
                "symbolic-ref",
                "--quiet",
                "--short",
                "refs/remotes/origin/HEAD",
            ],
        )
        .ok()
        .into_iter()
        .chain(["origin/main".to_owned(), "origin/master".to_owned()])
        .find_map(|name| {
            let reference = format!("refs/remotes/{name}");
            let tip = git_out(&repo, &["rev-parse", "--verify", "--quiet", &reference]).ok()?;
            Some((name, tip))
        });
        if let Some((name, tip)) = default {
            against.push(name);
            tips.push(tip);
        }
    }
    if let Ok(tip) = git_out(&repo, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
        against.push(format!("HEAD of {}", repo.display()));
        tips.push(tip);
    }

    let listed = git_out(&repo, &["worktree", "list", "--porcelain"])?;
    let mut linked = Vec::new();
    // Blocks split by a blank line; the first is always the main checkout.
    for block in listed.split("\n\n").skip(1) {
        let field = |key: &str| {
            block.lines().find_map(|l| {
                l.strip_prefix(key)
                    .filter(|rest| rest.is_empty() || rest.starts_with(' '))
                    .map(str::trim)
            })
        };
        let (Some(path), Some(head)) = (field("worktree"), field("HEAD")) else {
            continue;
        };
        let path = canonical(Path::new(path));
        let head = head.to_owned();
        let branch = field("branch").map(|b| b.strip_prefix("refs/heads/").unwrap_or(b).to_owned());
        let merged = tips
            .iter()
            .any(|tip| git(&repo, &["merge-base", "--is-ancestor", &head, tip]).is_ok());
        let upstream_gone = branch.as_ref().is_some_and(|b| {
            let reference = format!("refs/heads/{b}");
            git_out(
                &repo,
                &["for-each-ref", "--format=%(upstream:track)", &reference],
            )
            .is_ok_and(|track| track == "[gone]")
        });
        let missing = !path.is_dir();
        let dirty = if missing {
            0
        } else {
            git_out(&path, &["status", "--porcelain"]).map_or(0, |out| out.lines().count() as u32)
        };
        linked.push(Linked {
            locked: field("locked").is_some(),
            path,
            branch,
            head,
            merged,
            upstream_gone,
            missing,
            dirty,
        });
    }
    Ok(Survey {
        repo,
        against,
        linked,
    })
}

impl crate::Registry {
    /// Judges each linked worktree of `cwd`'s repository, and with `remove` takes
    /// away those that may go: merged, clean, with no session running in it and no
    /// backlog item on it. Anyone may look; removing is the user's, as shipping is.
    pub(crate) async fn prune_worktrees(
        &self,
        cwd: PathBuf,
        remote: bool,
        remove: bool,
        peer: Option<crate::context::Peer>,
    ) -> Result<valkyrie_proto::Reply> {
        if remove {
            self.vouch(peer, "removing worktrees")?;
        }
        let asked = cwd.clone();
        let survey = tokio::task::spawn_blocking(move || survey(&cwd, remote)).await??;
        let items = self.backlog.lock().unwrap().load();
        let sessions: Vec<_> = self
            .listed()
            .into_iter()
            .filter(|s| !s.exited())
            .map(|s| s.info())
            .collect();
        let mut worktrees = Vec::new();
        for wt in survey.linked {
            let item = items.iter().find(|i| {
                matches!(i.status, ItemStatus::Open | ItemStatus::Proposed)
                    && i.checkout.as_ref().is_some_and(|c| {
                        valkyrie_proto::canonical(c).unwrap_or_else(|_| c.clone()) == wt.path
                    })
            });
            let session = sessions.iter().find(|s| s.cwd.starts_with(&wt.path));
            let why = if wt.missing {
                Some("its folder is gone; `git worktree prune` forgets it".to_owned())
            } else if let Some(item) = item {
                Some(format!(
                    "backlog #{0} is on it: `valk todo ship {0}` instead",
                    item.id
                ))
            } else if let Some(s) = session {
                Some(format!("session {} runs in it", s.id))
            } else if asked.starts_with(&wt.path) {
                Some("you're in it".to_owned())
            } else if wt.locked {
                Some("locked".to_owned())
            } else if !wt.merged && wt.upstream_gone {
                Some("not merged (squash merge?)".to_owned())
            } else if !wt.merged {
                Some("not merged".to_owned())
            } else if wt.dirty > 0 {
                Some(format!("has changes ({} files)", wt.dirty))
            } else {
                None
            };
            let prunable = why.is_none();
            let (removed, why) = match why {
                Some(why) => (false, why),
                None if remove => {
                    let repo = survey.repo.clone();
                    let (path, branch, head) = (wt.path.clone(), wt.branch.clone(), wt.head);
                    tokio::task::spawn_blocking(move || {
                        self::remove(&repo, &path, branch.as_deref(), Some(&head))
                    })
                    .await?
                }
                None => (false, "merged".to_owned()),
            };
            worktrees.push(valkyrie_proto::Worktree {
                path: wt.path,
                branch: wt.branch,
                prunable,
                removed,
                why,
            });
        }
        if remove {
            let gone = worktrees.iter().filter(|w| w.removed).count();
            tracing::info!(gone, "pruned worktrees");
        }
        Ok(valkyrie_proto::Reply::Worktrees {
            against: survey.against,
            worktrees,
        })
    }
}

/// Runs `git` in `dir`, returning what it printed, trimmed.
fn git_out(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .no_window()
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .context("couldn't run git")?;
    if !out.status.success() {
        bail!("git {} failed", args.first().unwrap_or(&""));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Runs `git` in `dir`, failing with what it printed.
fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let out = Command::new("git")
        .no_window()
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .context("couldn't run git")?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let err = err.trim();
    bail!(
        "git {}: {}",
        args.first().unwrap_or(&""),
        err.strip_prefix("fatal: ").unwrap_or(err)
    )
}

/// `Fix the login bug!` → `fix-the-login-bug`: a branch and folder name. Only ASCII
/// letters, digits and `_` stay, so it is always a valid branch name.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let out = out.trim_matches('-').to_owned();
    if out.is_empty() {
        "session".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository with one commit and a `src` folder, removed when dropped along
    /// with every worktree made beside it.
    struct Repo(PathBuf);

    impl Repo {
        fn new(tag: &str) -> Repo {
            let base =
                std::env::temp_dir().join(format!("valkyrie-wt-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let main = base.join("app");
            std::fs::create_dir_all(main.join("src")).unwrap();
            std::fs::write(main.join("src/lib.rs"), "").unwrap();
            for args in [
                &["init", "--quiet", "-b", "main"][..],
                &["add", "."],
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "--quiet",
                    "-m",
                    "init",
                ],
            ] {
                git(&main, args).unwrap();
            }
            // As `create` canonicalizes: without `\\?\` on Windows.
            Repo(valkyrie_proto::canonical(&base).unwrap())
        }

        fn main(&self) -> PathBuf {
            self.0.join("app")
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn names_a_new_branch_after_the_session_and_keeps_the_subfolder() {
        let repo = Repo::new("named");
        let at = create(&repo.main().join("src"), "", Some("Fix the login bug!"))
            .unwrap()
            .start;
        assert_eq!(at, repo.0.join("app-fix-the-login-bug/src"));
        assert!(at.join("lib.rs").exists());
        assert!(has_branch(&repo.main(), "fix-the-login-bug"));

        // The same name again gets the next free one.
        let again = create(&repo.main(), "", Some("Fix the login bug!"))
            .unwrap()
            .start;
        assert_eq!(again, repo.0.join("app-fix-the-login-bug-2"));
        // As text: `Path`'s `==` ignores a trailing `/`, which a session's `PWD`
        // keeps, and which `sh` (dash) then prints as the folder it's in.
        assert_eq!(
            again.to_str(),
            repo.0.join("app-fix-the-login-bug-2").to_str()
        );
    }

    #[test]
    fn from_a_worktree_the_new_one_still_goes_beside_the_main_checkout() {
        let repo = Repo::new("nested");
        let first = create(&repo.main(), "", None).unwrap().start;
        assert_eq!(first, repo.0.join("app-session"));
        let second = create(&first, "feature/x", None).unwrap().start;
        assert_eq!(second, repo.0.join("app-feature-x"));
    }

    #[test]
    fn checks_out_an_existing_branch() {
        let repo = Repo::new("existing");
        git(&repo.main(), &["branch", "old-work"]).unwrap();
        let at = create(&repo.main(), "old-work", None).unwrap().start;
        assert_eq!(at, repo.0.join("app-old-work"));
    }

    #[test]
    fn refuses_outside_git_and_bad_branch_names() {
        let repo = Repo::new("refuse");
        let plain = repo.0.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let err = create(&plain, "", None).unwrap_err().to_string();
        assert!(err.contains("not in a git repository"), "{err}");
        let err = create(&repo.main(), "bad..name", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a valid branch name"), "{err}");
    }

    #[test]
    fn undo_takes_away_the_worktree_and_the_branch_it_made() {
        let repo = Repo::new("undo");
        let made = create(&repo.main(), "", Some("oops")).unwrap();
        made.undo();
        assert!(!repo.0.join("app-oops").exists());
        assert!(!has_branch(&repo.main(), "oops"));

        // A branch that was there before stays.
        git(&repo.main(), &["branch", "keep"]).unwrap();
        create(&repo.main(), "keep", None).unwrap().undo();
        assert!(!repo.0.join("app-keep").exists());
        assert!(has_branch(&repo.main(), "keep"));
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Fix the login bug!"), "fix-the-login-bug");
        assert_eq!(slug("  --  "), "session");
        assert_eq!(slug("v1.2 release"), "v1-2-release");
        assert_eq!(slug("bump deps.lock"), "bump-deps-lock");
    }

    #[test]
    fn a_shipped_worktree_goes_only_when_clean_and_its_branch_only_when_merged() {
        let repo = Repo::new("remove");
        let clean = create(&repo.main(), "feat/1-x", None).unwrap().start;
        let (gone, said) = remove(&repo.main(), &clean, Some("feat/1-x"), None);
        assert!(gone && !clean.exists(), "{said}");
        assert!(
            !has_branch(&repo.main(), "feat/1-x"),
            "merged (no new commits): {said}"
        );

        let dirty = create(&repo.main(), "fix/2-y", None).unwrap().start;
        std::fs::write(dirty.join("new.txt"), "wip").unwrap();
        let (gone, said) = remove(&repo.main(), &dirty, Some("fix/2-y"), None);
        assert!(!gone && dirty.exists(), "{said}");
        assert!(said.starts_with("kept "), "{said}");

        let ahead = create(&repo.main(), "fix/3-z", None).unwrap().start;
        let commit = [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "z",
        ];
        git(&ahead, &commit).unwrap();
        let (gone, said) = remove(&repo.main(), &ahead, Some("fix/3-z"), None);
        assert!(gone && !ahead.exists(), "{said}");
        assert!(
            has_branch(&repo.main(), "fix/3-z"),
            "unmerged work stays: {said}"
        );

        let (gone, _) = remove(&repo.main(), &repo.main(), None, None);
        assert!(
            !gone && repo.main().exists(),
            "the main checkout is never taken"
        );
    }

    #[test]
    fn facts_count_commits_and_what_isnt_committed() {
        let repo = Repo::new("facts");
        assert!(facts(&repo.main(), &repo.main()).is_none());
        let at = create(&repo.main(), "fix/4-w", None).unwrap().start;
        let fresh = facts(&repo.main(), &at).unwrap();
        assert_eq!(
            (
                fresh.branch.as_str(),
                fresh.ahead,
                fresh.diff.as_str(),
                fresh.dirty
            ),
            ("fix/4-w", 0, "", 0)
        );
        std::fs::write(at.join("src/lib.rs"), "fn a() {}\n").unwrap();
        let commit = [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "--quiet",
            "-am",
            "add a",
        ];
        git(&at, &commit).unwrap();
        std::fs::write(at.join("new.txt"), "wip\n").unwrap();
        let f = facts(&repo.main(), &at).unwrap();
        assert_eq!((f.ahead, f.dirty), (1, 1));
        assert!(f.commits[0].ends_with(" add a"), "{:?}", f.commits);
        assert!(
            f.diff.starts_with("1 file changed, 1 insertion"),
            "{}",
            f.diff
        );
        std::fs::remove_dir_all(&at).unwrap();
        assert!(facts(&repo.main(), &at).is_none());
    }
}

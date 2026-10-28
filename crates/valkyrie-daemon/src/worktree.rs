//! A git worktree of its own for a new session (DESIGN §4.3), so agents working in
//! one repository at once never edit the same checkout.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use valkyrie_context::project;
use valkyrie_proto::NoWindow;

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

/// Takes away a worktree a backlog item's launch made, now that it shipped, and its
/// branch if that's merged; returns whether the worktree went, and what to tell the
/// user. Never forced: git keeps a checkout with changes or untracked files, and a
/// branch with commits not merged here (a squash-merged pull request among them).
pub fn remove(repo: &Path, path: &Path, branch: Option<&str>) -> (bool, String) {
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
    match git(repo, &["branch", "--quiet", "-d", branch]) {
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
        let (gone, said) = remove(&repo.main(), &clean, Some("feat/1-x"));
        assert!(gone && !clean.exists(), "{said}");
        assert!(
            !has_branch(&repo.main(), "feat/1-x"),
            "merged (no new commits): {said}"
        );

        let dirty = create(&repo.main(), "fix/2-y", None).unwrap().start;
        std::fs::write(dirty.join("new.txt"), "wip").unwrap();
        let (gone, said) = remove(&repo.main(), &dirty, Some("fix/2-y"));
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
        let (gone, said) = remove(&repo.main(), &ahead, Some("fix/3-z"));
        assert!(gone && !ahead.exists(), "{said}");
        assert!(
            has_branch(&repo.main(), "fix/3-z"),
            "unmerged work stays: {said}"
        );

        let (gone, _) = remove(&repo.main(), &repo.main(), None);
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

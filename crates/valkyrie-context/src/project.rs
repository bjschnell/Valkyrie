//! Which project a directory belongs to (ADR-0007 §2), read from disk without
//! running `git`: the injection hook runs on every session start.

use std::path::{Path, PathBuf};

/// The project holding `dir`: the main worktree's root of the git repository it is
/// in, so every worktree of a repo shares one project; outside git, `dir` itself.
pub fn root(dir: &Path) -> PathBuf {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    for top in dir.ancestors() {
        let git = top.join(".git");
        let Ok(meta) = std::fs::symlink_metadata(&git) else {
            continue;
        };
        if meta.is_dir() {
            return top.to_path_buf();
        }
        // A linked worktree's `.git` is a file naming its private git dir, whose
        // `commondir` leads back to the main repository's `.git`.
        return main_worktree(&git).unwrap_or_else(|| top.to_path_buf());
    }
    dir
}

/// The checkout holding `dir`: the nearest directory with a `.git` (a linked
/// worktree's own root, unlike `root`), or `dir` outside git.
pub fn checkout(dir: &Path) -> PathBuf {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    dir.ancestors()
        .find(|top| std::fs::symlink_metadata(top.join(".git")).is_ok())
        .map_or_else(|| dir.clone(), Path::to_path_buf)
}

/// `HEAD` of the checkout at `dir`, short; `None` outside git.
pub fn head(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !sha.is_empty()).then_some(sha)
}

fn main_worktree(git_file: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(git_file).ok()?;
    let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let gitdir = git_file.parent()?.join(gitdir);
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = std::fs::canonicalize(gitdir.join(common.trim())).ok()?;
    // A bare repository's worktrees have no main checkout; the repo itself names it.
    if common.file_name()? == ".git" {
        common.parent().map(Path::to_path_buf)
    } else {
        Some(common)
    }
}

/// The directory name a project's files live under: readable, and unique per root.
pub fn key(root: &Path) -> String {
    let name: String = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "root".into())
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => c,
            _ => '_',
        })
        .take(40)
        .collect();
    format!(
        "{name}-{:08x}",
        fnv1a(root.as_os_str().as_encoded_bytes()) as u32
    )
}

/// FNV-1a: stable across Rust versions, unlike `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("valk-ctx-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::canonicalize(dir).unwrap()
    }

    #[test]
    fn a_subdirectory_resolves_to_the_repo_root() {
        let repo = tmp("sub");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src/deep")).unwrap();
        assert_eq!(root(&repo.join("src/deep")), repo);
        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn a_linked_worktree_resolves_to_the_main_checkout() {
        let base = tmp("wt");
        let main = base.join("main");
        let private = main.join(".git/worktrees/feature");
        std::fs::create_dir_all(&private).unwrap();
        std::fs::write(private.join("commondir"), "../..\n").unwrap();
        let wt = base.join("feature");
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", private.display())).unwrap();
        assert_eq!(root(&wt.join("src")), main);
        assert_eq!(checkout(&wt.join("src")), wt);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn outside_git_the_directory_is_the_project() {
        let dir = tmp("plain");
        // A temp dir could sit inside a repo on some machines; only check the
        // fallback when it doesn't.
        if !dir.ancestors().any(|d| d.join(".git").exists()) {
            assert_eq!(root(&dir), dir);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keys_are_readable_and_distinct() {
        let a = key(Path::new("/home/u/repos/my app"));
        let b = key(Path::new("/work/repos/my app"));
        assert!(a.starts_with("my_app-"));
        assert_ne!(a, b);
        assert_eq!(a, key(Path::new("/home/u/repos/my app")));
    }
}

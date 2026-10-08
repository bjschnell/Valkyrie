//! Folders for the new-session launcher (DESIGN §8.8): where sessions already run,
//! the git repos under the usual roots, and a listing to browse into any folder.
//! Only directories are listed, never files.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Where people keep repos, under the home directory.
const ROOTS: &[&str] = &["repos", "src", "code", "projects", "dev", "work", "git"];
/// Repos suggested; the most recently touched first.
const MAX_REPOS: usize = 40;
/// Entries in one listing.
const MAX_LIST: usize = 400;

#[derive(Debug, Serialize, PartialEq)]
pub struct Place {
    pub path: String,
    /// `~/repos/valkyrie`, as it reads.
    pub label: String,
    pub git: bool,
}

#[derive(Debug, Serialize)]
pub struct Places {
    pub home: String,
    /// The shell a shell session runs.
    pub shell: String,
    /// Where sessions run now, most recent first.
    pub recent: Vec<Place>,
    /// Git repos under the usual roots, most recently changed first.
    pub repos: Vec<Place>,
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/bin/sh".into())
}

pub fn place(path: &Path, home: &Path) -> Place {
    Place {
        path: path.to_string_lossy().into_owned(),
        label: label(path, home),
        git: path.join(".git").exists(),
    }
}

fn label(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Git repos one level under each root, newest first.
pub fn repos(home: &Path) -> Vec<Place> {
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    for root in ROOTS {
        let Ok(entries) = std::fs::read_dir(home.join(root)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || hidden(&path) {
                continue;
            }
            let git = path.join(".git");
            if let Ok(meta) = git.metadata() {
                // The index changes with most git work; the directory itself rarely.
                let touched = git
                    .join("index")
                    .metadata()
                    .and_then(|m| m.modified())
                    .or_else(|_| meta.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                found.push((touched, path));
            }
        }
    }
    found.sort_by_key(|(touched, _)| std::cmp::Reverse(*touched));
    found
        .into_iter()
        .take(MAX_REPOS)
        .map(|(_, path)| place(&path, home))
        .collect()
}

fn hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

#[derive(Debug, Serialize)]
pub struct Listing {
    pub path: String,
    pub label: String,
    pub parent: Option<String>,
    pub git: bool,
    pub dirs: Vec<Place>,
}

/// The folders in `path` (`~` and `~/…` allowed), hidden ones last.
pub fn list(path: &str, home: &Path) -> std::io::Result<Listing> {
    let path = expand(path, home).canonicalize()?;
    let home = &home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&path)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort_by_key(|p| {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        (hidden(p), name)
    });
    dirs.truncate(MAX_LIST);
    Ok(Listing {
        label: label(&path, home),
        parent: path.parent().map(|p| p.to_string_lossy().into_owned()),
        git: path.join(".git").exists(),
        dirs: dirs.iter().map(|d| place(d, home)).collect(),
        path: path.to_string_lossy().into_owned(),
    })
}

pub fn expand(path: &str, home: &Path) -> PathBuf {
    let path = path.trim();
    if path == "~" || path.is_empty() {
        home.to_path_buf()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_repos_and_lists_folders() {
        let home = std::env::temp_dir().join(format!("valk-places-{}", std::process::id()));
        for dir in [
            "repos/a/.git",
            "repos/b/.git",
            "repos/plain",
            "repos/.hidden/.git",
            "code/c/.git",
        ] {
            std::fs::create_dir_all(home.join(dir)).unwrap();
        }
        std::fs::write(home.join("repos/a.txt"), "").unwrap();
        let mut labels: Vec<String> = repos(&home).into_iter().map(|p| p.label).collect();
        labels.sort();
        assert_eq!(labels, ["~/code/c", "~/repos/a", "~/repos/b"]);

        let listing = list("~/repos", &home).unwrap();
        assert_eq!(listing.label, "~/repos");
        let names: Vec<&str> = listing.dirs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(
            names,
            ["~/repos/a", "~/repos/b", "~/repos/plain", "~/repos/.hidden"]
        );
        assert!(listing.dirs[0].git && !listing.dirs[2].git);
        assert!(list("~/nope", &home).is_err());
        std::fs::remove_dir_all(&home).unwrap();
    }
}

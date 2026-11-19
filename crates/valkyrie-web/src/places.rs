//! Folders for the new-session launcher (DESIGN §8.8): where sessions already run,
//! and a listing to browse into any folder. The repositories it offers are the
//! daemon's one list (backlog #240). Only directories are listed, never files.

use serde::Serialize;
use std::path::{Path, PathBuf};

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
}

pub fn home() -> PathBuf {
    valkyrie_proto::home_dir()
}

pub fn shell() -> String {
    valkyrie_proto::default_shell()
}

pub fn place(path: &Path, home: &Path) -> Place {
    Place {
        path: path.to_string_lossy().into_owned(),
        label: label(path, home),
        git: valkyrie_context::project::is_checkout(path),
    }
}

fn label(path: &Path, home: &Path) -> String {
    valkyrie_context::repos::label(path, home)
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
    let path = valkyrie_proto::canonical(&expand(path, home))?;
    let home = &valkyrie_proto::canonical(home).unwrap_or_else(|_| home.to_path_buf());
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
        git: valkyrie_context::project::is_checkout(&path),
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
    fn lists_folders() {
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
        // A real repository has a HEAD; a stray empty `.git` is not one.
        for repo in ["repos/a", "repos/b", "repos/.hidden", "code/c"] {
            std::fs::write(home.join(repo).join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        }
        std::fs::create_dir_all(home.join("repos/stray/.git")).unwrap();

        let listing = list("~/repos", &home).unwrap();
        assert_eq!(listing.label, "~/repos");
        let names: Vec<&str> = listing.dirs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(
            names,
            [
                "~/repos/a",
                "~/repos/b",
                "~/repos/plain",
                "~/repos/stray",
                "~/repos/.hidden"
            ]
        );
        assert!(listing.dirs[0].git && !listing.dirs[2].git && !listing.dirs[3].git);
        assert!(list("~/nope", &home).is_err());
        std::fs::remove_dir_all(&home).unwrap();
    }
}

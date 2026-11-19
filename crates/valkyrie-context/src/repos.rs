//! The repositories every picker offers (backlog #240): one list, worked out
//! from where sessions run, the backlog's folders and a scan of parent folders,
//! plus the repositories the user added and less the ones they hid. Each is a
//! main checkout's root as git has it (`project::root`), so a worktree never
//! shows as a repository of its own and nothing is guessed from folder names.
//!
//! The user's changes are kept in `repos.json` in the config directory.

use crate::project;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;
use valkyrie_proto::{KnownRepo, RepoEdit, RepoList, ScanFolder};

/// Where people keep repositories, under the home directory: scanned until the
/// user changes which folders are.
pub const DEFAULT_FOLDERS: &[&str] = &["repos", "src", "code", "projects", "dev", "work", "git"];

/// Folders listed in one scanned folder, at most.
const MAX_SCAN: usize = 1000;

/// Read-modify-write of `repos.json` one at a time.
static EDITING: Mutex<()> = Mutex::new(());

/// The user's changes, as `repos.json` keeps them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Repositories added by hand.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<PathBuf>,
    /// The folders scanned; `None` until the user changes them: the defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folders: Option<Vec<PathBuf>>,
    /// Repositories never offered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<PathBuf>,
}

/// `repos.json` in the config directory.
pub fn path() -> PathBuf {
    valkyrie_proto::config_dir().join("repos.json")
}

impl Config {
    /// What `path` holds; nothing when it's missing. A broken file reads as
    /// nothing too rather than hiding every picker's choices.
    pub fn load(path: &Path) -> Config {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)? + "\n")?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// The folders scanned: the user's, else the defaults that exist.
    pub fn folders(&self, home: &Path) -> Vec<PathBuf> {
        match &self.folders {
            Some(folders) => folders.clone(),
            None => DEFAULT_FOLDERS
                .iter()
                .map(|d| home.join(d))
                .filter(|d| d.is_dir())
                .map(|d| key(&d))
                .collect(),
        }
    }
}

/// How two paths are compared (decision #69): git keeps worktree paths resolved,
/// so a symlinked or short-named spelling of the same folder must match. A path
/// that's gone compares by its canonical parent and its own name.
pub fn key(path: &Path) -> PathBuf {
    if let Ok(path) = valkyrie_proto::canonical(path) {
        return path;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
            valkyrie_proto::canonical(parent).map_or_else(|_| path.to_path_buf(), |p| p.join(name))
        }
        _ => path.to_path_buf(),
    }
}

/// `~/repos/valkyrie` for a path under `home`, else the path. Both as spelled:
/// pass a canonical `home` for canonical paths.
pub fn label(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => {
            let rest = rest.display().to_string();
            #[cfg(windows)]
            let rest = rest.replace('\\', "/");
            format!("~/{rest}")
        }
        Err(_) => path.display().to_string(),
    }
}

/// `~` and `~/…` expanded against `home`.
pub fn expand(path: &Path, home: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let text = text.trim();
    if text == "~" {
        home.to_path_buf()
    } else if let Some(rest) = text.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(text)
    }
}

/// When a repository was last worked in: its index changes with most git work.
fn touched(root: &Path) -> SystemTime {
    let git = root.join(".git");
    git.join("index")
        .metadata()
        .or_else(|_| git.metadata())
        .or_else(|_| root.metadata())
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn dot_named(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

/// The main checkouts one level under `folder`.
fn scan(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .flatten()
        .take(MAX_SCAN)
        .map(|e| e.path())
        .filter(|p| !dot_named(p) && p.is_dir() && project::is_checkout(p))
        .map(|p| project::root(&p))
        .collect()
}

/// The list every picker offers. `sessions` are where sessions run (only git
/// checkouts count), `folders` the backlog items' folders (each counts as it is,
/// since the user picked it). Reads the disk: call it off a connection's thread.
pub fn known(config: &Config, home: &Path, sessions: &[PathBuf], folders: &[PathBuf]) -> RepoList {
    let home = &key(home);
    let scanned = config.folders(home);
    let hidden: Vec<PathBuf> = config.hidden.iter().map(|h| key(h)).collect();
    let mut found: Vec<(PathBuf, bool)> = Vec::new();
    let found_in_git = sessions
        .iter()
        .filter(|d| project::in_git(d))
        .chain(folders.iter().filter(|d| d.is_dir()))
        .map(|d| (project::root(d), false));
    let added = config
        .added
        .iter()
        .filter(|d| d.is_dir())
        .map(|d| (project::root(d), true));
    let in_folders = scanned.iter().flat_map(|f| scan(f)).map(|d| (d, false));
    for (root, by_hand) in added.chain(found_in_git).chain(in_folders) {
        let root = key(&root);
        if project::is_user(&root) || hidden.contains(&root) {
            continue;
        }
        match found.iter_mut().find(|(r, _)| *r == root) {
            Some((_, was)) => *was |= by_hand,
            None => found.push((root, by_hand)),
        }
    }
    let mut repos: Vec<(SystemTime, KnownRepo)> = found
        .into_iter()
        .map(|(path, added)| {
            let repo = KnownRepo {
                label: label(&path, home),
                path,
                added,
            };
            (touched(&repo.path), repo)
        })
        .collect();
    repos.sort_by(|(a, x), (b, y)| b.cmp(a).then_with(|| x.path.cmp(&y.path)));
    let folder = |path: &PathBuf| ScanFolder {
        label: label(path, home),
        path: path.clone(),
    };
    RepoList {
        repos: repos.into_iter().map(|(_, r)| r).collect(),
        folders: scanned.iter().map(folder).collect(),
        hidden: config.hidden.iter().map(folder).collect(),
    }
}

/// Applies `edit` to `config`. Refuses a path that isn't there, a repository
/// that isn't a git checkout, and a folder to scan that isn't a folder.
pub fn edit(config: &mut Config, home: &Path, edit: &RepoEdit) -> Result<()> {
    let existing = |path: &Path| -> Result<PathBuf> {
        let path = expand(path, home);
        valkyrie_proto::canonical(&path).with_context(|| format!("no folder {}", path.display()))
    };
    let same = |a: &Path, b: &Path| key(a) == key(b);
    match edit {
        RepoEdit::Add { path } => {
            let path = existing(path)?;
            if !project::in_git(&path) {
                bail!("{} isn't a git repository", path.display());
            }
            let root = key(&project::root(&path));
            config.hidden.retain(|h| !same(h, &root));
            if !config.added.iter().any(|a| same(a, &root)) {
                config.added.push(root);
            }
        }
        RepoEdit::Hide { path } => {
            let path = expand(path, home);
            let root = if path.exists() {
                key(&project::root(&path))
            } else {
                key(&path)
            };
            config.added.retain(|a| !same(a, &root));
            if !config.hidden.iter().any(|h| same(h, &root)) {
                config.hidden.push(root);
            }
        }
        RepoEdit::Unhide { path } => {
            let path = expand(path, home);
            config.hidden.retain(|h| !same(h, &path));
        }
        RepoEdit::Scan { path } => {
            let path = existing(path)?;
            if !path.is_dir() {
                bail!("{} isn't a folder", path.display());
            }
            let mut folders = config.folders(home);
            if !folders.iter().any(|f| same(f, &path)) {
                folders.push(path);
            }
            config.folders = Some(folders);
        }
        RepoEdit::Unscan { path } => {
            let path = expand(path, home);
            let mut folders = config.folders(home);
            folders.retain(|f| !same(f, &path));
            config.folders = Some(folders);
        }
    }
    Ok(())
}

/// Applies `change` to the config at `at` and saves it, one edit at a time.
pub fn edit_saved(at: &Path, home: &Path, change: &RepoEdit) -> Result<Config> {
    let _one = EDITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut config = Config::load(at);
    edit(&mut config, home, change)?;
    config.save(at)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("valk-repos-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir(valkyrie_proto::canonical(&dir).unwrap())
    }

    /// A main checkout, as `git init` leaves one (enough for `project`).
    fn repo(path: &Path) {
        std::fs::create_dir_all(path.join(".git")).unwrap();
        std::fs::write(path.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    }

    /// A linked worktree of `main`, as `git worktree add` leaves one.
    fn worktree(main: &Path, path: &Path, name: &str) {
        let private = main.join(".git/worktrees").join(name);
        std::fs::create_dir_all(&private).unwrap();
        std::fs::write(private.join("commondir"), "../..\n").unwrap();
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(
            path.join(".git"),
            format!("gitdir: {}\n", private.display()),
        )
        .unwrap();
    }

    fn labels(list: &RepoList) -> Vec<&str> {
        let mut out: Vec<&str> = list.repos.iter().map(|r| r.label.as_str()).collect();
        out.sort();
        out
    }

    #[test]
    fn worktrees_and_sessions_resolve_to_their_main_checkout() {
        let home = tmp("resolve");
        let home = &home.0;
        repo(&home.join("repos/app"));
        // A worktree named nothing like `<repo>-<branch>`, outside the folders.
        worktree(
            &home.join("repos/app"),
            &home.join("elsewhere/odd-name"),
            "odd",
        );
        // One beside the repo, inside a scanned folder: not a repository of its own.
        worktree(
            &home.join("repos/app"),
            &home.join("repos/app-feat"),
            "feat",
        );
        repo(&home.join("far/away/lib"));
        std::fs::create_dir_all(home.join("plain")).unwrap();
        let sessions = [
            home.join("elsewhere/odd-name"),
            home.join("far/away/lib/src"),
            home.join("plain"),
        ];
        let list = known(&Config::default(), home, &sessions, &[]);
        assert_eq!(labels(&list), ["~/far/away/lib", "~/repos/app"]);
        assert_eq!(list.folders.len(), 1, "only the default folders that exist");
    }

    #[test]
    #[cfg(unix)]
    fn compares_through_a_symlink() {
        let home = tmp("symlink");
        let home = &home.0;
        repo(&home.join("real/app"));
        std::os::unix::fs::symlink(home.join("real"), home.join("link")).unwrap();
        let mut config = Config::default();
        edit(
            &mut config,
            home,
            &RepoEdit::Add {
                path: home.join("link/app"),
            },
        )
        .unwrap();
        // Found again through the real path: one repository, added by hand.
        let list = known(
            &config,
            home,
            &[home.join("real/app")],
            &[home.join("link/app")],
        );
        assert_eq!(list.repos.len(), 1);
        assert!(list.repos[0].added);
        assert_eq!(list.repos[0].path, home.join("real/app"));
        // Hidden through the link, gone through the real path.
        edit(
            &mut config,
            home,
            &RepoEdit::Hide {
                path: "~/link/app".into(),
            },
        )
        .unwrap();
        assert!(
            known(&config, home, &[home.join("real/app")], &[])
                .repos
                .is_empty()
        );
        edit(
            &mut config,
            home,
            &RepoEdit::Unhide {
                path: home.join("link/app"),
            },
        )
        .unwrap();
        assert_eq!(
            known(&config, home, &[home.join("real/app")], &[])
                .repos
                .len(),
            1
        );
    }

    #[test]
    fn scan_folders_are_yours_to_change() {
        let home = tmp("scan");
        let home = &home.0;
        repo(&home.join("repos/a"));
        repo(&home.join("code/b"));
        repo(&home.join("work-stuff/c"));
        repo(&home.join("repos/.hidden"));
        std::fs::create_dir_all(home.join("repos/stray/.git")).unwrap();
        let mut config = Config::default();
        assert_eq!(
            labels(&known(&config, home, &[], &[])),
            ["~/code/b", "~/repos/a"]
        );

        edit(
            &mut config,
            home,
            &RepoEdit::Scan {
                path: "~/work-stuff".into(),
            },
        )
        .unwrap();
        edit(
            &mut config,
            home,
            &RepoEdit::Unscan {
                path: "~/code".into(),
            },
        )
        .unwrap();
        let list = known(&config, home, &[], &[]);
        assert_eq!(labels(&list), ["~/repos/a", "~/work-stuff/c"]);
        let folders: Vec<&str> = list.folders.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(folders, ["~/repos", "~/work-stuff"]);

        assert!(
            edit(
                &mut config,
                home,
                &RepoEdit::Scan {
                    path: "~/nope".into()
                }
            )
            .is_err()
        );
        assert!(
            edit(
                &mut config,
                home,
                &RepoEdit::Add {
                    path: "~/work-stuff".into()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn backlog_folders_count_as_they_are() {
        let home = tmp("backlog");
        let home = &home.0;
        std::fs::create_dir_all(home.join("notes")).unwrap();
        let list = known(&Config::default(), home, &[home.join("notes")], &[]);
        assert!(
            list.repos.is_empty(),
            "a session outside git isn't a repository"
        );
        let list = known(
            &Config::default(),
            home,
            &[],
            &[home.join("notes"), home.join("gone")],
        );
        assert_eq!(labels(&list), ["~/notes"]);
    }

    #[test]
    fn saves_and_loads() {
        let home = tmp("save");
        let home = &home.0;
        repo(&home.join("x/app"));
        let at = home.join("cfg/repos.json");
        edit_saved(
            &at,
            home,
            &RepoEdit::Add {
                path: home.join("x/app"),
            },
        )
        .unwrap();
        let config = Config::load(&at);
        assert_eq!(config.added, [home.join("x/app")]);
        assert_eq!(config.folders, None);
        std::fs::write(&at, "not json").unwrap();
        assert_eq!(Config::load(&at), Config::default());
    }
}

//! The repositories every picker offers (backlog #240, protocol 49): one list the
//! daemon keeps, found from sessions, backlog folders and a scan of parent
//! folders, which the user can add to and hide from.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One repository a picker offers: a main checkout's root, never a worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownRepo {
    pub path: PathBuf,
    /// `~/repos/valkyrie`, as it reads.
    pub label: String,
    /// Added by hand, rather than found.
    #[serde(default)]
    pub added: bool,
}

/// A folder whose repositories (one level down) are offered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanFolder {
    pub path: PathBuf,
    pub label: String,
}

/// What `Repos` replies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoList {
    /// Most recently worked in first.
    pub repos: Vec<KnownRepo>,
    /// The folders scanned.
    pub folders: Vec<ScanFolder>,
    /// Repositories the user hid, to offer back.
    #[serde(default)]
    pub hidden: Vec<ScanFolder>,
}

/// A change to the list. Paths may start with `~`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum RepoEdit {
    /// Offers this repository (a worktree adds its main checkout), and unhides it.
    Add { path: PathBuf },
    /// Stops offering this repository, however it was found.
    Hide { path: PathBuf },
    /// Offers a hidden repository again.
    Unhide { path: PathBuf },
    /// Scans this folder for repositories too.
    Scan { path: PathBuf },
    /// Stops scanning this folder, a default one included.
    Unscan { path: PathBuf },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientMsg;

    #[test]
    fn edits_read_as_the_web_app_writes_them() {
        let msg: ClientMsg =
            serde_json::from_str(r#"{"t":"repo_edit","req":3,"edit":{"do":"add","path":"~/x"}}"#)
                .unwrap();
        assert_eq!(
            msg,
            ClientMsg::RepoEdit {
                req: 3,
                edit: RepoEdit::Add { path: "~/x".into() },
            }
        );
        let list: RepoList =
            serde_json::from_str(r#"{"repos":[{"path":"/r/a","label":"~/a"}],"folders":[]}"#)
                .unwrap();
        assert!(!list.repos[0].added && list.hidden.is_empty());
    }
}

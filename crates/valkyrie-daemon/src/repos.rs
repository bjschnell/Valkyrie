//! The repositories every picker offers (backlog #240): the launcher's, the
//! backlog's and the wiki's, in the web app and the TUI alike.

use crate::Registry;
use anyhow::Result;
use std::path::PathBuf;
use valkyrie_context::repos;
use valkyrie_proto::{Reply, RepoEdit, RepoList};

impl Registry {
    /// The list as it is now. Reads the disk: call it off a connection's thread.
    pub(crate) fn repos(&self) -> Reply {
        Reply::Repos {
            list: self.repo_list(&repos::Config::load(&repos::path())),
        }
    }

    /// Changes the list and replies it as it is after.
    pub(crate) fn repo_edit(&self, edit: &RepoEdit) -> Result<Reply> {
        let home = valkyrie_proto::home_dir();
        let config = repos::edit_saved(&repos::path(), &home, edit)?;
        Ok(Reply::Repos {
            list: self.repo_list(&config),
        })
    }

    fn repo_list(&self, config: &repos::Config) -> RepoList {
        let sessions: Vec<PathBuf> = self.listed().iter().map(|s| s.info().cwd).collect();
        let mut folders: Vec<PathBuf> = self
            .backlog
            .lock()
            .unwrap()
            .load()
            .into_iter()
            .filter_map(|i| i.project)
            .collect();
        folders.sort();
        folders.dedup();
        repos::known(config, &valkyrie_proto::home_dir(), &sessions, &folders)
    }
}

//! A session's markdown and text files, beside it (ADR-0021 item 9). The checks
//! are `valkyrie_context::files::check`; this is who may ask, and for which
//! session's repository.

use crate::Registry;
use crate::context::Peer;
use anyhow::{Result, bail};
use std::path::PathBuf;
use valkyrie_context::files::{self, Checked};
use valkyrie_proto::{FileMention, Reply, SessionFile, SessionId};

fn wire(c: Checked) -> SessionFile {
    SessionFile {
        path: c.rel,
        size: c.size,
        modified: c.modified,
    }
}

impl Registry {
    /// Session `id`'s working directory, for a caller that is the user or that
    /// session's own process; another agent is refused.
    fn files_cwd(&self, id: SessionId, peer: Option<Peer>) -> Result<PathBuf> {
        let session = self.get(id)?;
        let caller = self.caller(peer);
        if let Some(agent) = &caller.agent
            && caller.session.as_ref().map(|s| s.info().id) != Some(id)
        {
            bail!("{agent} can read only its own session's files");
        }
        Ok(session.info().cwd)
    }

    pub(crate) async fn session_files(
        &self,
        id: SessionId,
        check: Vec<String>,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        let cwd = self.files_cwd(id, peer)?;
        tokio::task::spawn_blocking(move || {
            // Not in a repository, or a top level too wide: nothing to list.
            let repo = match files::Repo::open(&cwd) {
                Ok(repo) => repo,
                Err(e) => {
                    tracing::debug!(id, "session files: {e:#}");
                    return Ok(Reply::SessionFiles {
                        files: Vec::new(),
                        passing: Vec::new(),
                    });
                }
            };
            // Mentions are checked alone: the list is its own request.
            if !check.is_empty() {
                let passing = files::passing(&repo, &check)
                    .into_iter()
                    .map(|(given, path)| FileMention { given, path })
                    .collect();
                return Ok(Reply::SessionFiles {
                    files: Vec::new(),
                    passing,
                });
            }
            let default = crate::worktree::default_branch(&repo.top);
            let listed = files::list(&repo, default.as_deref());
            Ok(Reply::SessionFiles {
                files: listed.into_iter().map(wire).collect(),
                passing: Vec::new(),
            })
        })
        .await?
    }

    pub(crate) async fn read_session_file(
        &self,
        id: SessionId,
        path: String,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        let cwd = self.files_cwd(id, peer)?;
        tokio::task::spawn_blocking(move || {
            let repo = files::Repo::open(&cwd)?;
            let (checked, text) = files::read(&repo, &path)?;
            Ok(Reply::SessionFileText {
                file: wire(checked),
                text,
            })
        })
        .await?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{AS_CALLER, Caller};
    use crate::session::Session;
    use std::sync::Arc;
    use valkyrie_proto::{Size, SpawnSpec};

    fn as_caller(agent: Option<&str>, session: Option<Arc<Session>>) {
        let agent = agent.map(str::to_string);
        AS_CALLER.with(|c| {
            *c.borrow_mut() = Some(Box::new(move || Caller {
                agent: agent.clone(),
                session: session.clone(),
                bridge: false,
                desktop: false,
            }))
        });
    }

    #[tokio::test]
    async fn only_the_user_or_the_session_itself_reads_its_files() {
        let dir = std::env::temp_dir().join(format!("valkyrie-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                    .args(args)
                    .current_dir(&repo)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            )
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("README.md"), "# hello\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "one"]);
        std::fs::write(repo.join("NOTES.md"), "new\n").unwrap();
        let (registry, _) = crate::new_registry(
            dir.join("v.sock"),
            &dir.join("state"),
            std::path::Path::new("/bin/true"),
            1,
            0,
            7,
            false,
        )
        .unwrap();
        let spawn = || {
            let spec = SpawnSpec {
                command: vec!["sleep".into(), "30".into()],
                cwd: Some(repo.clone()),
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
        let (mine, other) = (spawn(), spawn());
        let id = mine.info().id;

        as_caller(None, None);
        let Reply::SessionFiles { files, .. } =
            registry.session_files(id, Vec::new(), None).await.unwrap()
        else {
            panic!("no files");
        };
        assert_eq!(
            files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["NOTES.md"]
        );
        let Reply::SessionFiles { passing, .. } = registry
            .session_files(id, vec!["README.md".into(), "../x.md".into()], None)
            .await
            .unwrap()
        else {
            panic!("no files");
        };
        assert_eq!(passing.len(), 1);
        assert_eq!(passing[0].path, "README.md");

        as_caller(Some("claude"), Some(mine.clone()));
        let Reply::SessionFileText { text, .. } = registry
            .read_session_file(id, "README.md".into(), None)
            .await
            .unwrap()
        else {
            panic!("no text");
        };
        assert_eq!(text, "# hello\n");

        as_caller(Some("claude"), Some(other.clone()));
        let e = registry
            .read_session_file(id, "README.md".into(), None)
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("only its own"), "{e}");
        assert!(registry.session_files(id, Vec::new(), None).await.is_err());

        as_caller(Some("claude"), None);
        assert!(
            registry
                .read_session_file(id, "README.md".into(), None)
                .await
                .is_err()
        );

        AS_CALLER.with(|c| *c.borrow_mut() = None);
        mine.kill();
        other.kill();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

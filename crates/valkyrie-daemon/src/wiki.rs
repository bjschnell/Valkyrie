//! The repository wiki for the TUI and the web app (backlog #210): list and read
//! pages, and start the agent that writes them, as `valk wiki explore` does.

use crate::{Registry, backlog::agent_command};
use anyhow::{Result, bail};
use std::path::Path;
use std::sync::Arc;
use valkyrie_context::wiki;
use valkyrie_proto::{Reply, Size, SpawnSpec, WikiListing};

/// What an exploration's session is called.
const NAME: &str = "wiki";

/// The last brief an exploration was started with, for the tests.
#[cfg(test)]
static BRIEFED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

impl Registry {
    /// The wiki of the repository holding `cwd`, each page's staleness worked
    /// out. Runs `git`: call it off the connection's thread.
    pub(crate) fn wiki(&self, cwd: &Path) -> Reply {
        let root = valkyrie_context::project::root(cwd);
        let pages = self.context.lock().unwrap().wiki(&root);
        // After the lock: one `git diff` per commit the pages were written at.
        let stale = wiki::staleness(&root, &pages);
        Reply::Wiki {
            listing: WikiListing {
                pages: pages
                    .iter()
                    .map(|p| wiki::wire(p, stale.get(&p.slug), false))
                    .collect(),
                root,
            },
        }
    }

    /// One page in full.
    pub(crate) fn wiki_page(&self, cwd: &Path, page: &str) -> Result<Reply> {
        let root = valkyrie_context::project::root(cwd);
        let Some(p) = self.context.lock().unwrap().wiki_page(&root, page) else {
            bail!("no wiki page {page:?}");
        };
        let stale = wiki::staleness(&root, std::slice::from_ref(&p));
        Ok(Reply::WikiPage {
            page: Box::new(wiki::wire(&p, stale.get(&p.slug), true)),
        })
    }

    /// Starts `agent` in the repository's root, briefed to write its wiki. One
    /// already doing that there is answered with itself, so a second click
    /// doesn't start a second agent.
    pub(crate) async fn wiki_explore(
        self: &Arc<Self>,
        cwd: &Path,
        stale: bool,
        agent: &str,
        size: Size,
        env: Vec<(String, Option<String>)>,
    ) -> Result<(Reply, bool)> {
        if !matches!(agent, "claude" | "codex") {
            bail!("the wiki is written by claude or codex, not {agent}");
        }
        let root = valkyrie_context::project::root(cwd);
        if let Some(live) = self
            .all()
            .into_iter()
            .find(|s| !s.exited() && s.info().name == NAME && s.info().cwd == root)
        {
            let info = Box::new(live.info());
            return Ok((Reply::Session { info }, false));
        }
        let pages = self.context.lock().unwrap().wiki(&root);
        let valk = valkyrie_context::valk_command();
        let brief = {
            let root = root.clone();
            tokio::task::spawn_blocking(move || wiki::explore_brief(&root, &pages, stale, &valk))
                .await??
        };
        #[cfg(test)]
        BRIEFED.lock().unwrap().replace(brief.clone());
        // A test starts `sleep`, not an agent: a stand-in of its own, so the
        // dispatch test's can't race it.
        let command = match cfg!(test) {
            true => vec!["sleep".to_owned(), "30".to_owned()],
            false => agent_command(agent, "", brief),
        };
        let spec = SpawnSpec {
            command,
            cwd: Some(root),
            name: Some(NAME.into()),
            size,
            env,
            worktree: None,
            hidden: false,
        };
        let registry = Arc::clone(self);
        let reply = crate::in_worktree(spec, move |spec| crate::spawn(&registry, spec)).await?;
        Ok((reply, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// The TUI's and the web app's way in (backlog #210): list, read, and start
    /// the agent, once per repository at a time.
    #[tokio::test]
    async fn the_apps_list_read_and_start_the_wiki_agent() {
        let dir = std::env::temp_dir().join(format!("vd-wiki-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/main.rs"), "fn main() {}\n").unwrap();
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "start"]);
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
        let size = Size { cols: 80, rows: 24 };

        let Reply::Wiki { listing } = registry.wiki(&repo.join("src")) else {
            panic!("not a listing");
        };
        assert_eq!(listing.root, valkyrie_context::project::root(&repo));
        assert!(listing.pages.is_empty());
        // Nothing to refresh, and only claude or codex write it.
        let e = registry
            .wiki_explore(&repo, true, "claude", size, vec![])
            .await
            .unwrap_err();
        assert!(e.to_string().contains("no page is stale"), "{e}");
        assert!(
            registry
                .wiki_explore(&repo, false, "bash", size, vec![])
                .await
                .is_err()
        );

        let (reply, started) = registry
            .wiki_explore(&repo.join("src"), false, "claude", size, vec![])
            .await
            .unwrap();
        let Reply::Session { info } = reply else {
            panic!("not a session");
        };
        assert!(started);
        assert_eq!((info.name.as_str(), &info.cwd), ("wiki", &listing.root));
        let brief = BRIEFED.lock().unwrap().clone().unwrap();
        assert!(brief.starts_with("Explore this repository"), "{brief}");
        // A second press while it runs: the same session, not another agent.
        let (again, started) = registry
            .wiki_explore(&repo, false, "claude", size, vec![])
            .await
            .unwrap();
        let Reply::Session { info: same } = again else {
            panic!("not a session");
        };
        assert!(!started);
        assert_eq!(same.id, info.id);

        // A page the agent wrote, listed without its body and read with it.
        let new = valkyrie_context::wiki::NewPage {
            slug: "overview".into(),
            title: "Overview".into(),
            summary: "What the parts are".into(),
            scope: vec!["src".into()],
            body: "Two parts.".into(),
        };
        registry
            .context
            .lock()
            .unwrap()
            .write_wiki(&listing.root, &new, None, 1)
            .unwrap();
        let Reply::Wiki { listing } = registry.wiki(&repo) else {
            panic!("not a listing");
        };
        assert_eq!(listing.pages.len(), 1);
        assert_eq!(listing.pages[0].body, "");
        let Reply::WikiPage { page } = registry.wiki_page(&repo, "overview").unwrap() else {
            panic!("not a page");
        };
        assert_eq!(page.body, "Two parts.");
        assert!(registry.wiki_page(&repo, "../etc").is_err());

        for s in registry.all() {
            s.kill();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

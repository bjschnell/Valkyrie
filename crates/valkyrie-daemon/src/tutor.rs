//! The tutor (backlog #226): the agent `valk tutor`, the TUI's `?` and the web
//! app start to teach Valkyrie, one lesson at a time.

use crate::{Registry, backlog::agent_command};
use anyhow::{Result, bail};
use std::sync::Arc;
use valkyrie_context::tutor::{self, NAME};
use valkyrie_proto::{Reply, Size, SpawnSpec};

impl Registry {
    /// Starts `agent` as the tutor in the home directory. One still running is
    /// answered with itself, so a second press opens the lesson in progress.
    /// The `bool` is whether it started one.
    pub(crate) async fn tutor(
        self: &Arc<Self>,
        agent: &str,
        size: Size,
        env: Vec<(String, Option<String>)>,
    ) -> Result<(Reply, bool)> {
        if !matches!(agent, "claude" | "codex") {
            bail!("the tutor is claude or codex, not {agent}");
        }
        if let Some(live) = self
            .all()
            .into_iter()
            .find(|s| !s.exited() && s.info().name == NAME)
        {
            let info = Box::new(live.info());
            return Ok((Reply::Session { info }, false));
        }
        let brief = tutor::brief(&valkyrie_context::valk_command());
        // A test starts `sleep`, not an agent.
        let command = match cfg!(test) {
            true => vec!["sleep".to_owned(), "30".to_owned()],
            false => agent_command(agent, "", brief),
        };
        let spec = SpawnSpec {
            command,
            cwd: Some(valkyrie_proto::home_dir()),
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
    use std::path::Path;
    use valkyrie_proto::{Reply, Size};

    /// One tutor at a time: a second press opens the running one.
    #[tokio::test]
    async fn the_tutor_starts_once() {
        let dir = std::env::temp_dir().join(format!("vd-tutor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
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

        assert!(registry.tutor("bash", size, vec![]).await.is_err());
        let (reply, started) = registry.tutor("claude", size, vec![]).await.unwrap();
        let Reply::Session { info } = reply else {
            panic!("not a session");
        };
        assert!(started);
        assert_eq!(info.name, "tutor");
        assert_eq!(info.cwd, valkyrie_proto::home_dir());

        let (again, started) = registry.tutor("claude", size, vec![]).await.unwrap();
        let Reply::Session { info: same } = again else {
            panic!("not a session");
        };
        assert!(!started, "a second tutor started");
        assert_eq!(same.id, info.id);

        registry.get(info.id).unwrap().kill();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

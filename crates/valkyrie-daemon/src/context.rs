//! Project decisions (ADR-0007): the daemon is their only writer, and it decides who
//! is asking. A connection from inside a session running an agent can propose but
//! never accept, whatever environment it claims.

use crate::session::Session;
use crate::{Registry, foreground};
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use valkyrie_proto::{NewDecision, Provenance, Reply, ReviewAction};

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Whether a session's program is an agent rather than a shell or other tool. It
/// follows the foreground, so `claude` typed at a shell prompt counts.
fn runs_agent(session: &Session) -> Option<String> {
    let agent = session.info().status.agent;
    (agent != "generic").then_some(agent)
}

impl Registry {
    /// The session whose process tree holds `peer` (a connecting client's pid).
    fn session_holding(&self, peer: i32) -> Option<Arc<Session>> {
        let by_pid: HashMap<i32, Arc<Session>> = self
            .all()
            .into_iter()
            .filter_map(|s| Some((s.pid()? as i32, s)))
            .collect();
        foreground::ancestry(peer)
            .into_iter()
            .find_map(|pid| by_pid.get(&pid).cloned())
    }

    /// The session a request comes from: found from the peer's pid when the OS
    /// tells it, else the one the client named.
    fn caller(&self, peer: Option<i32>, claimed: Option<u32>) -> Option<Arc<Session>> {
        match peer {
            Some(pid) => self.session_holding(pid),
            None => claimed.and_then(|id| self.get(id).ok()),
        }
    }

    pub(crate) fn refresh_proposals(&self) {
        let items = self.context.lock().unwrap().proposals();
        self.proposals.send_if_modified(|current| {
            let changed = **current != items;
            if changed {
                *current = Arc::new(items);
            }
            changed
        });
    }

    pub(crate) fn decide(&self, new: NewDecision, peer: Option<i32>) -> Result<Reply> {
        let mut provenance = Provenance {
            by: "human".into(),
            commit: new.commit.clone(),
            cwd: Some(new.cwd.clone()),
            ..Provenance::default()
        };
        if let Some(session) = self.caller(peer, new.session) {
            provenance.session = Some(session.info().name);
            if let Some(agent) = runs_agent(&session) {
                provenance.by = agent;
                provenance.conversation = session.conversation();
            }
        }
        let decision = self
            .context
            .lock()
            .unwrap()
            .decide(&new, provenance, now_secs())?;
        tracing::info!(
            id = decision.id,
            project = %decision.project.display(),
            by = decision.provenance.by,
            status = decision.status.as_str(),
            "decision recorded"
        );
        self.refresh_proposals();
        Ok(Reply::Decision { decision })
    }

    pub(crate) fn review(
        &self,
        project: &Path,
        id: u32,
        action: &ReviewAction,
        peer: Option<i32>,
    ) -> Result<Reply> {
        if let Some(session) = self.caller(peer, None)
            && let Some(agent) = runs_agent(&session)
        {
            bail!("{agent} can't review decisions; the user accepts or rejects them in Valkyrie");
        }
        let decision = self
            .context
            .lock()
            .unwrap()
            .review(project, id, action, now_secs())?;
        tracing::info!(
            id,
            project = %project.display(),
            status = decision.status.as_str(),
            "decision reviewed"
        );
        self.refresh_proposals();
        Ok(Reply::Decision { decision })
    }

    /// The decisions of the project holding `cwd`, or of every project.
    pub(crate) fn decisions(&self, cwd: Option<PathBuf>) -> Reply {
        let store = self.context.lock().unwrap();
        let decisions = match cwd {
            Some(cwd) => store.load(&valkyrie_context::project::root(&cwd)),
            None => store
                .projects()
                .iter()
                .flat_map(|root| store.load(root))
                .collect(),
        };
        Reply::Decisions { decisions }
    }
}

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

/// A connecting process, pinned by its start time so a reused pid can't stand in
/// for it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Peer {
    pid: i32,
    started: Option<u64>,
}

impl Peer {
    pub(crate) fn of(stream: &tokio::net::UnixStream) -> Option<Peer> {
        let pid = stream.peer_cred().ok()?.pid()?;
        Some(Peer {
            pid,
            started: foreground::started(pid),
        })
    }
}

/// Who is asking: a human, or an agent (or what may be one).
#[derive(Clone)]
pub(crate) struct Caller {
    /// `None`: a human.
    pub(crate) agent: Option<String>,
    pub(crate) session: Option<Arc<Session>>,
}

impl Caller {
    fn agent(name: &str) -> Caller {
        Caller {
            agent: Some(name.into()),
            session: None,
        }
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Registry {
    /// Works out who `peer` is (ADR-0007 §4), failing closed: anything it can't
    /// show is a human's counts as an agent's. A human is a process outside every
    /// agent, either in a session no agent runs, started or typed into, or on a
    /// terminal of its own, or the `valk web` bridge (whose devices a human paired).
    pub(crate) fn caller(&self, peer: Option<Peer>) -> Caller {
        let Some(peer) = peer.filter(|p| p.pid > 0) else {
            return Caller::agent("unknown");
        };
        // Gone, or its pid reused since it connected (a fork whose parent left).
        if peer.started.is_none() || foreground::started(peer.pid) != peer.started {
            return Caller::agent("unknown");
        }
        let sessions: Vec<Arc<Session>> = self.all().into_iter().filter(|s| !s.exited()).collect();
        let by_pid: HashMap<i32, &Arc<Session>> = sessions
            .iter()
            .filter_map(|s| Some((s.pid()? as i32, s)))
            .collect();
        let mut seen_agent = None;
        let mut session = None;
        for pid in foreground::ancestry(peer.pid) {
            if seen_agent.is_none() {
                seen_agent = foreground::agent_running(pid).map(str::to_owned);
            }
            if let Some(s) = by_pid.get(&pid) {
                session = Some(Arc::clone(s));
                break;
            }
        }
        // A process that left its parents (`(valk decide &)`, nohup, disown) keeps
        // its controlling terminal.
        let tty = foreground::tty(peer.pid);
        if session.is_none()
            && let Some(tty) = tty
        {
            session = sessions.iter().find(|s| s.tty() == Some(tty)).cloned();
        }
        let agent = match &session {
            Some(s) => seen_agent.or_else(|| s.agent_behind()),
            None if seen_agent.is_some() => seen_agent,
            // A terminal of its own: someone at a keyboard.
            None if tty.is_some() => None,
            None if foreground::is_web_bridge(peer.pid) => None,
            // No terminal and no session: daemonized, so whoever started it is unknown.
            None => Some("detached".into()),
        };
        Caller { agent, session }
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

    pub(crate) fn decide(&self, new: NewDecision, peer: Option<Peer>) -> Result<Reply> {
        let caller = self.caller(peer);
        let provenance = Provenance {
            by: caller.agent.clone().unwrap_or_else(|| "human".into()),
            session: caller.session.as_ref().map(|s| s.info().name),
            conversation: caller.session.as_ref().and_then(|s| s.conversation()),
            commit: new.commit.clone(),
            cwd: Some(new.cwd.clone()),
        };
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

    /// Only a human may act on decisions, or vouch for a new phone.
    pub(crate) fn vouch(&self, peer: Option<Peer>, what: &str) -> Result<()> {
        if let Some(agent) = self.caller(peer).agent {
            bail!("{what} is for the user, not {agent}: run it from your own terminal");
        }
        Ok(())
    }

    pub(crate) fn review(
        &self,
        project: &Path,
        id: u32,
        action: &ReviewAction,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        self.vouch(peer, "reviewing decisions")?;
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

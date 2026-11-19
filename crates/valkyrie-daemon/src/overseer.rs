//! The supervisor session (ADR-0019 §10, backlog #236): one Claude session the
//! user starts on demand that reads every other agent session and prompts the
//! idle ones. The daemon keeps it to that: there's never more than one, its
//! typing into another session is dropped unless that session's agent is idle or
//! done (or it's a pane of its own tab), nothing it types gets through while
//! `valk supervise off`, and each prompt it sends is logged on the target and in
//! the project's `supervisor-prompts.jsonl` for the supervisor page. On each of
//! its own prompts it hears the roster of live agent sessions.

use crate::Registry;
use crate::context::Peer;
use crate::session::Session;
use anyhow::{Result, bail};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Arc;
use valkyrie_proto::{AgentState, Reply, SessionId, Size, SpawnSpec};

/// The name its tab gets.
pub(crate) const NAME: &str = "supervisor";

/// What it's told it is, as its system prompt. Fixed: nothing the user or another
/// agent wrote goes in.
const ROLE: &str = "\
You are the Valkyrie supervisor session: the one agent that oversees every other \
agent session this Valkyrie daemon runs, across repositories, for the user.

What you can do:
- Read: `valk ls` lists sessions; `valk pane read <id> [--lines N]` prints one's \
screen; `valk digest` sums up what each did; `valk todo list` and `valk todo show N` \
read the backlog; `gh pr view` reads a pull request.
- Prompt an idle agent: `valk pane prompt <id> \"<text>\" --wait` types a prompt \
into another session and waits for its turn to end. Valkyrie only lets it through \
when that agent is idle or done, and while the supervisor is on (`valk supervise`). \
Each prompt you send is logged against you and shown to the user.

Rules:
- Never merge, ship, accept, reopen or drop anything, and never ask another agent \
to: those are the user's (Valkyrie refuses them from agents anyway).
- Never prompt a session that is working, blocked, or waiting on the user: what \
it waits on is the user's to answer. Say so and leave it.
- Prompt only to move work the user already set going: unstick an agent that \
stopped short, point it at a red or conflicting pull request, tell it what \
another agent changed. Don't hand out new work the user didn't ask for.
- Keep prompts short and specific, and say they come from the supervisor session.
- When something needs the user (a decision, an approval, anything risky), tell \
the user here instead of acting.

Each prompt you get carries the current roster of agent sessions; trust it over \
what you read earlier.";

/// Its first prompt.
const OPENING: &str = "\
Read the roster of agent sessions you were given and give me a short status: \
what each one is doing, which are stuck or idle with work left, and what you'd \
prompt each idle one with. Don't prompt anyone yet; wait for me.";

impl Registry {
    /// `ClientMsg::SupervisorSession`: the running supervisor session, else a new
    /// one started in `cwd`. Starting it is the user's.
    pub(crate) fn supervisor_session(
        &self,
        cwd: PathBuf,
        size: Size,
        env: Vec<(String, Option<String>)>,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        self.vouch(peer, "starting the supervisor session")?;
        // One at a time, so two starts can't both find none and each spawn one.
        let _one = self.overseer_start.lock().unwrap();
        if let Some(s) = self.overseer() {
            return Ok(Reply::Session {
                info: Box::new(s.info()),
            });
        }
        if !self.supervisor.lock().unwrap().on() {
            bail!("the supervisor is off: `valk supervise on` first");
        }
        let mut command = crate::backlog::agent_command("claude", "", OPENING.to_owned());
        if command.first().map(String::as_str) == Some("claude") {
            command.splice(1..1, ["--append-system-prompt".to_owned(), ROLE.to_owned()]);
        }
        let spec = SpawnSpec {
            command,
            // Empty: the daemon's own default, for a client on another machine.
            cwd: (!cwd.as_os_str().is_empty()).then_some(cwd),
            name: Some(NAME.to_owned()),
            size,
            env,
            worktree: None,
            hidden: false,
        };
        let reply = crate::spawn(self, spec)?;
        let Reply::Session { info } = &reply else {
            return Ok(reply);
        };
        let session = self.get(info.id)?;
        session.make_supervisor();
        tracing::info!(session = info.id, "started the supervisor session");
        Ok(Reply::Session {
            info: Box::new(session.info()),
        })
    }

    /// The supervisor session, if one runs.
    pub(crate) fn overseer(&self) -> Option<Arc<Session>> {
        self.listed()
            .into_iter()
            .find(|s| s.is_supervisor() && !s.exited())
    }

    /// Whether the supervisor session `from` may type into `to`; `Err` says why
    /// not. Its own session and the panes of its own tab are its to drive.
    pub(crate) fn overseer_may_type(&self, from: &Session, to: &Session) -> Result<(), String> {
        if from.id == to.id || self.same_tab(from.id, to.id) {
            return Ok(());
        }
        if !self.supervisor.lock().unwrap().on() {
            return Err("the supervisor is off".to_owned());
        }
        if to.agent_behind().is_none() {
            return Err("not an agent session".to_owned());
        }
        let state = to.info().status.state;
        match state {
            AgentState::Idle | AgentState::ReviewReady => Ok(()),
            other => Err(format!("its agent is {}", other.label())),
        }
    }

    fn same_tab(&self, a: SessionId, b: SessionId) -> bool {
        self.layouts()
            .iter()
            .any(|p| p.contains(a) && p.contains(b))
    }

    /// Records what the supervisor session typed into `to`: a `supervisor` line in
    /// its event log, and a line in its project's `supervisor-prompts.jsonl`.
    pub(crate) fn log_overseer_input(&self, to: &Session, data: &[u8]) {
        let Some(text) = said(data) else {
            return;
        };
        to.log_supervisor_prompt(&text);
        let info = to.info();
        let root = valkyrie_context::project::root(&info.cwd);
        let now = crate::session::now_ms() / 1000;
        let record = valkyrie_context::steps::Prompt {
            at: now,
            session: info.name,
            text,
        };
        if let Err(e) = self
            .context
            .lock()
            .unwrap()
            .record_prompt(&root, &record, now)
        {
            tracing::warn!("couldn't record a supervisor prompt: {e:#}");
        }
    }

    /// What the supervisor session hears on each prompt: every other live agent
    /// session, one line each.
    pub(crate) fn roster(&self, me: SessionId) -> String {
        let mut text = String::from(
            "Valkyrie supervisor roster (live agent sessions; prompt only idle or done ones):",
        );
        let mut any = false;
        for s in self.listed() {
            if s.id == me || s.exited() || s.is_supervisor() {
                continue;
            }
            let Some(agent) = s.agent_behind() else {
                continue;
            };
            any = true;
            let flag = self.flag_of(&s, s.id);
            let _ = write!(text, "\n{}", line(&s.info(), &agent, flag.as_ref()));
        }
        if !any {
            text.push_str("\n(none)");
        }
        text
    }
}

/// One roster line: `- #7 api (claude, idle) in ~/repos/api: fixing the login
/// test; flag: CI red on PR #12: lint (offered: ci-fix)`.
fn line(
    info: &valkyrie_proto::SessionInfo,
    agent: &str,
    flag: Option<&valkyrie_proto::Flag>,
) -> String {
    let mut out = format!(
        "- #{} {} ({agent}, {}) in {}",
        info.id,
        info.name,
        info.status.state.label(),
        info.cwd.display()
    );
    if let Some(summary) = info.status.summary.as_deref().filter(|s| !s.is_empty()) {
        let _ = write!(out, ": {}", one_line(summary, 120));
    }
    if let Some(report) = &info.report {
        let _ = write!(out, "; status: {report}");
    }
    if let Some(flag) = flag {
        let _ = write!(out, "; flag: {}", flag.reason);
        if let Some(step) = &flag.step {
            let _ = write!(out, " (offered: {})", step.action.as_str());
        }
    }
    if let Some(overlap) = &info.overlap {
        let _ = write!(out, "; {overlap}");
    }
    out
}

/// The text a chunk of typing carries, control keys dropped; `None` when it's
/// only keys (an Enter, an arrow).
fn said(data: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(data);
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let text = one_line(&text, 500);
    (!text.is_empty()).then_some(text)
}

fn one_line(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::context::{AS_CALLER, Caller};
    #[cfg(unix)]
    use std::path::Path;
    #[cfg(unix)]
    use std::process::Command;

    #[cfg(unix)]
    fn caller(agent: Option<&str>) {
        let agent = agent.map(str::to_owned);
        AS_CALLER.with(|c| {
            *c.borrow_mut() = Some(Box::new(move || Caller {
                agent: agent.clone(),
                session: None,
                bridge: false,
                desktop: false,
            }))
        });
    }

    // Spawns real sessions (`sleep`) and kills them, as the dispatch tests do.
    #[test]
    #[cfg(unix)]
    fn one_session_started_by_the_user_reads_and_prompts_only_idle_agents() {
        let dir = std::env::temp_dir().join(format!("valk-overseer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(dir.join("state")).unwrap();
        // Its own repository, whatever is above the temp dir (decision #10).
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&repo)
                .status()
                .unwrap()
                .success()
        );
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
        *crate::backlog::AGENT_STANDIN.lock().unwrap() = Some(vec!["sleep".into(), "30".into()]);
        let size = Size { cols: 40, rows: 10 };
        let start = || registry.supervisor_session(repo.clone(), size, Vec::new(), None);

        // An agent can't start it.
        caller(Some("claude"));
        assert!(start().is_err());
        caller(None);
        // Nor while the supervisor is off.
        registry.supervisor.lock().unwrap().set_on(false).unwrap();
        assert!(format!("{:#}", start().unwrap_err()).contains("supervise on"));
        registry.supervisor.lock().unwrap().set_on(true).unwrap();

        let Reply::Session { info } = start().unwrap() else {
            panic!("no session");
        };
        assert!(info.supervisor && info.name == NAME);
        // A second start finds the first.
        let Reply::Session { info: again } = start().unwrap() else {
            panic!("no session");
        };
        assert_eq!(again.id, info.id);
        let me = registry.get(info.id).unwrap();

        let spawn = |agent: bool| {
            let spec = SpawnSpec {
                command: vec!["sleep".into(), "30".into()],
                cwd: Some(repo.clone()),
                name: None,
                size,
                env: Vec::new(),
                worktree: None,
                hidden: false,
            };
            let Reply::Session { info } = crate::spawn(&registry, spec).unwrap() else {
                panic!("no session");
            };
            let s = registry.get(info.id).unwrap();
            if agent {
                s.driven_by("claude");
            }
            s
        };
        let (idle, busy, shell) = (spawn(true), spawn(true), spawn(false));
        busy.hook(
            "claude",
            1,
            serde_json::json!({"hook_event_name": "UserPromptSubmit", "prompt": "more"}),
        );

        // The roster names every agent but itself, with its state.
        let roster = registry.roster(me.id);
        assert!(roster.contains(&format!("#{} ", idle.id)), "{roster}");
        assert!(roster.contains(&format!("#{} ", busy.id)), "{roster}");
        assert!(roster.contains("working"), "{roster}");
        assert!(!roster.contains(&format!("#{} ", shell.id)), "{roster}");
        assert!(!roster.contains(&format!("#{} ", me.id)), "{roster}");
        // It hears it on its prompts; nobody else does.
        let Reply::Text { text } = registry.siblings(me.id).unwrap() else {
            panic!("no text");
        };
        assert!(text.starts_with("Valkyrie supervisor roster"), "{text}");
        let Reply::Text { text } = registry.siblings(idle.id).unwrap() else {
            panic!("no text");
        };
        assert!(!text.contains("supervisor roster"), "{text}");

        // It may type into an idle agent, not a busy one or a human's shell.
        assert!(registry.overseer_may_type(&me, &idle).is_ok());
        assert!(registry.overseer_may_type(&me, &me).is_ok());
        let why = registry.overseer_may_type(&me, &busy).unwrap_err();
        assert!(why.contains("working"), "{why}");
        assert!(registry.overseer_may_type(&me, &shell).is_err());
        // Nor anywhere while the supervisor is off.
        registry.supervisor.lock().unwrap().set_on(false).unwrap();
        assert!(registry.overseer_may_type(&me, &idle).is_err());
        registry.supervisor.lock().unwrap().set_on(true).unwrap();

        // What it typed is on the supervisor page, and only text counts.
        registry.log_overseer_input(&idle, b"From the supervisor: merge main in\r");
        registry.log_overseer_input(&idle, b"\r");
        let Reply::SupervisorLog { entries, .. } = registry.supervisor_log().unwrap() else {
            panic!("no log");
        };
        let said: Vec<&str> = entries.iter().map(|e| e.said.as_str()).collect();
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].contains("merge main in"), "{said:?}");

        // Once it's gone, a start makes a new one.
        me.force_kill();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !me.exited() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let Reply::Session { info: fresh } = start().unwrap() else {
            panic!("no session");
        };
        assert_ne!(fresh.id, info.id);
        AS_CALLER.with(|c| *c.borrow_mut() = None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn said_drops_keys_and_keeps_text() {
        assert_eq!(said(b"\r"), None);
        assert_eq!(said(b"\x1b[A"), Some("[A".to_owned()));
        assert_eq!(
            said(b"From the supervisor: merge main in\r"),
            Some("From the supervisor: merge main in".to_owned())
        );
    }

    #[test]
    fn one_line_cuts_long_text() {
        let long = "word ".repeat(200);
        let cut = one_line(&long, 20);
        assert_eq!(cut.chars().count(), 21);
        assert!(cut.ends_with('…'));
    }
}

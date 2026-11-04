//! Learning a repository's facts from what runs in it (ADR-0016 §3). Each session
//! watches its agent's tool calls: the checks it runs, the pushes that go through,
//! and CI looked up with its own tools after a push. What it sees is recorded in
//! the project's store, and once seen in two sessions, proposed for the user to
//! accept. Counting only: no model reads anything here.

use crate::Registry;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use valkyrie_context::{facts, project};
use valkyrie_proto::{DecisionKind, NewDecision, Provenance, SessionId};

/// What a session has seen since its last push.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    /// Checks run, in order, each once.
    ran: Vec<String>,
    /// A push went through and CI hasn't been looked at since.
    pushed: bool,
    /// The last stop was clean with a pull request: asking about CI now counts.
    clean_pr: bool,
}

impl Watch {
    /// Takes a hook payload from the session's own agent; returns what it saw, with
    /// the directory it was seen in.
    pub(crate) fn hook(&mut self, payload: &Value) -> Option<(PathBuf, facts::Seen)> {
        let cwd = PathBuf::from(payload["cwd"].as_str()?);
        match payload["hook_event_name"].as_str()? {
            "PostToolUse" => {
                let tool = payload["tool_name"].as_str().unwrap_or_default();
                let input = &payload["tool_input"];
                if let Some(command) = (!tool.starts_with("mcp__"))
                    .then(|| facts::shell_command(input))
                    .flatten()
                {
                    for check in facts::checks_in(&command, &project::checkout(&cwd)) {
                        if !self.ran.contains(&check) && self.ran.len() < facts::MAX_RAN {
                            self.ran.push(check);
                        }
                    }
                    if facts::is_push(&command) {
                        self.pushed = true;
                        let checks = std::mem::take(&mut self.ran);
                        return Some((cwd, facts::Seen::Push { checks }));
                    }
                }
                if self.pushed && facts::is_ci_check(tool, input) {
                    self.pushed = false;
                    return Some((
                        cwd,
                        facts::Seen::Ci {
                            how: how(tool, input),
                        },
                    ));
                }
                None
            }
            "UserPromptSubmit" => {
                let asked = std::mem::take(&mut self.clean_pr);
                let prompt = payload["prompt"].as_str()?;
                (asked && asks_about_ci(prompt)).then(|| {
                    let quote = valkyrie_context::reply::quote(prompt);
                    (
                        cwd,
                        facts::Seen::Ci {
                            how: format!("you asked: “{quote}”"),
                        },
                    )
                })
            }
            _ => None,
        }
    }

    /// Where a stop left the work: after a clean stop with a pull request, the
    /// user asking about CI counts as checking it.
    pub(crate) fn stopped(&mut self, state: &str, pr: bool) {
        self.clean_pr = state == "clean" && pr;
    }
}

/// How the agent checked CI, for the evidence: the MCP tool, or the host it asked.
fn how(tool: &str, input: &Value) -> String {
    if tool.starts_with("mcp__") {
        return format!("`{tool}`");
    }
    let command = facts::shell_command(input).unwrap_or_default();
    let host = command
        .split_whitespace()
        .find_map(|w| {
            let w = w.trim_matches(|c| c == '"' || c == '\'');
            let rest = w.split_once("://")?.1;
            Some(rest.split(['/', '?', '@']).next()?.to_owned())
        })
        .unwrap_or_default();
    let program = command.split_whitespace().next().unwrap_or_default();
    format!("`{program}` to {host}")
        .trim_end_matches(" to ")
        .to_owned()
}

/// Whether a prompt asks about CI: "is CI green?", "did the build pass".
fn asks_about_ci(prompt: &str) -> bool {
    let text = prompt.to_ascii_lowercase();
    let words: Vec<&str> = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let about = words.iter().any(|w| {
        matches!(
            *w,
            "ci" | "build" | "builds" | "pipeline" | "bamboo" | "jenkins"
        )
    });
    let state = words.iter().any(|w| {
        matches!(
            *w,
            "green"
                | "red"
                | "pass"
                | "passed"
                | "passing"
                | "fail"
                | "failed"
                | "failing"
                | "status"
                | "check"
                | "broke"
                | "broken"
        )
    });
    prompt.chars().count() <= valkyrie_context::reply::MAX_REPLY && about && state
}

pub(crate) async fn run(
    registry: Arc<Registry>,
    mut seen: mpsc::UnboundedReceiver<(SessionId, PathBuf, facts::Seen)>,
) {
    while let Some((id, cwd, what)) = seen.recv().await {
        let Ok(name) = registry.get(id).map(|s| s.key()) else {
            continue;
        };
        let registry = registry.clone();
        let _ = tokio::task::spawn_blocking(move || learn(&registry, &name, &cwd, what)).await;
    }
}

/// Records one sighting, and proposes what it completes.
fn learn(registry: &Registry, session: &str, cwd: &Path, seen: facts::Seen) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let root = project::root(cwd);
    let sighting = facts::Sighting {
        at: now,
        session: session.to_owned(),
        seen,
    };
    let store = registry.context.lock().unwrap();
    if let Err(e) = store.record_fact(&root, &sighting, now) {
        tracing::warn!("couldn't record what ran: {e:#}");
        return;
    }
    let offers = facts::offers(&store.facts(&root, now), &store.load(&root), now);
    let mut proposed = false;
    for offer in offers {
        let new = NewDecision {
            cwd: cwd.to_path_buf(),
            title: offer.title,
            body: offer.body,
            kind: DecisionKind::Policy,
            propose: true,
            supersedes: None,
            review_every: None,
            session: None,
        };
        let provenance = Provenance {
            by: "valkyrie".into(),
            session: Some(session.to_owned()),
            conversation: None,
            commit: None,
            cwd: Some(cwd.to_path_buf()),
        };
        match store.decide(&new, provenance, now) {
            Ok(d) => {
                tracing::info!(id = d.id, project = %root.display(), "repository fact proposed");
                proposed = true;
            }
            Err(e) => tracing::warn!("couldn't propose a repository fact: {e:#}"),
        }
    }
    drop(store);
    if proposed {
        registry.refresh_proposals();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(tool: &str, input: Value) -> Value {
        json!({"hook_event_name":"PostToolUse","cwd":"/nowhere","tool_name":tool,"tool_input":input})
    }

    #[test]
    fn checks_before_a_push_then_ci_after_it() {
        let mut w = Watch::default();
        let bash = |c: &str| tool("Bash", json!({ "command": c }));
        assert_eq!(w.hook(&bash("cargo fmt --all --check")), None);
        assert_eq!(w.hook(&bash("cargo test -q 2>&1 | tail -3")), None);
        assert_eq!(w.hook(&bash("cargo test -q")), None);
        // Not CI yet: nothing pushed.
        assert_eq!(w.hook(&tool("mcp__bamboo__get_build", json!({}))), None);
        let (_, seen) = w.hook(&bash("git push -u origin fix")).unwrap();
        assert_eq!(
            seen,
            facts::Seen::Push {
                checks: vec!["cargo fmt --all --check".into(), "cargo test -q".into()]
            }
        );
        let (_, seen) = w.hook(&tool("mcp__bamboo__get_build", json!({}))).unwrap();
        assert_eq!(
            seen,
            facts::Seen::Ci {
                how: "`mcp__bamboo__get_build`".into()
            }
        );
        // Once per push.
        assert_eq!(w.hook(&tool("mcp__bamboo__get_build", json!({}))), None);
        // The next push starts its own list.
        let (_, seen) = w.hook(&bash("git push")).unwrap();
        assert_eq!(seen, facts::Seen::Push { checks: vec![] });
        let curl = bash("curl -s 'https://jenkins.corp/job/app/lastBuild/api/json?token=x'");
        let (_, seen) = w.hook(&curl).unwrap();
        assert_eq!(
            seen,
            facts::Seen::Ci {
                how: "`curl` to jenkins.corp".into()
            }
        );
    }

    #[test]
    fn asking_about_ci_after_a_clean_stop_with_a_pr_counts() {
        let prompt = |p: &str| json!({"hook_event_name":"UserPromptSubmit","cwd":"/r","prompt":p});
        let mut w = Watch::default();
        assert_eq!(w.hook(&prompt("is CI green?")), None);
        w.stopped("clean", true);
        assert_eq!(w.hook(&prompt("thanks, now rename the module")), None);
        w.stopped("clean", true);
        let (_, seen) = w.hook(&prompt("did the build pass?")).unwrap();
        assert!(matches!(seen, facts::Seen::Ci { how } if how.starts_with("you asked: “did")));
        w.stopped("no-pr", false);
        assert_eq!(w.hook(&prompt("is CI green?")), None);
    }
}

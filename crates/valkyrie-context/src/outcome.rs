//! How a policy's blocks turned out (ADR-0013 §6), so `health` can flag a policy
//! the user keeps overriding. A block (a stop the hook kept going) is *handled*
//! when the next stop shows the work got where the policy sent it, or the agent
//! stopped short saying it couldn't (the template tells it to). It's
//! *overridden* when the user interrupted the agent before that stop, or
//! answered a stop still short of it with something that isn't routine. This is
//! the pure part; the daemon watches the sessions and records each outcome in
//! the policy's own scope.

use crate::policy::Until;
use crate::{reply, stale};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus};

/// How long an outcome counts.
pub const OUTCOME_DAYS: u64 = 90;
/// In how many sessions a policy is overridden, since it was accepted, before
/// health says to reword or retire it (ADR-0013 §6).
pub const OVERRIDDEN: usize = 3;

/// A stop a policy kept going: the hook's `valk_continued`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub policy: u32,
    /// `user` for a habit, `project` for the repository's own policy.
    pub scope: String,
    /// The work state the policy acted on.
    pub state: String,
    /// How far `ship` sends the work; `None` for a template without a step.
    pub until: Option<Until>,
}

impl Block {
    pub fn from_json(v: &serde_json::Value) -> Option<Block> {
        Some(Block {
            policy: u32::try_from(v["policy"].as_u64()?).ok()?,
            scope: v["scope"].as_str().unwrap_or("project").to_owned(),
            state: v["state"].as_str()?.to_owned(),
            until: v["until"].as_str().and_then(Until::parse_str),
        })
    }

    /// Whether a stop in `after` (with a PR or not) is where the policy sent the
    /// work: as far as `until`, or, without one, out of the state it acted on.
    pub fn settled(&self, after: &str, pr: bool) -> bool {
        match self.until {
            Some(until) => reply::reached(after, pr).is_some_and(|got| got >= Some(until)),
            None => !after.is_empty() && after != self.state,
        }
    }
}

/// Whether the agent's last message says it stopped because it couldn't go on,
/// as the template allows ("if they fail and you can't fix them, stop and say
/// why"). Only words that say it couldn't, not that something failed: "fixed the
/// failing test" and "0 failed" are success. Only measured with: a miss just
/// counts one less override.
pub fn gave_up(message: &str) -> bool {
    let text = message
        .to_lowercase()
        .replace(['\u{2019}', '\u{2018}'], "'");
    let words: Vec<&str> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .collect();
    words.iter().any(|w| {
        matches!(
            *w,
            "can't" | "cannot" | "couldn't" | "unable" | "wasn't" | "won't" | "blocked"
        )
    }) || words.windows(2).any(|p| {
        p == ["still", "fail"]
            || p == ["still", "fails"]
            || p == ["still", "failing"]
            || p == ["still", "red"]
    })
}

/// A stop a policy kept going, and where it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub block: Block,
    pub cwd: PathBuf,
    /// The agent whose stop it was: another agent's hooks in the session (a
    /// `codex exec` it ran) aren't its outcome.
    pub agent: String,
}

/// How a block turned out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub kept: Kept,
    pub overridden: bool,
    /// `stop`, `gave up` or `interrupted`; `reply` is decided by the daemon.
    pub how: &'static str,
}

/// Follows one session's blocks to their outcomes, from its hooks and the work
/// state each turn end left.
#[derive(Debug, Default)]
pub struct Follow {
    /// The last block, until a turn ends after it: then whether the agent said it
    /// gave up.
    block: Option<(Kept, Option<bool>)>,
    /// A block whose work stopped short of where it was sent, waiting on the
    /// user's reply (`true` once a prompt came), until the stop after it.
    reply_to: Option<(Kept, bool)>,
    done: Vec<Outcome>,
}

impl Follow {
    /// One of the session's own hooks. `cwd` is the session's, for a payload
    /// without one.
    pub fn hook(&mut self, agent: &str, payload: &serde_json::Value, cwd: &Path) {
        let theirs = |k: &Kept| k.agent == agent;
        if self.block.as_ref().is_some_and(|(k, _)| !theirs(k))
            || self.reply_to.as_ref().is_some_and(|(k, _)| !theirs(k))
        {
            return;
        }
        match payload["hook_event_name"].as_str() {
            // A subagent's stop isn't the turn's.
            Some("Stop") if payload["agent_id"].is_null() => {
                if let Some(block) = Block::from_json(&payload["valk_continued"]) {
                    let cwd = payload["cwd"]
                        .as_str()
                        .map_or_else(|| cwd.into(), PathBuf::from);
                    let agent = agent.to_owned();
                    self.block = Some((Kept { block, cwd, agent }, None));
                    // A reply that left the work short is blocked again: it's still
                    // the reply to judge, at the stop after this block's turn.
                    self.reply_to.take_if(|(_, answered)| !*answered);
                } else if let Some((_, ended @ None)) = &mut self.block {
                    *ended = Some(
                        payload["last_assistant_message"]
                            .as_str()
                            .is_some_and(gave_up),
                    );
                }
            }
            Some("UserPromptSubmit") => {
                // A prompt before the kept-going turn ended cut the agent off.
                if let Some((kept, _)) = self.block.take_if(|(_, ended)| ended.is_none()) {
                    self.finish(kept, true, "interrupted");
                }
                match &mut self.reply_to {
                    Some((_, answered @ false)) => *answered = true,
                    // A second prompt isn't the reply to it any more.
                    Some(_) => self.reply_to = None,
                    None => {}
                }
            }
            _ => {}
        }
    }

    /// A turn end's work state (with a PR or not), once git has been read.
    /// Returns the block the user's reply to the last stop answers, for the
    /// daemon to decide by how that reply is classified.
    pub fn stopped(&mut self, after: &str, pr: bool) -> Option<Kept> {
        // Only the stop right after one short of it is the reply's to judge.
        let replied = self
            .reply_to
            .take()
            .and_then(|(kept, answered)| answered.then_some(kept));
        if let Some((kept, Some(gave_up))) = self.block.take_if(|(_, ended)| ended.is_some()) {
            if kept.block.settled(after, pr) {
                self.finish(kept, false, "stop");
            } else if gave_up {
                self.finish(kept, false, "gave up");
            } else {
                self.reply_to = Some((kept, false));
            }
        }
        replied
    }

    /// The outcomes since the last call.
    pub fn take(&mut self) -> Vec<Outcome> {
        std::mem::take(&mut self.done)
    }

    fn finish(&mut self, kept: Kept, overridden: bool, how: &'static str) {
        self.done.push(Outcome {
            kept,
            overridden,
            how,
        });
    }
}

/// A recorded outcome (`policy.jsonl` in the policy's scope).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub at: u64,
    pub session: String,
    pub policy: u32,
    pub scope: String,
    /// The repository the session worked in.
    pub project: String,
    pub overridden: bool,
    /// `stop`, `gave up`, `interrupted` or `reply`.
    pub how: String,
}

impl Record {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "at": self.at, "session": self.session, "policy": self.policy,
            "scope": self.scope, "project": self.project,
            "outcome": if self.overridden { "overridden" } else { "handled" },
            "how": self.how,
        })
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Record> {
        Some(Record {
            at: v["at"].as_u64()?,
            session: v["session"].as_str()?.to_owned(),
            policy: u32::try_from(v["policy"].as_u64()?).ok()?,
            scope: v["scope"].as_str().unwrap_or_default().to_owned(),
            project: v["project"].as_str().unwrap_or_default().to_owned(),
            overridden: v["outcome"].as_str()? == "overridden",
            how: v["how"].as_str().unwrap_or_default().to_owned(),
        })
    }
}

/// A policy health flags: overridden in `overridden` of the `blocked` sessions
/// it kept going since it was accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flag {
    pub id: u32,
    pub overridden: usize,
    pub blocked: usize,
}

/// The active policies among `decisions` overridden in [`OVERRIDDEN`] or more
/// sessions since they were accepted. A session counts once however often it
/// overrode.
pub fn flagged(records: &[Record], decisions: &[Decision]) -> Vec<Flag> {
    let mut sessions: HashMap<u32, (HashSet<&str>, HashSet<&str>)> = HashMap::new();
    for d in decisions
        .iter()
        .filter(|d| d.status == DecisionStatus::Active && d.kind == DecisionKind::Policy)
    {
        let since = stale::confirmed(d);
        let (over, all) = sessions.entry(d.id).or_default();
        for r in records.iter().filter(|r| r.policy == d.id && r.at > since) {
            all.insert(&r.session);
            if r.overridden {
                over.insert(&r.session);
            }
        }
    }
    let mut out: Vec<Flag> = sessions
        .into_iter()
        .filter(|(_, (over, _))| over.len() >= OVERRIDDEN)
        .map(|(id, (over, all))| Flag {
            id,
            overridden: over.len(),
            blocked: all.len(),
        })
        .collect();
    out.sort_by_key(|f| f.id);
    out
}

/// What health says about a flagged policy.
pub fn says(flag: &Flag) -> String {
    format!(
        "you overrode it in {} of the {} sessions it kept going since it was accepted; \
         reword it (`valk decisions` → edit) or retire it",
        flag.overridden, flag.blocked
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(session: &str, policy: u32, at: u64, overridden: bool) -> Record {
        Record {
            at,
            session: session.into(),
            policy,
            scope: "project".into(),
            project: "/r".into(),
            overridden,
            how: if overridden { "interrupted" } else { "stop" }.into(),
        }
    }

    fn policy(id: u32, status: DecisionStatus, confirmed: u64) -> Decision {
        let mut d = Decision {
            id,
            project: "/r".into(),
            title: format!("p{id}"),
            body: "template: ship\nuntil: pr".into(),
            kind: DecisionKind::Policy,
            status,
            created: 1,
            updated: 1,
            supersedes: None,
            superseded_by: None,
            pinned: false,
            scope: Vec::new(),
            proposed_scope: None,
            scope_asked: false,
            fresh: Default::default(),
            provenance: Default::default(),
        };
        d.fresh.confirmed = confirmed;
        d
    }

    #[test]
    fn a_block_settles_when_the_work_gets_as_far_as_it_was_sent() {
        let ship = |until| Block {
            policy: 1,
            scope: "project".into(),
            state: "uncommitted".into(),
            until: Some(until),
        };
        assert!(ship(Until::Commit).settled("unpushed", false));
        assert!(!ship(Until::Push).settled("unpushed", false));
        assert!(ship(Until::Push).settled("no-pr", false));
        assert!(!ship(Until::Pr).settled("no-pr", false));
        assert!(ship(Until::Pr).settled("clean", true));
        assert!(!ship(Until::Pr).settled("clean", false));
        let fix = Block {
            policy: 2,
            scope: "project".into(),
            state: "pr-red".into(),
            until: None,
        };
        assert!(!fix.settled("pr-red", true));
        assert!(fix.settled("clean", true));
        assert!(!fix.settled("", false));
    }

    #[test]
    fn reads_a_block_from_the_hook_and_a_record_back() {
        let b = Block::from_json(&serde_json::json!(
            {"policy": 9, "scope": "user", "state": "no-pr", "until": "pr"}))
        .unwrap();
        assert_eq!(
            (b.policy, b.scope.as_str(), b.until),
            (9, "user", Some(Until::Pr))
        );
        let r = rec("s", 9, 5, true);
        assert_eq!(Record::from_json(&r.to_json()), Some(r));
    }

    fn hook(f: &mut Follow, v: serde_json::Value) {
        f.hook("claude", &v, Path::new("/r"));
    }

    fn block(f: &mut Follow) {
        hook(
            f,
            serde_json::json!({"hook_event_name": "Stop", "cwd": "/r/w",
            "valk_continued": {"policy": 4, "scope": "user", "state": "uncommitted",
                               "until": "pr"}}),
        );
    }

    fn stop(f: &mut Follow, message: &str) {
        hook(
            f,
            serde_json::json!({"hook_event_name": "Stop",
            "last_assistant_message": message}),
        );
    }

    fn prompt(f: &mut Follow) {
        hook(
            f,
            serde_json::json!({"hook_event_name": "UserPromptSubmit", "prompt": "hm"}),
        );
    }

    fn hows(f: &mut Follow) -> Vec<(bool, &'static str)> {
        f.take().iter().map(|o| (o.overridden, o.how)).collect()
    }

    #[test]
    fn follows_a_block_to_how_it_turned_out() {
        let mut f = Follow::default();
        // The agent opened the PR: handled.
        block(&mut f);
        stop(&mut f, "Opened PR #3.");
        assert_eq!(f.stopped("clean", true), None);
        let done = f.take();
        assert_eq!((done[0].overridden, done[0].how), (false, "stop"));
        assert_eq!(done[0].kept.cwd, Path::new("/r/w"));
        assert_eq!(done[0].kept.block.policy, 4);

        // You typed before it stopped: interrupted.
        block(&mut f);
        hook(
            &mut f,
            serde_json::json!({"hook_event_name": "Stop", "agent_id": "sub"}),
        );
        prompt(&mut f);
        assert_eq!(hows(&mut f), [(true, "interrupted")]);
        stop(&mut f, "ok");
        assert_eq!(f.stopped("uncommitted", false), None);
        assert!(f.take().is_empty(), "that block is over");

        // Checks failed and it said so: the template's way out, handled.
        block(&mut f);
        stop(&mut f, "cargo test fails in parse.rs and I can't fix it");
        assert_eq!(f.stopped("uncommitted", false), None);
        assert_eq!(hows(&mut f), [(false, "gave up")]);

        // Stopped short with no reason: your reply decides, at the stop after it.
        block(&mut f);
        stop(&mut f, "Done.");
        assert_eq!(f.stopped("unpushed", false), None);
        assert!(f.take().is_empty());
        prompt(&mut f);
        stop(&mut f, "Pushed.");
        let replied = f
            .stopped("no-pr", false)
            .expect("the reply answers the block");
        assert_eq!(replied.block.policy, 4);
        assert!(f.take().is_empty());

        // Your reply left it short and the policy blocked again: the reply still
        // decides, once that turn ends.
        block(&mut f);
        stop(&mut f, "Done.");
        f.stopped("uncommitted", false);
        prompt(&mut f);
        block(&mut f);
        stop(&mut f, "Explained.");
        let replied = f.stopped("uncommitted", false).expect("still the reply's");
        assert_eq!(replied.block.policy, 4);
        f.stopped("uncommitted", false);

        // Another agent in the session (a `codex exec`) isn't the block's.
        block(&mut f);
        f.hook(
            "codex",
            &serde_json::json!({"hook_event_name": "UserPromptSubmit"}),
            Path::new("/r"),
        );
        assert!(f.take().is_empty());
        stop(&mut f, "Opened PR.");
        f.stopped("clean", true);
        assert_eq!(hows(&mut f), [(false, "stop")]);

        // A stop short with no reply before the next stop is left alone.
        block(&mut f);
        stop(&mut f, "Done.");
        f.stopped("unpushed", false);
        stop(&mut f, "still here");
        assert_eq!(f.stopped("unpushed", false), None);
        prompt(&mut f);
        stop(&mut f, "x");
        assert_eq!(
            f.stopped("unpushed", false),
            None,
            "a later reply isn't to it"
        );
        assert!(f.take().is_empty());
    }

    #[test]
    fn hears_an_agent_say_it_gave_up() {
        assert!(gave_up(
            "cargo test fails in two places and I couldn't fix it."
        ));
        assert!(gave_up("CI is red and I can\u{2019}t reach the logs"));
        assert!(gave_up("Clippy still fails on parse.rs"));
        assert!(!gave_up("Committed; tests pass, 0 failed. Opened the PR."));
        assert!(!gave_up("Fixed the failing test and added error handling."));
        assert!(!gave_up(
            "Committed, pushed and opened PR #12. Required checks pass."
        ));
        assert!(!gave_up("Done: opened the PR, recorded it on #4."));
    }

    #[test]
    fn flags_a_policy_overridden_in_three_sessions_since_it_was_accepted() {
        let list = [
            policy(1, DecisionStatus::Active, 100),
            policy(2, DecisionStatus::Retired, 100),
        ];
        let mut records = vec![
            // Before it was accepted: doesn't count.
            rec("old", 1, 50, true),
            rec("a", 1, 200, true),
            rec("a", 1, 210, true),
            rec("b", 1, 220, true),
            rec("c", 1, 230, false),
            rec("x", 2, 200, true),
            rec("y", 2, 200, true),
            rec("z", 2, 200, true),
        ];
        assert!(
            flagged(&records, &list).is_empty(),
            "two sessions isn't three"
        );
        records.push(rec("c", 1, 240, true));
        records.push(rec("d", 1, 250, false));
        assert_eq!(
            flagged(&records, &list),
            [Flag {
                id: 1,
                overridden: 3,
                blocked: 4
            }]
        );
        assert!(says(&flagged(&records, &list)[0]).contains("3 of the 4 sessions"));
    }
}

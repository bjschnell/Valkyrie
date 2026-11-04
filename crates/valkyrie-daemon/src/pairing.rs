//! Pairs a stop short of shipping with the user's reply to it (ADR-0013 §2), and
//! follows the stops policies kept going (§6). A turn end's work state takes a
//! while to read (git, then `gh`), so each read is numbered. A prompt that
//! arrives while the latest one is still being read waits for it and is
//! replayed once it lands: otherwise a quick reply ("ship it", "no") would find
//! no stop to answer. Only the latest read sets the state; an older one finishing
//! late is just logged.

use serde_json::Value;
use std::path::{Path, PathBuf};
use valkyrie_context::outcome::{Follow, Kept, Outcome};

#[derive(Debug, Default)]
pub struct Pairing {
    /// The last stop's work state, while it was short of shipping and no prompt
    /// has answered it yet.
    unshipped: Option<String>,
    /// The prompt that answered it, with that state, until the next stop shows
    /// how far the work got.
    answer: Option<(String, String)>,
    blocks: Follow,
    /// The last read started, and the last one that landed.
    started: u64,
    landed: u64,
    /// Prompts (their agent and payload) that came while the latest read was
    /// still going.
    waiting: Vec<(String, Value)>,
}

/// A reply and the stop it answered: what `habit` classifies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub block: Option<Kept>,
    pub before: String,
    pub prompt: String,
    pub after: String,
    pub pr: bool,
}

impl Pairing {
    /// One of the session's own hooks. `cwd` is the session's. A turn end (the
    /// main thread's stop, not one a policy kept going, with its `cwd`) starts a
    /// read of the work state: its number and where, for [`Self::read`].
    pub fn hook(&mut self, agent: &str, payload: &Value, cwd: &Path) -> Option<(u64, PathBuf)> {
        let turn_end = payload["hook_event_name"] == "Stop"
            && payload["agent_id"].is_null()
            && payload["valk_continued"].is_null();
        if payload["hook_event_name"] == "UserPromptSubmit" && self.started > self.landed {
            self.waiting.push((agent.to_owned(), payload.clone()));
            return None;
        }
        let dir = payload["cwd"]
            .as_str()
            .filter(|_| turn_end)
            .map(PathBuf::from);
        if dir.is_some() {
            // The prompts waiting came before this turn end, whose read overtakes
            // theirs: they go first.
            self.release(cwd);
        }
        self.apply(agent, payload, cwd);
        let dir = dir?;
        self.started += 1;
        Some((self.started, dir))
    }

    /// Whether read `n` is the latest, so what it found is where the work is.
    pub fn latest(&self, n: u64) -> bool {
        n == self.started
    }

    /// The work state read `n` found (`None` when it found none, outside git),
    /// with whether the branch has a PR. Returns the reply it completes, when
    /// the stop before was short of shipping and the user answered it.
    pub fn read(&mut self, n: u64, work: Option<(&str, bool)>, cwd: &Path) -> Option<Paired> {
        if n != self.started {
            // Overtaken by a later turn end, which says where the work is now.
            return None;
        }
        self.landed = n;
        let paired = work.and_then(|(after, pr)| {
            let block = self.blocks.stopped(after, pr);
            let paired = self.answer.take().map(|(before, prompt)| Paired {
                block,
                before,
                prompt,
                after: after.to_owned(),
                pr,
            });
            self.unshipped =
                matches!(after, "uncommitted" | "unpushed" | "no-pr").then(|| after.to_owned());
            paired
        });
        if work.is_none() {
            self.unshipped = None;
            self.answer = None;
        }
        self.release(cwd);
        paired
    }

    fn release(&mut self, cwd: &Path) {
        for (agent, payload) in std::mem::take(&mut self.waiting) {
            self.apply(&agent, &payload, cwd);
        }
    }

    /// How blocks turned out since the last call.
    pub fn outcomes(&mut self) -> Vec<Outcome> {
        self.blocks.take()
    }

    fn apply(&mut self, agent: &str, payload: &Value, cwd: &Path) {
        self.blocks.hook(agent, payload, cwd);
        // The user's first prompt after a stop short of shipping answers it.
        if payload["hook_event_name"] == "UserPromptSubmit" {
            let unshipped = self.unshipped.take();
            if let (Some(before), Some(prompt)) = (unshipped, payload["prompt"].as_str()) {
                self.answer = Some((before, prompt.to_owned()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CWD: &str = "/r";

    fn prompt(p: &mut Pairing, text: &str) {
        p.hook(
            "claude",
            &json!({"hook_event_name": "UserPromptSubmit", "prompt": text}),
            Path::new(CWD),
        );
    }

    fn stop(p: &mut Pairing) -> u64 {
        p.hook(
            "claude",
            &json!({"hook_event_name": "Stop", "cwd": CWD}),
            Path::new(CWD),
        )
        .expect("a turn end starts a read")
        .0
    }

    fn read(p: &mut Pairing, n: u64, after: &str) -> Option<(String, String, String)> {
        p.read(n, Some((after, false)), Path::new(CWD))
            .map(|a| (a.before, a.prompt, a.after))
    }

    fn s(a: &str, b: &str, c: &str) -> Option<(String, String, String)> {
        Some((a.into(), b.into(), c.into()))
    }

    #[test]
    fn a_reply_after_the_read_answers_the_stop() {
        let mut p = Pairing::default();
        let n = stop(&mut p);
        assert_eq!(read(&mut p, n, "uncommitted"), None);
        prompt(&mut p, "ship it");
        let n = stop(&mut p);
        assert_eq!(
            read(&mut p, n, "no-pr"),
            s("uncommitted", "ship it", "no-pr")
        );
    }

    #[test]
    fn a_reply_typed_while_the_stop_is_read_still_answers_it() {
        let mut p = Pairing::default();
        let n = stop(&mut p);
        // You answered before git and `gh` had said where the work was.
        prompt(&mut p, "ship it");
        assert_eq!(read(&mut p, n, "unpushed"), None);
        let n = stop(&mut p);
        assert_eq!(read(&mut p, n, "no-pr"), s("unpushed", "ship it", "no-pr"));
    }

    #[test]
    fn a_read_overtaken_by_a_later_turn_end_changes_nothing() {
        let mut p = Pairing::default();
        let first = stop(&mut p);
        prompt(&mut p, "hm");
        let second = stop(&mut p);
        // The second turn's read lands first; the first one's, late, is stale.
        assert_eq!(read(&mut p, second, "uncommitted"), None);
        assert_eq!(read(&mut p, first, "clean"), None);
        prompt(&mut p, "commit it");
        let n = stop(&mut p);
        assert_eq!(
            read(&mut p, n, "unpushed"),
            s("uncommitted", "commit it", "unpushed")
        );
    }

    #[test]
    fn a_stop_that_isnt_a_turn_end_keeps_the_reply_waiting() {
        let mut p = Pairing::default();
        let n = stop(&mut p);
        prompt(&mut p, "ship it");
        // The turn the reply started: a subagent's stop, then a policy's block.
        let sub = json!({"hook_event_name": "Stop", "agent_id": "a", "cwd": CWD});
        assert_eq!(p.hook("claude", &sub, Path::new(CWD)), None);
        let kept = json!({"hook_event_name": "Stop", "cwd": CWD, "valk_continued":
            {"policy": 1, "state": "unpushed", "until": "pr"}});
        assert_eq!(p.hook("claude", &kept, Path::new(CWD)), None);
        assert_eq!(p.waiting.len(), 1);
        assert_eq!(read(&mut p, n, "unpushed"), None);
        let n = stop(&mut p);
        assert_eq!(read(&mut p, n, "no-pr"), s("unpushed", "ship it", "no-pr"));
    }

    #[test]
    fn a_read_outside_git_pairs_nothing_and_releases_the_prompts() {
        let mut p = Pairing::default();
        let n = stop(&mut p);
        read(&mut p, n, "uncommitted");
        prompt(&mut p, "ship it");
        let n = stop(&mut p);
        assert_eq!(p.read(n, None, Path::new(CWD)), None);
        // Not held forever: the next prompt goes straight through.
        prompt(&mut p, "x");
        assert!(p.waiting.is_empty());
    }

    #[test]
    fn a_quick_override_still_reaches_the_policys_block() {
        let mut p = Pairing::default();
        p.hook(
            "claude",
            &json!({"hook_event_name": "Stop", "valk_continued":
                {"policy": 3, "scope": "project", "state": "uncommitted", "until": "pr"}}),
            Path::new(CWD),
        );
        let n = stop(&mut p);
        prompt(&mut p, "no, leave it");
        read(&mut p, n, "uncommitted");
        assert!(p.outcomes().is_empty(), "the reply decides");
        let n = stop(&mut p);
        let a = p
            .read(n, Some(("uncommitted", false)), Path::new(CWD))
            .unwrap();
        assert_eq!(a.block.map(|k| k.block.policy), Some(3));
    }
}

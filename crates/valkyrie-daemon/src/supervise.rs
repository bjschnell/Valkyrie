//! Supervisor proposals (ADR-0019 §2): for each flag the PR poll raises on an idle
//! session, one step from a fixed list, offered on the session's queue row until
//! the user takes it or the session moves on. A conflicting pull request gets
//! `merge-base`; a red one gets `rerun` when every failed check is a known flake,
//! else `ci-fix` (the flake call: `valkyrie_context::flake`'s rules, then the
//! model for what they can't place, never with `valk decisions auto off`). `leave`
//! drops the offer and keeps the flag.
//!
//! Each answer is recorded in the project's `supervisor.jsonl` (ADR-0019 §3).
//! Once the user has taken the same step on the same trigger in two sessions, a
//! `policy` for it is proposed; accepted, the step runs without asking, unless the
//! Context setting is off, and its outcome is measured for `valk decisions
//! health`. `valk supervise off` stops all of it. Nothing reaches an agent but a
//! fixed template.

use crate::Registry;
use crate::context::Peer;
use crate::work::{Check, GH_TIMEOUT, Pr};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use valkyrie_context::flake::{self, Bug, Failure, Verdict};
use valkyrie_context::policy::{self, Trigger};
use valkyrie_context::{outcome, project, steps};
use valkyrie_proto::{
    AutoMode, DecisionKind, Flag, Item, ItemKind, ItemStatus, NewDecision, NoWindow, Provenance,
    Reply, SessionId, Step, StepAction,
};

/// Flake calls asked of the model per hour, at most.
const PER_HOUR: usize = 6;
const MODEL_TIMEOUT: Duration = Duration::from_secs(90);
/// Longest a failed job's log is read for (it can be large).
const LOG_TIMEOUT: Duration = Duration::from_secs(30);
/// The tail of a failed log that's kept: panics and failed tests are at the end.
const LOG_KEEP: usize = 20_000;

/// What the supervisor offers on one flagged session.
#[derive(Debug, Clone)]
pub(crate) struct Offer {
    /// The transition the flag raised; anything newer means it moved on.
    seq: u64,
    reason: String,
    pr: String,
    step: Option<Step>,
    /// What taking the step needs.
    project: PathBuf,
    branch: String,
    base: Option<String>,
    failed: Vec<Check>,
    head: String,
}

impl Offer {
    /// The watcher's flag it answers: a conflict, when there's one, comes first.
    fn trigger(&self) -> Trigger {
        if self.base.is_some() {
            Trigger::Conflicting
        } else {
            Trigger::Red
        }
    }
}

/// A step a policy took, until the PR poll can say how it turned out
/// (ADR-0013 §6, ADR-0019 §3).
#[derive(Debug, Clone)]
pub(crate) struct Taken {
    policy: u32,
    trigger: Trigger,
    pr: String,
    root: PathBuf,
    /// The session's name, for the outcome.
    session: String,
    /// The session's prompts before the step, and whether the step typed one.
    prompts: u64,
    typed: bool,
    at: Instant,
}

/// How long a policy's step waits for the poll to judge it before it's let go.
const JUDGE_WITHIN: Duration = Duration::from_secs(24 * 3600);

/// What the poll's read says about a step a policy took.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Judged {
    /// Not yet: the agent is still at it, or CI or mergeability isn't settled.
    Wait,
    /// The outcome, and how it was seen.
    Outcome { overridden: bool, how: &'static str },
    /// Nothing to measure: the session's gone, the problem's still there, or it
    /// took too long.
    Drop,
}

/// Judges a step a policy took from the session's prompts and state and the PR
/// as the poll read it. Handled: the next settled read is clean (or the PR merged
/// or closed). Overridden: the user prompted the session before that.
pub(crate) fn judge(taken: &Taken, exited: bool, prompts: u64, resting: bool, pr: &Pr) -> Judged {
    let theirs = taken.prompts + u64::from(taken.typed);
    if exited || taken.at.elapsed() > JUDGE_WITHIN {
        return Judged::Drop;
    }
    if prompts > theirs {
        return Judged::Outcome {
            overridden: true,
            how: "prompt",
        };
    }
    // The step's own prompt not heard yet, or the agent still on it.
    if prompts < theirs || !resting {
        return Judged::Wait;
    }
    match pr {
        Pr::Merged { .. } | Pr::Closed { .. } => Judged::Outcome {
            overridden: false,
            how: "merged or closed",
        },
        Pr::Open { settled: false, .. } | Pr::None | Pr::Unknown => Judged::Wait,
        Pr::Open {
            failed, conflicts, ..
        } => {
            let clean = match taken.trigger {
                Trigger::Conflicting => conflicts.is_none(),
                Trigger::Red => failed.is_empty(),
            };
            if clean {
                Judged::Outcome {
                    overridden: false,
                    how: "watcher",
                }
            } else {
                Judged::Drop
            }
        }
    }
}

/// The offers, the reruns done and the model's budget.
#[derive(Default)]
pub(crate) struct Supervisor {
    offers: HashMap<SessionId, Offer>,
    /// The PR head each pull request was last rerun at: once per head.
    reruns: HashMap<String, String>,
    path: PathBuf,
    calls: VecDeque<Instant>,
    /// What policies did (pull request, step, head): once each, so a step that
    /// didn't fix it goes back to the user instead of running again.
    automatic: HashSet<(String, StepAction, String)>,
    /// Steps policies took, by session, until the poll judges them.
    taken: HashMap<SessionId, Taken>,
    /// Reads the Context setting; tests set their own.
    context_on: Option<fn() -> bool>,
}

impl Supervisor {
    pub(crate) fn load(path: PathBuf) -> Self {
        let reruns = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            reruns,
            path,
            ..Self::default()
        }
    }

    fn save(&self) {
        let write = || -> Result<()> {
            let text = serde_json::to_vec_pretty(&self.reruns)?;
            let tmp = self.path.with_extension("tmp");
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, &self.path)?;
            Ok(())
        };
        if let Err(e) = write() {
            tracing::warn!("couldn't save the reruns: {e:#}");
        }
    }

    /// Whether the supervisor runs: `valk supervise off` leaves a file saying not.
    pub(crate) fn on(&self) -> bool {
        !self.off_file().exists()
    }

    fn off_file(&self) -> PathBuf {
        self.path.with_file_name("supervise-off")
    }

    fn set_on(&mut self, on: bool) -> Result<()> {
        let file = self.off_file();
        if on {
            match std::fs::remove_file(&file) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    return Err(e).with_context(|| format!("remove {}", file.display()));
                }
                _ => {}
            }
        } else {
            std::fs::write(&file, "off\n").with_context(|| format!("write {}", file.display()))?;
            self.offers.clear();
        }
        Ok(())
    }

    fn rerun_spent(&self, pr: &str, head: &str) -> bool {
        !head.is_empty() && self.reruns.get(pr).is_some_and(|h| h == head)
    }

    /// Room for one more model call this hour; takes it.
    fn room(&mut self) -> bool {
        let hour = Duration::from_secs(3600);
        while self.calls.front().is_some_and(|t| t.elapsed() > hour) {
            self.calls.pop_front();
        }
        let room = self.calls.len() < PER_HOUR;
        if room {
            self.calls.push_back(Instant::now());
        }
        room
    }
}

/// What the flake call reads about a red pull request (gathered by `gather`).
#[derive(Debug, Default)]
pub(crate) struct Signals {
    pub failures: Vec<Failure>,
    /// The files the pull request changes.
    pub changed: Vec<String>,
}

/// The step for a flag, apart from the model: conflicting merges the base in
/// (CI on a conflicting branch says little); red reruns when the rules call every
/// failure a flake, unless this head was rerun already, and fixes CI when one is
/// real. `None`: the rules can't say.
pub(crate) fn pick(conflicts: bool, verdict: &Verdict, rerun_spent: bool) -> Option<Step> {
    if conflicts {
        return Some(Step {
            action: StepAction::MergeBase,
            why: "the branch conflicts with its base".into(),
            alt: None,
        });
    }
    Some(match verdict {
        Verdict::Rerun(why) if rerun_spent => Step {
            action: StepAction::CiFix,
            why: format!("{why}, but this commit was rerun once already"),
            alt: None,
        },
        Verdict::Rerun(why) => Step {
            action: StepAction::Rerun,
            why: why.clone(),
            alt: Some(StepAction::CiFix),
        },
        Verdict::CiFix(why) => Step {
            action: StepAction::CiFix,
            why: why.clone(),
            alt: None,
        },
        Verdict::Unsure => return None,
    })
}

/// Whether every failed check is a GitHub Actions run, so `rerun` could work.
fn rerunnable(failed: &[Check]) -> bool {
    !failed.is_empty() && failed.iter().all(|c| c.run.is_some())
}

impl Registry {
    /// After the poll flagged `session` at `seq` about `item`'s pull request:
    /// works out the step to offer, and offers it.
    pub(crate) async fn offer_step(
        &self,
        item: &Item,
        session: SessionId,
        seq: u64,
        reason: &str,
        pr: &Pr,
    ) {
        let Pr::Open {
            url,
            failed,
            conflicts,
            head,
            ..
        } = pr
        else {
            return;
        };
        if !self.supervisor.lock().unwrap().on() {
            return;
        }
        let project = item
            .checkout
            .clone()
            .filter(|p| p.is_dir())
            .or_else(|| item.project.clone())
            .unwrap_or_else(std::env::temp_dir);
        let mut offer = Offer {
            seq,
            reason: reason.to_owned(),
            pr: url.clone(),
            step: None,
            project: project.clone(),
            branch: item
                .branch
                .clone()
                .or_else(|| crate::work::current_branch(&project))
                .unwrap_or_default(),
            base: conflicts.clone(),
            failed: failed.clone(),
            head: head.clone(),
        };
        // Offers on sessions that have moved on go; this one's shown at once, its
        // step to follow, since the flake call can take a while.
        let live: HashMap<SessionId, u64> = self
            .supervisor
            .lock()
            .unwrap()
            .offers
            .keys()
            .filter_map(|&id| Some((id, self.get(id).ok()?.flag()?.0)))
            .collect();
        {
            let mut sup = self.supervisor.lock().unwrap();
            sup.offers.retain(|id, o| live.get(id) == Some(&o.seq));
            sup.offers.insert(session, offer.clone());
        }
        self.host.changed.notify_one();
        let spent = self.supervisor.lock().unwrap().rerun_spent(url, head);
        let verdict = if conflicts.is_some() {
            Verdict::Unsure
        } else if !rerunnable(failed) {
            // Nothing to rerun outside Actions: no need to read anything.
            Verdict::CiFix("a failed check isn't GitHub Actions, so it can't be rerun".into())
        } else {
            self.flake_call(item, url, head, failed, &project).await
        };
        let step = pick(conflicts.is_some(), &verdict, spent).unwrap_or_else(|| Step {
            action: StepAction::CiFix,
            why: "the rules couldn't place the failure".into(),
            alt: (!spent).then_some(StepAction::Rerun),
        });
        tracing::info!(
            item = item.id,
            session,
            action = step.action.as_str(),
            why = step.why,
            "supervisor offers a step"
        );
        if let Some(policy) = self.policy_for(&offer, &step) {
            let key = (offer.pr.clone(), step.action, offer.head.clone());
            match self
                .run_step(session, &offer, &step, step.action, seq, Some(policy))
                .await
            {
                Ok(said) => {
                    tracing::info!(
                        item = item.id,
                        session,
                        policy,
                        said,
                        "a policy took a step"
                    );
                    let mut sup = self.supervisor.lock().unwrap();
                    sup.automatic.insert(key);
                    sup.offers.remove(&session);
                    drop(sup);
                    self.host.changed.notify_one();
                    return;
                }
                // Past a limit, say: it's the user's to take.
                Err(e) => tracing::info!(session, policy, "offering the step instead: {e:#}"),
            }
        }
        offer.step = Some(step);
        let mut sup = self.supervisor.lock().unwrap();
        // Only if the flag is still the one this was worked out for.
        if sup.offers.get(&session).is_some_and(|o| o.seq == seq) {
            sup.offers.insert(session, offer);
        }
        drop(sup);
        self.host.changed.notify_one();
    }

    /// The rules over what can be read, then the model for what they can't place
    /// (not with auto off, nor past its hourly limit).
    async fn flake_call(
        &self,
        item: &Item,
        url: &str,
        head: &str,
        failed: &[Check],
        project: &Path,
    ) -> Verdict {
        let bugs = self.open_bugs(item);
        let (url, head, failed, project) = (
            url.to_owned(),
            head.to_owned(),
            failed.to_vec(),
            project.to_owned(),
        );
        let gathered = {
            let (url, project) = (url.clone(), project.clone());
            tokio::task::spawn_blocking(move || gather(&project, &url, &head, &failed))
        };
        let signals = gathered.await.unwrap_or_default();
        let calls: Vec<_> = signals
            .failures
            .iter()
            .map(|f| flake::call(f, &signals.changed, &bugs))
            .collect();
        let verdict = flake::verdict(&calls);
        if verdict != Verdict::Unsure {
            return verdict;
        }
        let mode = crate::extract::mode(&self.host.context_dir);
        if mode == AutoMode::Off || !self.supervisor.lock().unwrap().room() {
            return Verdict::Unsure;
        }
        let unsure: Vec<Failure> = signals
            .failures
            .iter()
            .zip(&calls)
            .filter(|(_, c)| **c == flake::Call::Unsure)
            .map(|(f, _)| f.clone())
            .collect();
        let dir = self.host.context_dir.clone();
        let asked = tokio::task::spawn_blocking(move || {
            let prompt = flake::prompt(
                &unsure,
                &signals.changed,
                &bugs,
                valkyrie_context::extract::redact,
            );
            valkyrie_proto::ensure_private_dir(&dir)?;
            let command = valkyrie_context::model::command(flake::SYSTEM);
            let reply = valkyrie_context::model::ask(&command, &prompt, &dir, MODEL_TIMEOUT)?;
            Ok::<_, anyhow::Error>(flake::parse_reply(&reply, &bugs))
        })
        .await;
        match asked {
            Ok(Ok(Some((true, why)))) => Verdict::Rerun(why),
            Ok(Ok(Some((false, why)))) => Verdict::CiFix(why),
            Ok(Ok(None)) => Verdict::Unsure,
            Ok(Err(e)) => {
                tracing::warn!(pr = url, "the flake call failed: {e:#}");
                Verdict::Unsure
            }
            Err(_) => Verdict::Unsure,
        }
    }

    /// The open bugs in `item`'s repository, title and body.
    fn open_bugs(&self, item: &Item) -> Vec<Bug> {
        let items = self.backlog.lock().unwrap().load();
        items
            .into_iter()
            .filter(|i| {
                i.kind == ItemKind::Bug
                    && matches!(i.status, ItemStatus::Open | ItemStatus::Proposed)
                    && i.project == item.project
            })
            .map(|i| (i.id, format!("{}\n{}", i.title, i.body)))
            .collect()
    }

    /// The flag `session` shows, with its offer, for its queue row.
    pub(crate) fn flag_of(&self, session: &crate::session::Session, id: SessionId) -> Option<Flag> {
        let (seq, reason) = session.flag()?;
        let sup = self.supervisor.lock().unwrap();
        let offer = sup.offers.get(&id).filter(|o| o.seq == seq);
        Some(Flag {
            reason,
            pr: offer.map(|o| o.pr.clone()).unwrap_or_default(),
            seq,
            step: offer.and_then(|o| o.step.clone()),
            working: offer.is_some_and(|o| o.step.is_none()),
        })
    }

    /// `ClientMsg::TakeStep`: the user only, on a session still showing the flag
    /// the step was offered for.
    pub(crate) async fn take_step(
        &self,
        id: SessionId,
        seq: u64,
        action: StepAction,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        self.caller(peer).vouch("taking a supervisor step")?;
        let session = self.get(id)?;
        let shown = session.flag().map(|(s, _)| s);
        let offer = {
            let mut sup = self.supervisor.lock().unwrap();
            sup.offers.retain(|_, o| o.seq == seq || shown != Some(seq));
            sup.offers.get(&id).filter(|o| o.seq == seq).cloned()
        };
        let Some(offer) = offer.filter(|_| shown == Some(seq)) else {
            bail!("session {id} has moved on since: nothing to take");
        };
        let Some(step) = offer.step.clone() else {
            bail!("the supervisor is still working out its step for session {id}");
        };
        if action != step.action && Some(action) != step.alt && action != StepAction::Leave {
            bail!("{} isn't offered for session {id}", action.as_str());
        }
        let said = self.run_step(id, &offer, &step, action, seq, None).await?;
        self.drop_offer(id);
        tracing::info!(
            session = id,
            action = action.as_str(),
            pr = offer.pr,
            "took a supervisor step"
        );
        Ok(Reply::StepTaken { said })
    }

    /// The active policy that takes `step` on `offer`'s flag without asking:
    /// not with the Context setting off, and once per pull request head.
    fn policy_for(&self, offer: &Offer, step: &Step) -> Option<u32> {
        let key = (offer.pr.clone(), step.action, offer.head.clone());
        {
            let sup = self.supervisor.lock().unwrap();
            if !sup.context_on.unwrap_or(context_on)() || sup.automatic.contains(&key) {
                return None;
            }
        }
        let root = project::root(&offer.project);
        let decisions = self.context.lock().unwrap().load(&root);
        policy::step_policy(&decisions, step.action, offer.trigger())
    }

    /// Takes `action` on `offer`, for the user or for `policy`, on a session still
    /// showing the flag raised at `seq`; logs it in the session's event log and
    /// records it in the project's `supervisor.jsonl`. The user's answer may make a
    /// policy worth proposing; a policy's step waits to be judged.
    async fn run_step(
        &self,
        id: SessionId,
        offer: &Offer,
        step: &Step,
        action: StepAction,
        seq: u64,
        by: Option<u32>,
    ) -> Result<String> {
        let session = self.get(id)?;
        if session.flag().map(|(s, _)| s) != Some(seq) {
            bail!("session {id} has moved on since: nothing to take");
        }
        let prompts = session.prompts();
        let said = match action {
            StepAction::Leave => format!("left session {id}'s flag for you"),
            StepAction::MergeBase => {
                let base = offer.base.as_deref().unwrap_or("its base branch");
                let checks = self.checks_of(&offer.project);
                session.prompt(&flake::merge_base(&offer.pr, base, checks.as_deref()));
                format!("asked session {id} to merge {base} in")
            }
            StepAction::CiFix => {
                let root = project::root(&offer.project);
                let now = crate::session::now_ms() / 1000;
                let block = self
                    .context
                    .lock()
                    .unwrap()
                    .record_ci_fix(&root, &offer.pr, now, policy::CI_FIX_LIMIT)
                    .context("couldn't count the ci-fix")?;
                let Some(block) = block else {
                    bail!(
                        "ci-fix has had its {} tries on this PR: it's yours now",
                        policy::CI_FIX_LIMIT
                    );
                };
                let checks = self.checks_of(&offer.project);
                session.prompt(&policy::ci_fix_step(
                    checks.as_deref(),
                    &offer.pr,
                    &offer.branch,
                    &offer.failed,
                    block,
                ));
                format!(
                    "asked session {id} to fix CI (try {block} of {})",
                    policy::CI_FIX_LIMIT
                )
            }
            StepAction::Rerun => {
                if self
                    .supervisor
                    .lock()
                    .unwrap()
                    .rerun_spent(&offer.pr, &offer.head)
                {
                    bail!("this commit was rerun once already");
                }
                if !rerunnable(&offer.failed) {
                    bail!("a failed check isn't GitHub Actions, so it can't be rerun");
                }
                let mut runs: Vec<u64> = offer.failed.iter().filter_map(|c| c.run).collect();
                runs.sort_unstable();
                runs.dedup();
                let dir = offer.project.clone();
                let rerun = runs.clone();
                tokio::task::spawn_blocking(move || {
                    for run in rerun {
                        gh(
                            &dir,
                            &["run", "rerun", &run.to_string(), "--failed"],
                            GH_TIMEOUT,
                        )
                        .with_context(|| format!("gh run rerun {run} failed"))?;
                    }
                    Ok::<_, anyhow::Error>(())
                })
                .await??;
                {
                    let mut sup = self.supervisor.lock().unwrap();
                    sup.reruns.insert(offer.pr.clone(), offer.head.clone());
                    sup.save();
                }
                // Nothing for the agent to do: the flag's been dealt with.
                session.mark_seen(seq);
                let runs: Vec<String> = runs.iter().map(u64::to_string).collect();
                format!("reran the failed jobs of run {}", runs.join(", "))
            }
        };
        let trigger = offer.trigger();
        let mut fields = json!({
            "trigger": trigger.as_str(),
            "reason": offer.reason,
            "action": action.as_str(),
            "offered": step.action.as_str(),
            "why": step.why,
            "pr": offer.pr,
            "auto": by.is_some(),
        });
        if let Some(policy) = by {
            fields["policy"] = policy.into();
        }
        session.log_step(fields);
        let root = project::root(&offer.project);
        let now = crate::session::now_ms() / 1000;
        let record = steps::Record {
            at: now,
            session: session.key(),
            trigger,
            offered: step.action,
            taken: action,
            pr: offer.pr.clone(),
            policy: by,
        };
        if let Err(e) = self
            .context
            .lock()
            .unwrap()
            .record_step(&root, &record, now)
        {
            tracing::warn!("couldn't record a supervisor step: {e:#}");
        }
        match by {
            Some(policy) => {
                let taken = Taken {
                    policy,
                    trigger,
                    pr: offer.pr.clone(),
                    root,
                    session: session.key(),
                    prompts,
                    typed: matches!(action, StepAction::MergeBase | StepAction::CiFix),
                    at: Instant::now(),
                };
                self.supervisor.lock().unwrap().taken.insert(id, taken);
            }
            None => self.learn(&root, &session.key(), now),
        }
        Ok(said)
    }

    /// Proposes a policy for each step the user's answers in `root` say to take
    /// without asking (ADR-0019 §3).
    fn learn(&self, root: &Path, session: &str, now: u64) {
        let store = self.context.lock().unwrap();
        let offers = steps::offers(&store.steps(root, now), &store.load(root), now);
        let mut proposed = false;
        for offer in &offers {
            let (title, body) = steps::proposal(offer);
            let new = NewDecision {
                cwd: root.to_owned(),
                title,
                body,
                kind: DecisionKind::Policy,
                propose: true,
                supersedes: None,
                review_every: None,
                session: None,
                scope: None,
            };
            let provenance = Provenance {
                by: "valkyrie".into(),
                session: Some(session.to_owned()),
                conversation: None,
                commit: None,
                cwd: Some(root.to_owned()),
            };
            match store.decide(&new, provenance, now) {
                Ok(d) => {
                    proposed = true;
                    tracing::info!(
                        id = d.id,
                        action = offer.step.action.as_str(),
                        on = offer.step.on.as_str(),
                        "supervisor step policy proposed"
                    );
                }
                Err(e) => tracing::warn!("couldn't propose a supervisor step policy: {e:#}"),
            }
        }
        drop(store);
        if proposed {
            self.refresh_proposals();
        }
    }

    /// Judges the steps policies took on `url`'s sessions by the poll's read of
    /// it, and records each outcome in the project's `policy.jsonl` for `valk
    /// decisions health` (ADR-0013 §6).
    pub(crate) fn judge_steps(&self, url: &str, pr: &Pr) {
        let taken: Vec<(SessionId, Taken)> = self
            .supervisor
            .lock()
            .unwrap()
            .taken
            .iter()
            .filter(|(_, t)| t.pr == url)
            .map(|(id, t)| (*id, t.clone()))
            .collect();
        for (id, t) in taken {
            let judged = match self.get(id) {
                Ok(s) => judge(&t, s.exited(), s.prompts(), s.resting(), pr),
                Err(_) => Judged::Drop,
            };
            let (overridden, how) = match judged {
                Judged::Wait => continue,
                Judged::Drop => {
                    self.supervisor.lock().unwrap().taken.remove(&id);
                    tracing::info!(
                        session = id,
                        policy = t.policy,
                        "a policy's step wasn't judged"
                    );
                    continue;
                }
                Judged::Outcome { overridden, how } => (overridden, how),
            };
            self.supervisor.lock().unwrap().taken.remove(&id);
            let now = crate::session::now_ms() / 1000;
            let record = outcome::Record {
                at: now,
                session: t.session.clone(),
                policy: t.policy,
                scope: "project".into(),
                project: t.root.display().to_string(),
                overridden,
                how: how.to_owned(),
            };
            tracing::info!(
                session = id,
                policy = t.policy,
                overridden,
                how,
                "judged a policy's step"
            );
            if let Err(e) = self
                .context
                .lock()
                .unwrap()
                .record_outcome(&t.root, &record, now)
            {
                tracing::warn!("couldn't record a policy's outcome: {e:#}");
            }
        }
    }

    /// `ClientMsg::Supervise`: whether the supervisor runs; setting it is the
    /// user's.
    pub(crate) fn supervise(&self, on: Option<bool>, peer: Option<Peer>) -> Result<Reply> {
        if let Some(on) = on {
            self.caller(peer)
                .vouch("turning the supervisor on or off")?;
            self.supervisor.lock().unwrap().set_on(on)?;
            self.host.changed.notify_one();
        }
        let on = self.supervisor.lock().unwrap().on();
        Ok(Reply::Text {
            text: if on { "on" } else { "off" }.into(),
        })
    }

    fn drop_offer(&self, id: SessionId) {
        self.supervisor.lock().unwrap().offers.remove(&id);
        self.host.changed.notify_one();
    }

    /// The project's recorded checks command (ADR-0016 §3), for the templates.
    fn checks_of(&self, dir: &Path) -> Option<String> {
        let root = valkyrie_context::project::root(dir);
        let decisions = self.context.lock().unwrap().load(&root);
        policy::checks(&decisions).map(|(c, _)| c)
    }
}

/// Whether the Context setting is on (ADR-0019 §6): off, policies stop, the
/// supervisor's with them. It's kept in `settings.toml`, which the daemon reads as
/// the hooks do, at the moment it matters.
fn context_on() -> bool {
    let off = |v: &str| matches!(v.trim().trim_matches('"'), "false" | "off");
    if let Ok(v) = std::env::var("VALK_PROJECT_CONTEXT") {
        return !off(&v);
    }
    let path = valkyrie_proto::config_dir().join("settings.toml");
    !std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split('#').next()?.split_once('='))
        .any(|(k, v)| k.trim() == "context" && off(v))
}

/// Runs `gh` in `dir`: its stdout, or an error. Killed past `timeout`.
fn gh(dir: &Path, args: &[&str], timeout: Duration) -> Result<String> {
    let mut child = Command::new("gh")
        .no_window()
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn't run gh")?;
    let started = Instant::now();
    // Read as it comes, so a large log can't fill the pipe and stall it.
    let mut out = child.stdout.take().context("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut buf);
        buf
    });
    loop {
        match child.try_wait()? {
            Some(status) => {
                let out = reader.join().unwrap_or_default();
                if !status.success() {
                    let mut err = String::new();
                    if let Some(mut e) = child.stderr.take() {
                        let _ = std::io::Read::read_to_string(&mut e, &mut err);
                    }
                    bail!("{}", err.trim());
                }
                return Ok(String::from_utf8_lossy(&out).into_owned());
            }
            None if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(50)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("gh took too long");
            }
        }
    }
}

/// `owner/repo` from a pull request's URL.
fn repo_of(url: &str) -> Option<String> {
    let path = url.split_once("://")?.1.split_once('/')?.1;
    let mut parts = path.split('/');
    Some(format!("{}/{}", parts.next()?, parts.next()?))
}

/// The job's own name in a check's "workflow / job".
fn job(check: &str) -> &str {
    check.rsplit_once(" / ").map_or(check, |(_, j)| j)
}

/// Reads what the flake call needs about a red pull request from GitHub: the files
/// it changes, each failed job's log, and whether the same check passed on another
/// run of its head commit. What can't be read is left out.
pub(crate) fn gather(dir: &Path, url: &str, head: &str, failed: &[Check]) -> Signals {
    let changed = gh(
        dir,
        &[
            "pr",
            "view",
            url,
            "--json",
            "files",
            "--jq",
            ".files[].path",
        ],
        GH_TIMEOUT,
    )
    .map(|out| out.lines().map(str::to_owned).collect())
    .unwrap_or_default();
    // Every run of every check on the head commit, by job name.
    let passed: Vec<String> = match (repo_of(url), head.is_empty()) {
        (Some(repo), false) => gh(
            dir,
            &[
                "api",
                &format!("repos/{repo}/commits/{head}/check-runs?filter=all&per_page=100"),
                "--jq",
                ".check_runs[] | select(.conclusion == \"success\") | .name",
            ],
            GH_TIMEOUT,
        )
        .map(|out| out.lines().map(str::to_owned).collect())
        .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut logs: HashMap<u64, String> = HashMap::new();
    let failures = failed
        .iter()
        .map(|c| {
            let log = c.run.map_or_else(String::new, |run| {
                let whole = logs
                    .entry(run)
                    .or_insert_with(|| {
                        gh(
                            dir,
                            &["run", "view", &run.to_string(), "--log-failed"],
                            LOG_TIMEOUT,
                        )
                        .unwrap_or_default()
                    })
                    .clone();
                // `--log-failed` lines start with the job's name and a tab.
                let name = job(&c.name);
                let own: Vec<&str> = whole
                    .lines()
                    .filter(|l| l.split('\t').next() == Some(name))
                    .collect();
                let text = if own.is_empty() {
                    whole
                } else {
                    own.join("\n")
                };
                let skip = text.len().saturating_sub(LOG_KEEP);
                let skip = (skip..text.len())
                    .find(|i| text.is_char_boundary(*i))
                    .unwrap_or(0);
                text[skip..].to_owned()
            });
            Failure {
                check: c.name.clone(),
                run: c.run,
                passed_before: passed.iter().any(|p| p == job(&c.name)),
                log,
            }
        })
        .collect();
    Signals { failures, changed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicting_merges_the_base_first_and_red_follows_the_rules() {
        let step = |c, v: Verdict, spent| pick(c, &v, spent).map(|s| (s.action, s.alt));
        assert_eq!(
            step(true, Verdict::CiFix("x".into()), false),
            Some((StepAction::MergeBase, None))
        );
        assert_eq!(
            step(false, Verdict::Rerun("#17".into()), false),
            Some((StepAction::Rerun, Some(StepAction::CiFix)))
        );
        // Once per head: a flake on a commit already rerun is the agent's.
        assert_eq!(
            step(false, Verdict::Rerun("#17".into()), true),
            Some((StepAction::CiFix, None))
        );
        assert_eq!(
            step(false, Verdict::CiFix("real".into()), false),
            Some((StepAction::CiFix, None))
        );
        assert_eq!(step(false, Verdict::Unsure, false), None);
    }

    #[test]
    fn a_policys_step_is_handled_when_the_next_settled_read_is_clean() {
        let taken = |trigger, typed| Taken {
            policy: 7,
            trigger,
            pr: "u".into(),
            root: PathBuf::from("/r"),
            session: "s".into(),
            prompts: 4,
            typed,
            at: Instant::now(),
        };
        let open = |conflicts: bool, red: bool, settled: bool| Pr::Open {
            url: "u".into(),
            failed: if red {
                vec![Check {
                    name: "ci".into(),
                    url: String::new(),
                    run: Some(1),
                }]
            } else {
                vec![]
            },
            conflicts: conflicts.then(|| "main".into()),
            head: "h".into(),
            settled,
        };
        let handled = Judged::Outcome {
            overridden: false,
            how: "watcher",
        };
        let merge = taken(Trigger::Conflicting, true);
        // Its own prompt not heard yet, or the agent still at work: wait.
        assert_eq!(
            judge(&merge, false, 4, true, &open(false, false, true)),
            Judged::Wait
        );
        assert_eq!(
            judge(&merge, false, 5, false, &open(false, false, true)),
            Judged::Wait
        );
        // Back at rest: GitHub still working out mergeability, then clean.
        assert_eq!(
            judge(&merge, false, 5, true, &open(false, false, false)),
            Judged::Wait
        );
        assert_eq!(
            judge(&merge, false, 5, true, &open(false, true, true)),
            handled
        );
        // Still conflicting: not handled, nothing to record.
        assert_eq!(
            judge(&merge, false, 5, true, &open(true, false, true)),
            Judged::Drop
        );
        // The user prompted it first.
        assert_eq!(
            judge(&merge, false, 6, false, &open(false, false, true)),
            Judged::Outcome {
                overridden: true,
                how: "prompt"
            }
        );
        assert_eq!(
            judge(&merge, true, 5, true, &open(false, false, true)),
            Judged::Drop
        );
        // A rerun types nothing: any prompt is the user's; red is judged once CI ran.
        let rerun = taken(Trigger::Red, false);
        assert_eq!(
            judge(&rerun, false, 4, true, &open(false, false, false)),
            Judged::Wait
        );
        assert_eq!(
            judge(&rerun, false, 4, true, &open(true, false, true)),
            handled
        );
        assert_eq!(
            judge(&rerun, false, 4, true, &open(false, true, true)),
            Judged::Drop
        );
        assert!(matches!(
            judge(&rerun, false, 5, true, &open(false, false, true)),
            Judged::Outcome {
                overridden: true,
                ..
            }
        ));
        assert!(matches!(
            judge(&rerun, false, 4, true, &Pr::Merged { url: "u".into() }),
            Judged::Outcome {
                overridden: false,
                ..
            }
        ));
    }

    /// Two sessions take merge-base; the policy proposed for it is accepted; the
    /// next conflict merges the base in without asking, and the poll judges it.
    #[cfg(unix)]
    #[tokio::test]
    async fn steps_taken_twice_become_a_policy_that_runs_and_is_measured() {
        use crate::context::{AS_CALLER, Caller};
        use crate::work::PrLookup;
        use std::sync::Arc;
        use valkyrie_proto::{DecisionStatus, ItemChange, NewItem, ReviewAction, Size, SpawnSpec};
        let dir = std::env::temp_dir().join(format!("valkyrie-learn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Steps are learned per repository.
        let git = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status();
        assert!(git.unwrap().success());
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
        registry.supervisor.lock().unwrap().context_on = Some(|| true);
        let spawn = || {
            let spec = SpawnSpec {
                command: vec!["sleep".into(), "30".into()],
                cwd: Some(dir.clone()),
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
        let url = |n: u32| format!("https://github.com/o/r/pull/{n}");
        let with_pr = |session: &Arc<crate::session::Session>, n: u32| {
            let b = registry.backlog.lock().unwrap();
            let new = NewItem {
                title: format!("item {n}"),
                cwd: Some(dir.clone()),
                ..Default::default()
            };
            let item = b.add(&new, "human", 1).unwrap();
            let open = ItemChange::Status {
                status: ItemStatus::Open,
            };
            b.change(item.id, &open, 1).unwrap();
            let id = session.info().id;
            b.taken(
                item.id,
                id,
                registry.boot,
                &[],
                None,
                Some(&url(n)),
                None,
                1,
            )
            .unwrap();
        };
        let (a, b, c) = (spawn(), spawn(), spawn());
        with_pr(&a, 1);
        with_pr(&b, 2);
        let conflicting: PrLookup = Arc::new(|_, pr| Pr::Open {
            url: pr.to_owned(),
            failed: vec![],
            conflicts: Some("main".into()),
            head: "h1".into(),
            settled: true,
        });
        AS_CALLER.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(|| Caller {
                agent: None,
                session: None,
                bridge: false,
            }))
        });
        registry.ship_merged_prs(conflicting.clone()).await;
        let root = project::root(&dir);
        let policies = || {
            registry
                .context
                .lock()
                .unwrap()
                .load(&root)
                .into_iter()
                .filter(|d| d.kind == DecisionKind::Policy)
                .collect::<Vec<_>>()
        };
        for (i, s) in [&a, &b].into_iter().enumerate() {
            let flag = registry.flag_of(s, s.info().id).unwrap();
            assert_eq!(flag.step.as_ref().unwrap().action, StepAction::MergeBase);
            assert!(policies().is_empty(), "proposed after {i} answers");
            registry
                .take_step(s.info().id, flag.seq, StepAction::MergeBase, None)
                .await
                .unwrap();
        }
        let now = crate::session::now_ms() / 1000;
        let answers = registry.context.lock().unwrap().steps(&root, now);
        assert_eq!(answers.len(), 2);
        assert!(answers.iter().all(|r| r.accepted() && r.policy.is_none()));
        // Two sessions: proposed, for the user to accept.
        let proposed = policies();
        assert_eq!(proposed.len(), 1);
        assert_eq!(proposed[0].status, DecisionStatus::Proposed);
        assert!(
            proposed[0]
                .body
                .starts_with("template: merge-base\non: conflicting")
        );
        let id = proposed[0].id;
        registry
            .context
            .lock()
            .unwrap()
            .review(&root, id, &ReviewAction::Accept, None, None, now)
            .unwrap();

        // The next conflict merges the base in on its own, and says so.
        with_pr(&c, 3);
        registry.ship_merged_prs(conflicting.clone()).await;
        let cid = c.info().id;
        let flag = registry.flag_of(&c, cid).unwrap();
        assert!(flag.step.is_none() && !flag.working, "{flag:?}");
        let answers = registry.context.lock().unwrap().steps(&root, now + 1);
        let auto = answers.last().unwrap();
        assert_eq!(
            (auto.taken, auto.policy, auto.pr.as_str()),
            (StepAction::MergeBase, Some(id), url(3).as_str())
        );
        let log = std::fs::read_to_string(
            dir.join("state/sessions")
                .join(format!("{}.events.jsonl", c.key())),
        )
        .unwrap();
        let line = log
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|v| v["k"] == "supervisor")
            .unwrap();
        assert_eq!(line["auto"], true);
        assert_eq!(line["policy"], id);
        assert_eq!(line["trigger"], "conflicting");
        // Once per head: the same conflict on the same commit isn't merged again.
        registry.ship_merged_prs(conflicting.clone()).await;
        assert_eq!(
            registry.context.lock().unwrap().steps(&root, now + 1).len(),
            3
        );

        // The agent takes the step's prompt and stops; the next settled read is
        // clean: handled.
        let hook = |event: &str| {
            c.hook(
                "claude",
                1,
                json!({"hook_event_name": event, "prompt": "merge main in"}),
            );
        };
        hook("UserPromptSubmit");
        let clean: PrLookup = Arc::new(|_, pr| Pr::Open {
            url: pr.to_owned(),
            failed: vec![],
            conflicts: None,
            head: "h2".into(),
            settled: true,
        });
        registry.ship_merged_prs(clean.clone()).await;
        assert!(
            registry
                .context
                .lock()
                .unwrap()
                .outcomes(&root, now + 1)
                .is_empty()
        );
        hook("Stop");
        registry.ship_merged_prs(clean).await;
        let outcomes = registry.context.lock().unwrap().outcomes(&root, now + 1);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            (
                outcomes[0].policy,
                outcomes[0].overridden,
                outcomes[0].how.as_str()
            ),
            (id, false, "watcher")
        );

        // Off, the supervisor offers nothing; flags still show.
        registry.supervise(Some(false), None).unwrap();
        let d = spawn();
        with_pr(&d, 4);
        registry.ship_merged_prs(conflicting).await;
        let flag = registry.flag_of(&d, d.info().id).unwrap();
        assert!(flag.step.is_none() && !flag.working);
        registry.supervise(Some(true), None).unwrap();
        AS_CALLER.with(|cell| *cell.borrow_mut() = None);

        for s in [a, b, c, d] {
            s.kill();
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_the_repo_and_job_from_github_names() {
        assert_eq!(
            repo_of("https://github.com/bjschnell/Valkyrie/pull/115").as_deref(),
            Some("bjschnell/Valkyrie")
        );
        assert_eq!(repo_of("nope"), None);
        assert_eq!(job("ci / windows-latest"), "windows-latest");
        assert_eq!(job("lint"), "lint");
    }
}

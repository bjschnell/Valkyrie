//! Supervisor proposals (ADR-0019 §2): for each flag the PR poll raises on an idle
//! session, one step from a fixed list, offered on the session's queue row until
//! the user takes it or the session moves on. A conflicting pull request gets
//! `merge-base`; a red one gets `rerun` when every failed check is a known flake,
//! else `ci-fix` (the flake call: `valkyrie_context::flake`'s rules, then the
//! model for what they can't place, never with `valk decisions auto off`). `leave`
//! drops the offer and keeps the flag.
//!
//! Every step is the user's tap for now: nothing here runs one on its own, and
//! nothing reaches an agent but a fixed template.

use crate::Registry;
use crate::context::Peer;
use crate::work::{Check, GH_TIMEOUT, Pr};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use valkyrie_context::flake::{self, Bug, Failure, Verdict};
use valkyrie_context::policy;
use valkyrie_proto::{
    AutoMode, Flag, Item, ItemKind, ItemStatus, NoWindow, Reply, SessionId, Step, StepAction,
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

/// The offers, the reruns done and the model's budget.
#[derive(Default)]
pub(crate) struct Supervisor {
    offers: HashMap<SessionId, Offer>,
    /// The PR head each pull request was last rerun at: once per head.
    reruns: HashMap<String, String>,
    path: PathBuf,
    calls: VecDeque<Instant>,
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
        } = pr
        else {
            return;
        };
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
        let Some(step) = &offer.step else {
            bail!("the supervisor is still working out its step for session {id}");
        };
        if action != step.action && Some(action) != step.alt && action != StepAction::Leave {
            bail!("{} isn't offered for session {id}", action.as_str());
        }
        let said = match action {
            StepAction::Leave => {
                self.drop_offer(id);
                format!("left session {id}'s flag for you")
            }
            StepAction::MergeBase => {
                let base = offer.base.as_deref().unwrap_or("its base branch");
                let checks = self.checks_of(&offer.project);
                session.prompt(&flake::merge_base(&offer.pr, base, checks.as_deref()));
                self.drop_offer(id);
                format!("asked session {id} to merge {base} in")
            }
            StepAction::CiFix => {
                let root = valkyrie_context::project::root(&offer.project);
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
                self.drop_offer(id);
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
                self.drop_offer(id);
                let runs: Vec<String> = runs.iter().map(u64::to_string).collect();
                format!("reran the failed jobs of run {}", runs.join(", "))
            }
        };
        session.log_step(json!({
            "trigger": offer.reason,
            "action": action.as_str(),
            "offered": step.action.as_str(),
            "why": step.why,
            "pr": offer.pr,
        }));
        tracing::info!(
            session = id,
            action = action.as_str(),
            pr = offer.pr,
            "took a supervisor step"
        );
        Ok(Reply::StepTaken { said })
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

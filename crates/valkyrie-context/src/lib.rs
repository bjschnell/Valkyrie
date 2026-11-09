//! The project context layer (DESIGN §6, ADR-0007): decisions kept per project as
//! markdown files in the state dir, their review lifecycle, and the block injected
//! into agents.

pub mod backlog;
pub mod blocker;
pub mod extract;
pub mod facts;
mod file;
pub mod find;
pub mod flake;
pub mod flaketree;
pub mod handoff;
pub mod inject;
pub mod mine;
pub mod model;
pub mod order;
pub mod outcome;
pub mod policy;
pub mod project;
pub mod reply;
pub mod retro;
pub mod scope;
pub mod served;
pub mod skill;
pub mod stale;
pub mod steps;

use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use valkyrie_proto::{
    Decision, DecisionKind, DecisionStatus, NewDecision, Provenance, ReviewAction,
};

pub use file::{name as file_name, parse, render};

/// Longest title kept; titles are one line in every list.
pub const MAX_TITLE: usize = 200;
/// Longest body kept; a decision is a few sentences, not a design doc.
pub const MAX_BODY: usize = 4000;
/// A skill's body holds a procedure: steps, pitfalls and how to verify.
pub const MAX_SKILL_BODY: usize = 12_000;

/// How long a failure counts (ADR-0011 §2).
pub const FAILURE_DAYS: u64 = 90;
/// Size past which `failures.jsonl` drops what no longer counts.
const FAILURES_COMPACT: u64 = 1 << 20;
/// In how many sessions a decision's failure happens after it's accepted before
/// health says it isn't working.
pub const RECURRED: usize = 2;

/// How a project's skills are used (ADR-0011 §6), so one nobody needs is found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SkillUse {
    /// When this started being recorded: nothing before it is known.
    pub since: u64,
    /// When each skill was last read by an agent.
    pub reads: BTreeMap<u32, u64>,
    /// Each session's (conversation's) last turn in the project.
    pub sessions: BTreeMap<String, u64>,
    /// When an idle skill was first asked about, by id and the `updated` it had.
    pub flagged: BTreeMap<(u32, u64), u64>,
    /// The approvals a skill was already learned from (their transcript ids).
    pub learned: Vec<String>,
}

impl SkillUse {
    /// Sessions with a turn after `at`.
    pub fn sessions_since(&self, at: u64) -> usize {
        self.sessions.values().filter(|t| **t > at).count()
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "since": self.since,
            "reads": self.reads.iter().map(|(k, v)| (k.to_string(), *v)).collect::<BTreeMap<_, _>>(),
            "sessions": self.sessions,
            "flagged": self.flagged.iter().map(|((id, u), at)| (format!("{id}:{u}"), *at)).collect::<BTreeMap<_, _>>(),
            "learned": self.learned,
        })
    }

    fn from_json(v: &serde_json::Value) -> Self {
        let map = |key: &str| -> Vec<(String, u64)> {
            v[key]
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_u64()?)))
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            since: v["since"].as_u64().unwrap_or(0),
            reads: map("reads")
                .into_iter()
                .filter_map(|(k, at)| Some((k.parse().ok()?, at)))
                .collect(),
            sessions: map("sessions").into_iter().collect(),
            flagged: map("flagged")
                .into_iter()
                .filter_map(|(k, at)| {
                    let (id, u) = k.split_once(':')?;
                    Some(((id.parse().ok()?, u.parse().ok()?), at))
                })
                .collect(),
            learned: v["learned"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// What a project's sessions' failures have been (ADR-0011 §2).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Failures {
    /// How often each signature was seen.
    pub seen: HashMap<u64, usize>,
    /// The failed calls recorded, so none counts twice (a restart, a resume).
    pub calls: HashSet<String>,
}

/// Every project's decisions, under one directory (`<state dir>/context`).
pub struct Store {
    base: PathBuf,
}

impl Store {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// `<state dir>/context`.
    pub fn default_base() -> PathBuf {
        valkyrie_proto::state_dir().join("context")
    }

    fn dir(&self, root: &Path) -> PathBuf {
        self.base.join(project::key(root)).join("decisions")
    }

    /// Every project that has decisions.
    pub fn projects(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.base) else {
            return Vec::new();
        };
        let mut roots: Vec<PathBuf> = entries
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path().join("root")).ok())
            .map(|r| PathBuf::from(r.trim_end_matches('\n')))
            .collect();
        roots.sort();
        roots
    }

    /// A project's decisions by id. Files that don't parse are skipped, so one bad
    /// hand edit can't hide the rest.
    pub fn load(&self, root: &Path) -> Vec<Decision> {
        self.files(root).into_values().map(|(_, d)| d).collect()
    }

    fn files(&self, root: &Path) -> BTreeMap<u32, (PathBuf, Decision)> {
        let mut out = BTreeMap::new();
        let Ok(entries) = std::fs::read_dir(self.dir(root)) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "md") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(d) = file::parse(&text, root.to_path_buf()) {
                out.insert(d.id, (path, d));
            }
        }
        out
    }

    /// Every project's decisions waiting on review, oldest first.
    pub fn proposals(&self) -> Vec<Decision> {
        let mut all: Vec<Decision> = self
            .projects()
            .iter()
            .flat_map(|root| self.load(root))
            .filter(|d| {
                d.status == DecisionStatus::Proposed
                    || (d.status == DecisionStatus::Active && d.proposed_scope.is_some())
            })
            .collect();
        all.sort_by_key(|d| (d.created, d.id));
        all
    }

    /// Records a decision: `active` when a human asks for it outright, else
    /// `proposed`. `provenance.by` must already be set by the caller (the daemon,
    /// which knows whether the session runs an agent).
    pub fn decide(&self, new: &NewDecision, provenance: Provenance, now: u64) -> Result<Decision> {
        let root = project::root(&new.cwd);
        let (title, body) = clean(&new.title, &new.body, new.kind, &root)?;
        let files = self.files(&root);
        let by_human = provenance.by == "human";
        if let Some(old) = new.supersedes {
            match files.get(&old) {
                Some((_, d)) if d.status == DecisionStatus::Active => {}
                Some((_, d)) => bail!("#{old} is {}, not active", d.status.as_str()),
                None => bail!("no decision #{old} in {}", root.display()),
            }
            // One proposed replacement at a time: two, accepted in turn, would
            // leave two active (a skill rewritten twice from the same version).
            let proposing = !by_human || new.propose;
            if proposing
                && let Some((_, p)) = files.values().find(|(_, p)| {
                    p.status == DecisionStatus::Proposed && p.supersedes == Some(old)
                })
            {
                bail!(
                    "#{} already proposes replacing #{old}; review it first",
                    p.id
                );
            }
        }
        let mut provenance = provenance;
        provenance.commit = provenance.commit.filter(|c| stale::valid_commit(c));
        let text = format!("{title}\n{body}");
        let anchors = stale::anchors(&root, &new.cwd, &text);
        // Habits act in every project, so they have no paths of their own.
        let scope = match &new.scope {
            _ if project::is_user(&root) => Vec::new(),
            Some(scope) => scope::clean(scope)?,
            // A rewrite applies where what it replaces did.
            None => new
                .supersedes
                .and_then(|old| files.get(&old))
                .map(|(_, old)| old.scope.clone())
                .filter(|scope| !scope.is_empty())
                .unwrap_or_else(|| scope::from_anchors(&anchors)),
        };
        let fresh = valkyrie_proto::Freshness {
            anchors,
            review_every: new
                .review_every
                .or_else(|| stale::volatile(&text).then_some(stale::VOLATILE_DAYS)),
            ..Default::default()
        };
        let mut decision = Decision {
            id: self.next_id(&root),
            project: root.clone(),
            title,
            body,
            kind: new.kind,
            status: if by_human && !new.propose {
                DecisionStatus::Active
            } else {
                DecisionStatus::Proposed
            },
            created: now,
            updated: now,
            supersedes: new.supersedes,
            superseded_by: None,
            pinned: false,
            scope,
            proposed_scope: None,
            scope_asked: true,
            fresh,
            provenance,
        };
        self.mark_project(&root)?;
        // The new one first: a failure between the two writes must not leave the
        // old one pointing at a decision that doesn't exist.
        self.write(&root, &decision, None)?;
        if decision.status == DecisionStatus::Active
            && let Some(old) = decision.supersedes
        {
            decision.pinned |= self.supersede(&root, old, decision.id, now)?;
        }
        Ok(decision)
    }

    pub fn review(
        &self,
        root: &Path,
        id: u32,
        action: &ReviewAction,
        seen: Option<u64>,
        head: Option<String>,
        now: u64,
    ) -> Result<Decision> {
        use DecisionStatus::*;
        let files = self.files(root);
        let Some((path, d)) = files.get(&id) else {
            bail!("no decision #{id} in {}", root.display());
        };
        if seen.is_some_and(|seen| seen != d.updated) {
            bail!("#{id} changed since you read it; look at it again");
        }
        let mut d = d.clone();
        // What it replaces was replaced by another meanwhile: accepting this too
        // would make two active versions.
        if matches!(action, ReviewAction::Accept | ReviewAction::Revise { .. })
            && let Some(old) = d.supersedes
            && let Some((_, o)) = files.get(&old)
            && o.status == Superseded
        {
            bail!(
                "#{id} replaces #{old}, which #{} already replaced; reject it, or propose it again against #{0}",
                o.superseded_by.unwrap_or(0)
            );
        }
        let mut accepting = false;
        let mut confirming = false;
        let mut reworded = false;
        // A pin changes where it's shown, not what it says: it isn't a
        // confirmation, so `updated` (which stands in for one) stays.
        // Nor is where it applies (ADR-0014 §2).
        let pinning = matches!(
            action,
            ReviewAction::Pin
                | ReviewAction::Unpin
                | ReviewAction::Scope { .. }
                | ReviewAction::ProposeScope { .. }
                | ReviewAction::DeclineScope
        );
        let scoping = match action {
            ReviewAction::ProposeScope { .. } => true,
            ReviewAction::Scope { scope }
            | ReviewAction::Edit {
                scope: Some(scope), ..
            }
            | ReviewAction::Revise {
                scope: Some(scope), ..
            } => !scope.is_empty(),
            _ => false,
        };
        if scoping && project::is_user(root) {
            bail!("#{id} is a habit: habits hold in every project, so they have no scope");
        }
        match (action, d.status) {
            (ReviewAction::Accept, Proposed) => accepting = true,
            (ReviewAction::Reject, Proposed) => d.status = Rejected,
            (ReviewAction::Retire, Active) => d.status = Retired,
            (ReviewAction::Confirm, Active) => confirming = true,
            (ReviewAction::Pin | ReviewAction::Unpin, Active) => {
                // A skill is an index line read when needed, and a policy's body
                // is settings for its hook: neither has a body to put in the block.
                if matches!(d.kind, DecisionKind::Skill | DecisionKind::Policy) {
                    bail!("#{id} is a {}: it isn't pinned", d.kind.as_str());
                }
                // Habits are listed by title in every project, never in full.
                if project::is_user(root) {
                    bail!("#{id} is a habit: habits aren't pinned");
                }
                d.pinned = matches!(action, ReviewAction::Pin);
            }
            (
                ReviewAction::Edit {
                    title,
                    body,
                    kind,
                    scope,
                },
                Proposed | Active,
            ) => {
                (d.title, d.body) = clean(title, body, *kind, root)?;
                d.kind = *kind;
                if let Some(scope) = scope {
                    set_scope(&mut d, scope::clean(scope)?);
                }
                // Neither has a body for the block (see Pin below).
                if matches!(d.kind, DecisionKind::Skill | DecisionKind::Policy) {
                    d.pinned = false;
                }
                reworded = true;
            }
            (
                ReviewAction::Revise {
                    title,
                    body,
                    kind,
                    scope,
                },
                Proposed,
            ) => {
                (d.title, d.body) = clean(title, body, *kind, root)?;
                d.kind = *kind;
                if let Some(scope) = scope {
                    set_scope(&mut d, scope::clean(scope)?);
                }
                accepting = true;
                reworded = true;
            }
            (ReviewAction::Scope { scope }, Proposed | Active) => {
                set_scope(&mut d, scope::clean(scope)?);
            }
            (ReviewAction::ProposeScope { scope }, Active) => {
                if d.scope_asked || !d.scope.is_empty() || d.proposed_scope.is_some() {
                    bail!(
                        "#{id} has had its scope settled; set one with `valk decisions scope {id} GLOB…`"
                    );
                }
                let scope = scope::clean(scope)?;
                d.proposed_scope = (!scope.is_empty()).then_some(scope);
                d.scope_asked = true;
            }
            (ReviewAction::DeclineScope, Active) if d.proposed_scope.is_some() => {
                d.proposed_scope = None;
            }
            (ReviewAction::DeclineScope, Active) => bail!("#{id} has no proposed scope"),
            (action, status) => bail!(
                "can't {} #{id}: it is {}",
                match action {
                    ReviewAction::Accept => "accept",
                    ReviewAction::Reject => "reject",
                    ReviewAction::Retire => "retire",
                    ReviewAction::Confirm => "confirm",
                    ReviewAction::Pin => "pin",
                    ReviewAction::Unpin => "unpin",
                    ReviewAction::Edit { .. } => "edit",
                    ReviewAction::Revise { .. } => "revise",
                    ReviewAction::Scope { .. } => "scope",
                    ReviewAction::ProposeScope { .. } => "propose a scope for",
                    ReviewAction::DeclineScope => "decline the scope of",
                },
                status.as_str()
            ),
        }
        if accepting {
            d.status = Active;
        }
        if reworded {
            let cwd = d
                .provenance
                .cwd
                .clone()
                .unwrap_or_else(|| root.to_path_buf());
            d.fresh.anchors = stale::anchors(root, &cwd, &format!("{}\n{}", d.title, d.body));
        }
        // Confirmed as of now, and at this commit (`head`, the project's `HEAD`): its
        // files' churn counts from here. Files gone since aren't its anchors any more.
        if accepting || confirming {
            d.fresh.confirmed = now;
            if let Some(head) = head.filter(|h| stale::valid_commit(h)) {
                d.provenance.commit = Some(head);
            }
            d.fresh.anchors.retain(|a| root.join(a).exists());
        }
        if !pinning {
            d.updated = now;
        }
        self.write(root, &d, Some(path))?;
        // The one it replaces may have been retired meanwhile; then there's
        // nothing left to supersede.
        if accepting
            && let Some(old) = d.supersedes
            && files.get(&old).is_some_and(|(_, o)| o.status == Active)
        {
            d.pinned |= self.supersede(root, old, id, now)?;
        }
        Ok(d)
    }

    /// Marks `old` replaced by `by`; whether `by` took over its pin.
    fn supersede(&self, root: &Path, old: u32, by: u32, now: u64) -> Result<bool> {
        let files = self.files(root);
        let Some((path, d)) = files.get(&old) else {
            return Ok(false);
        };
        let mut d = d.clone();
        d.status = DecisionStatus::Superseded;
        d.superseded_by = Some(by);
        d.updated = now;
        self.write(root, &d, Some(path))?;
        // What replaces a pinned decision keeps the pin: you pinned the rule,
        // not its wording.
        if d.pinned
            && let Some((path, new)) = files.get(&by)
            && !matches!(new.kind, DecisionKind::Skill | DecisionKind::Policy)
        {
            let mut new = new.clone();
            new.pinned = true;
            self.write(root, &new, Some(path))?;
            return Ok(true);
        }
        Ok(false)
    }

    /// One past the highest number on any decision file, parsed or not, so a file
    /// broken by a hand edit never has its id given out again.
    fn next_id(&self, root: &Path) -> u32 {
        let Ok(entries) = std::fs::read_dir(self.dir(root)) else {
            return 1;
        };
        entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
                digits.parse::<u32>().ok()
            })
            .max()
            .map_or(1, |id| id + 1)
    }

    /// Writes atomically, then removes the old file if the title (so the name)
    /// changed.
    fn write(&self, root: &Path, d: &Decision, old: Option<&PathBuf>) -> Result<()> {
        let dir = self.dir(root);
        valkyrie_proto::ensure_private_dir(&dir)?;
        let path = dir.join(file::name(d));
        write_atomic(&path, &file::render(d))?;
        if let Some(old) = old
            && *old != path
        {
            let _ = std::fs::remove_file(old);
        }
        Ok(())
    }

    /// What `root`'s sessions' failures have been (ADR-0011 §2) over the last
    /// [`FAILURE_DAYS`] days.
    pub fn failures(&self, root: &Path, now: u64) -> Failures {
        let mut out = Failures::default();
        let text = std::fs::read_to_string(self.failures_file(root)).unwrap_or_default();
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            let at = v["at"].as_u64().unwrap_or(0);
            let Some(sig) = v["sig"]
                .as_str()
                .and_then(|s| u64::from_str_radix(s, 16).ok())
            else {
                continue;
            };
            if now.saturating_sub(at) > FAILURE_DAYS * 86_400 {
                continue;
            }
            // A proposal's line has no call, so it isn't a failure.
            if let Some(call) = v["call"].as_str()
                && out.calls.insert(call.to_owned())
            {
                *out.seen.entry(sig).or_insert(0) += 1;
            }
        }
        out
    }

    /// Records failed calls (`(signature, call id)`) seen in `root`'s sessions.
    pub fn record_failures(
        &self,
        root: &Path,
        seen: &[(u64, &str)],
        session: Option<&str>,
        now: u64,
    ) -> Result<()> {
        let lines: Vec<String> = seen
            .iter()
            .map(|(sig, call)| {
                let mut v =
                    serde_json::json!({"sig": format!("{sig:016x}"), "at": now, "call": call});
                if let Some(session) = session {
                    v["session"] = session.into();
                }
                v.to_string()
            })
            .collect();
        self.append_failures(root, &lines, now)
    }

    /// Records that decision `id` was proposed from failures with signature `sig`.
    pub fn record_proposal(&self, root: &Path, sig: u64, id: u32, now: u64) -> Result<()> {
        let line = serde_json::json!({"sig": format!("{sig:016x}"), "at": now, "decision": id})
            .to_string();
        self.append_failures(root, &[line], now)
    }

    /// Active decisions whose failure kept happening after they were accepted
    /// (ADR-0011 §6): each with in how many sessions since. A session started
    /// before the acceptance never got it, so it takes [`RECURRED`] sessions.
    pub fn recurred(&self, root: &Path, decisions: &[Decision]) -> Vec<(u32, usize)> {
        let text = std::fs::read_to_string(self.failures_file(root)).unwrap_or_default();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        let mut out = Vec::new();
        for d in decisions
            .iter()
            .filter(|d| d.status == DecisionStatus::Active)
        {
            let Some(sig) = lines
                .iter()
                .find(|v| v["decision"].as_u64() == Some(u64::from(d.id)))
                .and_then(|v| v["sig"].as_str())
            else {
                continue;
            };
            let since = stale::confirmed(d);
            // A line without its session (an older one) counts as its own.
            let times = lines
                .iter()
                .filter(|v| {
                    v["call"].is_string()
                        && v["sig"].as_str() == Some(sig)
                        && v["at"].as_u64().is_some_and(|at| at > since)
                })
                .map(|v| v["session"].as_str().or(v["call"].as_str()))
                .collect::<HashSet<_>>()
                .len();
            if times >= RECURRED {
                out.push((d.id, times));
            }
        }
        out
    }

    /// How `root`'s skills are used (ADR-0011 §6); empty before anything was
    /// recorded.
    pub fn skill_use(&self, root: &Path) -> SkillUse {
        let path = self.base.join(project::key(root)).join("skill-use.json");
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .map(|v| SkillUse::from_json(&v))
            .unwrap_or_default()
    }

    /// Changes how `root`'s skills are used with `f`, from `now` on if nothing
    /// was recorded before. Sessions quiet for [`FAILURE_DAYS`] are forgotten.
    pub fn update_skill_use(
        &self,
        root: &Path,
        now: u64,
        f: impl FnOnce(&mut SkillUse),
    ) -> Result<()> {
        let mut used = self.skill_use(root);
        if used.since == 0 {
            used.since = now;
        }
        f(&mut used);
        used.sessions
            .retain(|_, at| now.saturating_sub(*at) <= FAILURE_DAYS * 86_400);
        let keep = used.learned.len().saturating_sub(100);
        used.learned.drain(..keep);
        self.mark_project(root)?;
        let path = self.base.join(project::key(root)).join("skill-use.json");
        write_atomic(&path, &format!("{}\n", used.to_json()))
    }

    /// How many times `ci-fix` has blocked on the pull request at `pr`, as
    /// [`Store::record_ci_fix`] counts them at `now`.
    pub fn ci_fix_blocks(&self, root: &Path, pr: &str, now: u64) -> u32 {
        let entry = &self.ci_fixes(root)[pr];
        let at = entry["at"].as_u64().unwrap_or(0);
        if now.saturating_sub(at) > FAILURE_DAYS * 86_400 {
            return 0;
        }
        entry["blocks"].as_u64().unwrap_or(0) as u32
    }

    /// Counts one more `ci-fix` block on the pull request at `pr` (ADR-0013 §3)
    /// and returns the new count, or `None` when it has already been blocked
    /// `limit` times. Pull requests not blocked for [`FAILURE_DAYS`] are forgotten,
    /// so one left at the limit starts over then. Two sessions stopping on one pull
    /// request in the same instant can each count from the same number: at worst
    /// one block too many, never a loop.
    pub fn record_ci_fix(
        &self,
        root: &Path,
        pr: &str,
        now: u64,
        limit: u32,
    ) -> Result<Option<u32>> {
        let mut all = self.ci_fixes(root);
        let map = all.as_object_mut().context("ci-fix.json isn't an object")?;
        map.retain(|_, v| {
            now.saturating_sub(v["at"].as_u64().unwrap_or(0)) <= FAILURE_DAYS * 86_400
        });
        let blocks = map.get(pr).and_then(|v| v["blocks"].as_u64()).unwrap_or(0) as u32;
        if blocks >= limit {
            return Ok(None);
        }
        map.insert(
            pr.to_owned(),
            serde_json::json!({"blocks": blocks + 1, "at": now}),
        );
        self.mark_project(root)?;
        let path = self.base.join(project::key(root)).join("ci-fix.json");
        write_atomic(&path, &format!("{all}\n"))?;
        Ok(Some(blocks + 1))
    }

    /// Counts one `ci: agent` block on the pull request at `pr` when its branch is
    /// at `head` (ADR-0016 §5) and returns the new count, or `None` when this head
    /// was already blocked or the pull request has been blocked `limit` times. Kept
    /// apart from [`Store::record_ci_fix`]: a different mode, its own count.
    pub fn record_ci_agent(
        &self,
        root: &Path,
        pr: &str,
        head: &str,
        now: u64,
        limit: u32,
    ) -> Result<Option<u32>> {
        let path = self.base.join(project::key(root)).join("ci-agent.json");
        let mut all = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
        let map = all
            .as_object_mut()
            .context("ci-agent.json isn't an object")?;
        map.retain(|_, v| {
            now.saturating_sub(v["at"].as_u64().unwrap_or(0)) <= FAILURE_DAYS * 86_400
        });
        let entry = map.get(pr);
        let blocks = entry.and_then(|v| v["blocks"].as_u64()).unwrap_or(0) as u32;
        let mut heads: Vec<String> = entry
            .and_then(|v| v["heads"].as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|h| h.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if blocks >= limit || heads.iter().any(|h| h == head) {
            return Ok(None);
        }
        heads.push(head.to_owned());
        map.insert(
            pr.to_owned(),
            serde_json::json!({"blocks": blocks + 1, "heads": heads, "at": now}),
        );
        self.mark_project(root)?;
        write_atomic(&path, &format!("{all}\n"))?;
        Ok(Some(blocks + 1))
    }

    fn ci_fixes(&self, root: &Path) -> serde_json::Value {
        let path = self.base.join(project::key(root)).join("ci-fix.json");
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}))
    }

    /// Records a reply to a stop short of shipping (ADR-0013 §2). Kept in the
    /// user scope: habits are learned across repositories (ADR-0016 §2). Past
    /// [`FAILURES_COMPACT`] bytes, what no longer counts is dropped first.
    pub fn record_reply(&self, record: &reply::Record, now: u64) -> Result<()> {
        use std::io::Write;
        let user = project::user();
        self.mark_project(&user)?;
        let path = self.replies_file();
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > FAILURES_COMPACT) {
            let kept: String = self
                .replies(now)
                .iter()
                .map(|r| format!("{}\n", r.to_json()))
                .collect();
            write_atomic(&path, &kept)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        writeln!(file, "{}", record.to_json())?;
        Ok(())
    }

    /// The replies recorded over the last [`reply::REPLY_DAYS`] days, oldest first.
    pub fn replies(&self, now: u64) -> Vec<reply::Record> {
        std::fs::read_to_string(self.replies_file())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|v| reply::Record::from_json(&v))
            .filter(|r| now.saturating_sub(r.at) <= reply::REPLY_DAYS * 86_400)
            .collect()
    }

    fn replies_file(&self) -> PathBuf {
        self.base
            .join(project::key(&project::user()))
            .join("replies.jsonl")
    }

    /// Records how a block of a policy kept in `root`'s scope turned out
    /// (ADR-0013 §6). Past [`FAILURES_COMPACT`] bytes, what no longer counts is
    /// dropped first.
    pub fn record_outcome(&self, root: &Path, record: &outcome::Record, now: u64) -> Result<()> {
        use std::io::Write;
        self.mark_project(root)?;
        let path = self.outcomes_file(root);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > FAILURES_COMPACT) {
            let kept: String = self
                .outcomes(root, now)
                .iter()
                .map(|r| format!("{}\n", r.to_json()))
                .collect();
            write_atomic(&path, &kept)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        writeln!(file, "{}", record.to_json())?;
        Ok(())
    }

    /// How the blocks of `root`'s policies turned out over the last
    /// [`outcome::OUTCOME_DAYS`] days, oldest first.
    pub fn outcomes(&self, root: &Path, now: u64) -> Vec<outcome::Record> {
        std::fs::read_to_string(self.outcomes_file(root))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|v| outcome::Record::from_json(&v))
            .filter(|r| now.saturating_sub(r.at) <= outcome::OUTCOME_DAYS * 86_400)
            .collect()
    }

    /// `root`'s active policies the user keeps overriding (ADR-0013 §6).
    pub fn overridden(&self, root: &Path, decisions: &[Decision], now: u64) -> Vec<outcome::Flag> {
        outcome::flagged(&self.outcomes(root, now), decisions)
    }

    fn outcomes_file(&self, root: &Path) -> PathBuf {
        self.base.join(project::key(root)).join("policy.jsonl")
    }

    /// Records a supervisor step offered in `root` and how it was answered, or
    /// taken by a policy (ADR-0019 §3). Past [`FAILURES_COMPACT`] bytes, what no
    /// longer counts is dropped first.
    pub fn record_step(&self, root: &Path, record: &steps::Record, now: u64) -> Result<()> {
        use std::io::Write;
        self.mark_project(root)?;
        let path = self.steps_file(root);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > FAILURES_COMPACT) {
            let kept: String = self
                .steps(root, now)
                .iter()
                .map(|r| format!("{}\n", r.to_json()))
                .collect();
            write_atomic(&path, &kept)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        writeln!(file, "{}", record.to_json())?;
        Ok(())
    }

    /// The supervisor steps recorded in `root` over the last
    /// [`steps::STEP_DAYS`] days, oldest first.
    pub fn steps(&self, root: &Path, now: u64) -> Vec<steps::Record> {
        std::fs::read_to_string(self.steps_file(root))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|v| steps::Record::from_json(&v))
            .filter(|r| now.saturating_sub(r.at) <= steps::STEP_DAYS * 86_400)
            .collect()
    }

    fn steps_file(&self, root: &Path) -> PathBuf {
        self.base.join(project::key(root)).join("supervisor.jsonl")
    }

    /// Records decisions served to a session in `root` (ADR-0014 §5). Written
    /// by the hooks themselves, so appends from two sessions can meet; each
    /// batch is one small write. Once the oldest line is a week past
    /// [`served::SERVED_DAYS`], what no longer counts is dropped first: only
    /// that line is read, so a busy project's hooks don't re-read the log each
    /// time. Writers hold `served.lock` throughout, so an append can't land in
    /// the file a prune is about to replace and be lost with it.
    pub fn record_served(&self, root: &Path, serves: &[served::Serve], now: u64) -> Result<()> {
        use std::io::{BufRead, Write};
        if serves.is_empty() {
            return Ok(());
        }
        self.mark_project(root)?;
        let path = self.served_file(root);
        let lock_path = path.with_extension("lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("open {}", lock_path.display()))?;
        lock.lock()
            .with_context(|| format!("lock {}", lock_path.display()))?;
        let oldest = std::fs::File::open(&path).ok().and_then(|f| {
            let mut first = String::new();
            std::io::BufReader::new(f).read_line(&mut first).ok()?;
            serde_json::from_str::<serde_json::Value>(&first).ok()?["at"].as_u64()
        });
        if oldest.is_some_and(|at| now.saturating_sub(at) > (served::SERVED_DAYS + 7) * 86_400) {
            let kept: String = self
                .served(root, now)
                .iter()
                .map(|s| format!("{}\n", s.to_json()))
                .collect();
            write_atomic(&path, &kept)?;
        }
        let lines: String = serves
            .iter()
            .map(|s| format!("{}\n", s.to_json()))
            .collect();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        file.write_all(lines.as_bytes())?;
        Ok(())
    }

    /// What was served in `root` over the last [`served::SERVED_DAYS`] days,
    /// oldest first.
    pub fn served(&self, root: &Path, now: u64) -> Vec<served::Serve> {
        std::fs::read_to_string(self.served_file(root))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|v| served::Serve::from_json(&v))
            .filter(|s| now.saturating_sub(s.at) <= served::SERVED_DAYS * 86_400)
            .collect()
    }

    /// `root`'s active decisions no session got lately (ADR-0014 §5), with how
    /// many days.
    pub fn unserved(&self, root: &Path, decisions: &[Decision], now: u64) -> Vec<(u32, u64)> {
        served::unserved(decisions, &self.served(root, now), now)
    }

    fn served_file(&self, root: &Path) -> PathBuf {
        self.base.join(project::key(root)).join("served.jsonl")
    }

    /// Records what ran in `root` (ADR-0016 §3). Past [`FAILURES_COMPACT`]
    /// bytes, what no longer counts is dropped first.
    pub fn record_fact(&self, root: &Path, seen: &facts::Sighting, now: u64) -> Result<()> {
        use std::io::Write;
        self.mark_project(root)?;
        let path = self.base.join(project::key(root)).join("facts.jsonl");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > FAILURES_COMPACT) {
            let kept: String = self
                .facts(root, now)
                .iter()
                .map(|s| format!("{}\n", s.to_json()))
                .collect();
            write_atomic(&path, &kept)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        writeln!(file, "{}", seen.to_json())?;
        Ok(())
    }

    /// What was seen run in `root` over the last [`facts::FACT_DAYS`] days.
    pub fn facts(&self, root: &Path, now: u64) -> Vec<facts::Sighting> {
        let path = self.base.join(project::key(root)).join("facts.jsonl");
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|v| facts::Sighting::from_json(&v))
            .filter(|s| now.saturating_sub(s.at) <= facts::FACT_DAYS * 86_400)
            .collect()
    }

    fn failures_file(&self, root: &Path) -> PathBuf {
        self.base.join(project::key(root)).join("failures.jsonl")
    }

    /// Appends `lines`. Past [`FAILURES_COMPACT`] bytes, what's older than
    /// [`FAILURE_DAYS`] is dropped first, since it no longer counts.
    fn append_failures(&self, root: &Path, lines: &[String], now: u64) -> Result<()> {
        use std::io::Write;
        if lines.is_empty() {
            return Ok(());
        }
        self.mark_project(root)?;
        let path = self.failures_file(root);
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > FAILURES_COMPACT) {
            let kept: String = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .lines()
                .filter(|l| {
                    serde_json::from_str::<serde_json::Value>(l).is_ok_and(|v| {
                        now.saturating_sub(v["at"].as_u64().unwrap_or(0)) <= FAILURE_DAYS * 86_400
                    })
                })
                .map(|l| format!("{l}\n"))
                .collect();
            write_atomic(&path, &kept)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        file.write_all(
            lines
                .iter()
                .map(|l| format!("{l}\n"))
                .collect::<String>()
                .as_bytes(),
        )?;
        Ok(())
    }

    /// Notes which root a project directory is for, so `projects` can list it.
    fn mark_project(&self, root: &Path) -> Result<()> {
        let dir = self.base.join(project::key(root));
        valkyrie_proto::ensure_private_dir(&self.base)?;
        valkyrie_proto::ensure_private_dir(&dir)?;
        let marker = dir.join("root");
        if !marker.exists() {
            write_atomic(&marker, &format!("{}\n", root.display()))?;
        }
        Ok(())
    }

    /// Copies `root`'s active decisions into `<checkout>/.valkyrie/decisions/` (the
    /// checkout being exported from, which may be a worktree of `root`), and its
    /// skills into `<checkout>/.claude/skills/`, replacing what an earlier export
    /// of these put there. Files that aren't this store's (a teammate's, say) are
    /// left alone. Returns the files written.
    pub fn export(&self, root: &Path, checkout: &Path) -> Result<Vec<PathBuf>> {
        let all = self.load(root);
        if all.is_empty() {
            bail!("no decisions for {} to export", root.display());
        }
        let into = checkout.join(".valkyrie/decisions");
        std::fs::create_dir_all(&into).with_context(|| format!("create {}", into.display()))?;
        for entry in std::fs::read_dir(&into)?.flatten() {
            let path = entry.path();
            let ours = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| file::parse(&t, root.to_path_buf()))
                .is_some_and(|d| all.iter().any(|s| s.id == d.id && s.created == d.created));
            if ours {
                std::fs::remove_file(&path)?;
            }
        }
        let mut written = Vec::new();
        for d in &all {
            if d.status != DecisionStatus::Active || d.kind == DecisionKind::Skill {
                continue;
            }
            let path = into.join(file::name(d));
            write_atomic(&path, &file::render(d))?;
            written.push(path);
        }
        written.extend(export_skills(&all, checkout)?);
        Ok(written)
    }
}

/// Active skills as `<checkout>/.claude/skills/<name>/SKILL.md`, where Claude Code
/// loads them (ADR-0011 §4); directories an earlier export made are replaced.
fn export_skills(all: &[Decision], checkout: &Path) -> Result<Vec<PathBuf>> {
    let into = checkout.join(".claude/skills");
    if let Ok(entries) = std::fs::read_dir(&into) {
        for entry in entries.flatten() {
            let file = entry.path().join("SKILL.md");
            let ours = std::fs::read_to_string(&file)
                .ok()
                .and_then(|t| skill::exported(&t))
                .is_some_and(|(id, created)| {
                    all.iter().any(|d| d.id == id && d.created == created)
                });
            // Only the file it wrote: anything added beside it stays, and so
            // does the directory then.
            if ours {
                std::fs::remove_file(&file)?;
                let _ = std::fs::remove_dir(entry.path());
            }
        }
    }
    let mut written = Vec::new();
    let mut names = HashSet::new();
    for d in all
        .iter()
        .filter(|d| d.status == DecisionStatus::Active && d.kind == DecisionKind::Skill)
    {
        // Not over someone else's skill, nor another of these, by the same name.
        let base = skill::name(d);
        let taken = |n: &String| into.join(n).join("SKILL.md").exists() || names.contains(n);
        let mut name = base.clone();
        let mut n = 1;
        while taken(&name) {
            let suffix = if n == 1 {
                format!("-{}", d.id)
            } else {
                format!("-{}-{n}", d.id)
            };
            let keep = 64usize.saturating_sub(suffix.len());
            name = format!(
                "{}{suffix}",
                base[..base.len().min(keep)].trim_end_matches('-')
            );
            n += 1;
        }
        names.insert(name.clone());
        let dir = into.join(&name);
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let path = dir.join("SKILL.md");
        write_atomic(&path, &skill::render(d, &name))?;
        written.push(path);
    }
    Ok(written)
}

/// A title on one line and a trimmed body, both without control characters (but
/// the body's line breaks) or bidi overrides: text a reviewer reads in a terminal
/// must be the text agents get.
/// A policy is read for the scope `root` is: the user's, or a project's.
/// `scope` as the user set it, settling any proposal: the one-time ask is done.
fn set_scope(d: &mut Decision, scope: Vec<String>) {
    d.scope = scope;
    d.proposed_scope = None;
    d.scope_asked = true;
}
fn clean(title: &str, body: &str, kind: DecisionKind, root: &Path) -> Result<(String, String)> {
    let title = file::one_line(&shown(title, false));
    if title.is_empty() {
        bail!("a decision needs a title");
    }
    if title.chars().count() > MAX_TITLE {
        bail!("title is over {MAX_TITLE} characters; put the detail in the body");
    }
    let body = shown(body, true).trim().to_owned();
    if kind == DecisionKind::Policy
        && let Err(e) = policy::check(&body, project::is_user(root))
    {
        bail!("{e}");
    }
    if kind == DecisionKind::Skill {
        if body.chars().count() > MAX_SKILL_BODY {
            bail!("body is over {MAX_SKILL_BODY} characters; a skill is one procedure");
        }
    } else if body.chars().count() > MAX_BODY {
        bail!("body is over {MAX_BODY} characters; a decision is a few sentences");
    }
    Ok((title, body))
}

/// `text` without control characters or bidi overrides, so what a reviewer reads in
/// a terminal is what an agent gets. On one line they become spaces; `multiline`
/// keeps newlines and tabs and drops the rest.
pub(crate) fn shown(text: &str, multiline: bool) -> String {
    let hidden = |c: char| {
        c.is_control()
            || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    };
    match multiline {
        true => text
            .chars()
            .filter(|&c| matches!(c, '\n' | '\t') || !hidden(c))
            .collect(),
        false => text
            .chars()
            .map(|c| if hidden(c) { ' ' } else { c })
            .collect(),
    }
}

pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        base: PathBuf,
        store: Store,
        repo: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let base =
                std::env::temp_dir().join(format!("valk-store-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let repo = base.join("repo");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
            let repo = std::fs::canonicalize(repo).unwrap();
            let store = Store::new(base.join("state/context"));
            Self { base, store, repo }
        }

        fn new_decision(&self, title: &str) -> NewDecision {
            NewDecision {
                cwd: self.repo.clone(),
                title: title.into(),
                body: "Because.".into(),
                kind: DecisionKind::Decision,
                propose: false,
                supersedes: None,
                review_every: None,
                session: None,
                scope: None,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn by(who: &str) -> Provenance {
        Provenance {
            by: who.into(),
            ..Provenance::default()
        }
    }

    #[test]
    fn failures_count_once_and_only_lately() {
        let f = Fixture::new("failures");
        let day = 86_400;
        assert_eq!(f.store.failures(&f.repo, day), Failures::default());
        f.store
            .record_failures(
                &f.repo,
                &[(0xabc, "c1"), (0xabc, "c2"), (0xdef, "c3")],
                None,
                day,
            )
            .unwrap();
        // Read again after a restart: not counted twice.
        f.store
            .record_failures(&f.repo, &[(0xabc, "c1")], None, day)
            .unwrap();
        f.store.record_proposal(&f.repo, 0xabc, 4, 2 * day).unwrap();
        let got = f.store.failures(&f.repo, 2 * day);
        assert_eq!(got.seen, HashMap::from([(0xabc, 2), (0xdef, 1)]));
        assert!(got.calls.contains("c3"));
        // A project with only failures is still a project.
        assert_eq!(f.store.projects(), std::slice::from_ref(&f.repo));
        // Long after, none of it counts.
        assert_eq!(
            f.store.failures(&f.repo, (FAILURE_DAYS + 3) * day),
            Failures::default()
        );
    }

    #[test]
    fn a_decision_is_scoped_as_asked_else_to_the_files_it_names_and_a_rewrite_keeps_it() {
        let f = Fixture::new("scope-decide");
        std::fs::create_dir_all(f.repo.join("web/src")).unwrap();
        std::fs::write(f.repo.join("web/src/proto.ts"), "").unwrap();
        let mut named = f.new_decision("Keep web/src/proto.ts in step");
        named.body = "Hand-copied types.".into();
        let named = f.store.decide(&named, by("human"), 1).unwrap();
        assert_eq!(named.scope, ["web/src/proto.ts"]);
        assert!(named.scope_asked);
        let mut asked = f.new_decision("TUI keys use Alt chords");
        asked.scope = Some(vec!["./crates/valkyrie-tui/".into()]);
        let asked = f.store.decide(&asked, by("claude"), 2).unwrap();
        assert_eq!(asked.scope, ["crates/valkyrie-tui/**"]);
        // Read back from its file.
        let back = f.store.load(&f.repo);
        assert_eq!(
            back.iter().find(|d| d.id == asked.id).unwrap().scope,
            asked.scope
        );
        let mut bad = f.new_decision("Bad");
        bad.scope = Some(vec!["../elsewhere/**".into()]);
        assert!(f.store.decide(&bad, by("human"), 3).is_err());
        // Nothing named and nothing asked: everywhere.
        assert!(
            f.store
                .decide(&f.new_decision("Plain"), by("human"), 4)
                .unwrap()
                .scope
                .is_empty()
        );
        // A rewrite with no scope of its own keeps the one it replaces.
        let mut rewrite = f.new_decision("Keep the proto types in step");
        rewrite.supersedes = Some(named.id);
        let rewrite = f.store.decide(&rewrite, by("human"), 5).unwrap();
        assert_eq!(rewrite.scope, ["web/src/proto.ts"]);
        // Habits hold in every project.
        let mut habit = f.new_decision("Ship as a PR");
        habit.cwd = project::user();
        habit.scope = Some(vec!["web/**".into()]);
        assert!(
            f.store
                .decide(&habit, by("human"), 6)
                .unwrap()
                .scope
                .is_empty()
        );
    }

    #[test]
    fn a_scope_is_proposed_once_and_waits_on_review() {
        let f = Fixture::new("scope-review");
        let a = f
            .store
            .decide(&f.new_decision("A"), by("human"), 1)
            .unwrap();
        let review =
            |id, action: &ReviewAction, at| f.store.review(&f.repo, id, action, None, None, at);
        let propose = |scope: &[&str]| ReviewAction::ProposeScope {
            scope: scope.iter().map(|s| s.to_string()).collect(),
        };
        // Recorded since scopes exist, so already settled.
        assert!(review(a.id, &propose(&["web/**"]), 2).is_err());
        // One from before: no scope, never asked.
        let mut old = f.store.load(&f.repo).remove(0);
        old.scope_asked = false;
        f.store.write(&f.repo, &old, None).unwrap();
        let d = review(a.id, &propose(&["web/**"]), 3).unwrap();
        assert_eq!(
            d.proposed_scope.as_deref(),
            Some(&["web/**".to_string()][..])
        );
        assert!(d.scope.is_empty() && d.scope_asked);
        // Not a rewording: it's no fresher for it.
        assert_eq!(d.updated, 1);
        // In the review queue, as an active decision.
        let queue = f.store.proposals();
        assert_eq!((queue.len(), queue[0].status), (1, DecisionStatus::Active));
        // Only once.
        assert!(review(a.id, &propose(&["docs/**"]), 4).is_err());
        let d = review(
            a.id,
            &ReviewAction::Scope {
                scope: vec!["web/src/**".into()],
            },
            5,
        )
        .unwrap();
        assert_eq!(
            (d.scope.as_slice(), d.proposed_scope.as_ref()),
            (&["web/src/**".to_string()][..], None)
        );
        assert!(f.store.proposals().is_empty());
        assert!(review(a.id, &ReviewAction::DeclineScope, 6).is_err());
        // Declined: still everywhere, and not asked again.
        let b = f
            .store
            .decide(&f.new_decision("B"), by("human"), 7)
            .unwrap();
        let mut old = f
            .store
            .load(&f.repo)
            .into_iter()
            .find(|d| d.id == b.id)
            .unwrap();
        old.scope_asked = false;
        f.store.write(&f.repo, &old, None).unwrap();
        review(b.id, &propose(&["docs/**"]), 8).unwrap();
        let d = review(b.id, &ReviewAction::DeclineScope, 9).unwrap();
        assert!(d.scope.is_empty() && d.proposed_scope.is_none() && d.scope_asked);
        // The model said it holds everywhere: asked, with no card.
        let c = f
            .store
            .decide(&f.new_decision("C"), by("human"), 10)
            .unwrap();
        let mut old = f
            .store
            .load(&f.repo)
            .into_iter()
            .find(|d| d.id == c.id)
            .unwrap();
        old.scope_asked = false;
        f.store.write(&f.repo, &old, None).unwrap();
        let d = review(c.id, &propose(&[]), 11).unwrap();
        assert!(d.proposed_scope.is_none() && d.scope_asked);
        assert!(f.store.proposals().is_empty());
        // Habits have no scope to propose.
        let mut habit = f.new_decision("Ship as a PR");
        habit.cwd = project::user();
        let h = f.store.decide(&habit, by("human"), 12).unwrap();
        let user = project::user();
        assert!(
            f.store
                .review(&user, h.id, &propose(&["web/**"]), None, None, 13)
                .is_err()
        );
    }

    #[test]
    fn active_decisions_pin_and_unpin_but_skills_and_proposals_dont() {
        let f = Fixture::new("pin");
        let a = f
            .store
            .decide(&f.new_decision("A"), by("human"), 1)
            .unwrap();
        let pin =
            |id, action: &ReviewAction, at| f.store.review(&f.repo, id, action, None, None, at);
        assert!(pin(a.id, &ReviewAction::Pin, 2).unwrap().pinned);
        // Kept in the file, read back.
        assert!(f.store.load(&f.repo)[0].pinned);
        assert!(!pin(a.id, &ReviewAction::Unpin, 3).unwrap().pinned);
        assert!(!f.store.load(&f.repo)[0].pinned);
        let mut skill = f.new_decision("Release the thing");
        skill.kind = DecisionKind::Skill;
        let skill = f.store.decide(&skill, by("human"), 4).unwrap();
        assert!(pin(skill.id, &ReviewAction::Pin, 5).is_err());
        let mut asked = f.new_decision("B");
        asked.propose = true;
        let asked = f.store.decide(&asked, by("human"), 6).unwrap();
        assert!(pin(asked.id, &ReviewAction::Pin, 7).is_err());
    }

    #[test]
    fn a_pin_isnt_a_confirmation_and_follows_the_rule_not_its_wording() {
        let f = Fixture::new("pin-keeps");
        let day = 86_400;
        let mut volatile = f.new_decision("Use node 24");
        volatile.review_every = Some(30);
        let a = f.store.decide(&volatile, by("human"), day).unwrap();
        let review =
            |id, action: &ReviewAction, at| f.store.review(&f.repo, id, action, None, None, at);
        // Pinned on day 40, it's still due: 30 days from when it was made.
        let pinned = review(a.id, &ReviewAction::Pin, 40 * day).unwrap();
        assert_eq!(pinned.updated, day);
        assert!(stale::due(&pinned, 40 * day).is_some());
        // Its replacement keeps the pin; an edit into a policy loses it.
        let mut next = f.new_decision("Use node 26");
        next.supersedes = Some(a.id);
        let b = f.store.decide(&next, by("human"), 41 * day).unwrap();
        assert!(b.pinned);
        assert!(
            f.store
                .load(&f.repo)
                .iter()
                .any(|d| d.id == b.id && d.pinned)
        );
        let edit = ReviewAction::Edit {
            title: "Checks to run".into(),
            body: "template: ship\nuntil: pr".into(),
            kind: DecisionKind::Policy,
            scope: None,
        };
        assert!(!review(b.id, &edit, 42 * day).unwrap().pinned);
    }

    #[test]
    fn humans_decide_agents_propose_and_ids_count_up() {
        let f = Fixture::new("who");
        let a = f
            .store
            .decide(&f.new_decision("A"), by("human"), 1)
            .unwrap();
        let b = f
            .store
            .decide(&f.new_decision("B"), by("claude"), 2)
            .unwrap();
        let mut asked = f.new_decision("C");
        asked.propose = true;
        let c = f.store.decide(&asked, by("human"), 3).unwrap();
        assert_eq!((a.id, a.status), (1, DecisionStatus::Active));
        assert_eq!((b.id, b.status), (2, DecisionStatus::Proposed));
        assert_eq!((c.id, c.status), (3, DecisionStatus::Proposed));
        assert_eq!(f.store.projects(), vec![f.repo.clone()]);
        let proposed: Vec<u32> = f.store.proposals().iter().map(|d| d.id).collect();
        assert_eq!(proposed, vec![2, 3]);
    }

    #[test]
    fn the_users_habits_are_a_scope_beside_the_projects() {
        let f = Fixture::new("user");
        let habit = NewDecision {
            cwd: project::user(),
            title: "Ship finished work as a pull request".into(),
            body: "template: ship\nuntil: pr".into(),
            kind: DecisionKind::Policy,
            ..f.new_decision("")
        };
        // An agent proposes a habit; it's the user's to accept, as in a project.
        let d = f.store.decide(&habit, by("claude"), 1).unwrap();
        assert_eq!((d.id, d.status), (1, DecisionStatus::Proposed));
        assert_eq!(d.project, project::user());
        assert!(f.base.join("state/context/user/decisions").is_dir());
        assert_eq!(f.store.projects(), vec![project::user()]);
        assert_eq!(f.store.proposals()[0].project, project::user());
        f.store
            .review(&project::user(), 1, &ReviewAction::Accept, None, None, 2)
            .unwrap();
        assert_eq!(
            f.store.load(&project::user())[0].status,
            DecisionStatus::Active
        );
        // A project's ids are its own.
        let p = f
            .store
            .decide(&f.new_decision("A"), by("human"), 3)
            .unwrap();
        assert_eq!(p.id, 1);
        // A repository's facts don't belong in the habits, nor a habit without until.
        for body in [
            "template: ship\nuntil: pr\nchecks: make",
            "template: ship\nci: gh",
        ] {
            let wrong = NewDecision {
                body: body.into(),
                ..habit.clone()
            };
            assert!(f.store.decide(&wrong, by("human"), 4).is_err(), "{body}");
        }
        // In a project, facts alone are fine.
        let facts = NewDecision {
            cwd: f.repo.clone(),
            body: "template: ship\nchecks: make".into(),
            ..habit
        };
        assert!(f.store.decide(&facts, by("human"), 5).is_ok());
    }

    #[test]
    fn accepting_a_replacement_supersedes_the_old_decision() {
        let f = Fixture::new("supersede");
        f.store
            .decide(&f.new_decision("Old"), by("human"), 1)
            .unwrap();
        let mut new = f.new_decision("New");
        new.supersedes = Some(1);
        let proposal = f.store.decide(&new, by("codex"), 2).unwrap();
        // Still only proposed: the old one holds until a human accepts.
        assert_eq!(f.store.load(&f.repo)[0].status, DecisionStatus::Active);
        f.store
            .review(&f.repo, proposal.id, &ReviewAction::Accept, None, None, 3)
            .unwrap();
        let all = f.store.load(&f.repo);
        assert_eq!(all[0].status, DecisionStatus::Superseded);
        assert_eq!(all[0].superseded_by, Some(2));
        assert_eq!(all[1].status, DecisionStatus::Active);
    }

    #[test]
    fn review_follows_the_lifecycle() {
        let f = Fixture::new("life");
        f.store
            .decide(&f.new_decision("P"), by("claude"), 1)
            .unwrap();
        let r = |id, a: ReviewAction| f.store.review(&f.repo, id, &a, None, None, 2);
        assert!(r(1, ReviewAction::Retire).is_err());
        let edited = r(
            1,
            ReviewAction::Edit {
                title: "Renamed".into(),
                body: "New body".into(),
                kind: DecisionKind::Gotcha,
                scope: None,
            },
        )
        .unwrap();
        assert_eq!(edited.status, DecisionStatus::Proposed);
        // The rename moved the file; there's still exactly one.
        assert_eq!(f.store.load(&f.repo).len(), 1);
        assert_eq!(
            r(1, ReviewAction::Reject).unwrap().status,
            DecisionStatus::Rejected
        );
        assert!(r(1, ReviewAction::Accept).is_err());
        assert!(r(9, ReviewAction::Accept).is_err());
    }

    #[test]
    fn bad_input_is_refused() {
        let f = Fixture::new("bad");
        assert!(
            f.store
                .decide(&f.new_decision("  "), by("human"), 1)
                .is_err()
        );
        let mut long = f.new_decision("x");
        long.body = "y".repeat(MAX_BODY + 1);
        assert!(f.store.decide(&long, by("human"), 1).is_err());
        let mut dangling = f.new_decision("x");
        dangling.supersedes = Some(5);
        assert!(f.store.decide(&dangling, by("human"), 1).is_err());
    }

    #[test]
    fn hidden_text_is_stripped() {
        let f = Fixture::new("hidden");
        let mut sneaky = f.new_decision("Use \u{1b}[8mrm -rf\u{1b}[0m tabs\u{202e}");
        sneaky.body = "line one\n\u{1b}[2K\rline two\u{2066}".into();
        let d = f.store.decide(&sneaky, by("claude"), 1).unwrap();
        assert_eq!(d.title, "Use [8mrm -rf [0m tabs");
        assert_eq!(d.body, "line one\n[2Kline two");
    }

    #[test]
    fn a_broken_file_keeps_its_id() {
        let f = Fixture::new("broken");
        f.store
            .decide(&f.new_decision("A"), by("human"), 1)
            .unwrap();
        let dir = f.store.dir(&f.repo);
        std::fs::write(
            dir.join("0002-hand-edited.md"),
            "---\nid: 2\nstatus: oops\n---\n# B\n",
        )
        .unwrap();
        let c = f
            .store
            .decide(&f.new_decision("C"), by("human"), 2)
            .unwrap();
        assert_eq!(c.id, 3);
    }

    #[test]
    fn revise_rewords_and_accepts_a_proposal_only() {
        let f = Fixture::new("revise");
        f.store
            .decide(&f.new_decision("Draft"), by("claude"), 1)
            .unwrap();
        let revise = ReviewAction::Revise {
            title: "Final".into(),
            body: "Better".into(),
            kind: DecisionKind::Constraint,
            scope: None,
        };
        let d = f
            .store
            .review(&f.repo, 1, &revise, Some(1), None, 2)
            .unwrap();
        assert_eq!(
            (d.title.as_str(), d.status),
            ("Final", DecisionStatus::Active)
        );
        // Read before it changed: refused.
        assert!(
            f.store
                .review(&f.repo, 1, &ReviewAction::Retire, Some(1), None, 3)
                .is_err()
        );
        // Already active (say, accepted elsewhere first): nothing is rewritten.
        assert!(f.store.review(&f.repo, 1, &revise, None, None, 3).is_err());
    }

    #[test]
    fn confirming_drops_anchors_that_are_gone() {
        let f = Fixture::new("gone");
        std::fs::write(f.repo.join("keep.rs"), "").unwrap();
        std::fs::write(f.repo.join("gone.rs"), "").unwrap();
        let d = f
            .store
            .decide(&f.new_decision("About keep.rs and gone.rs"), by("human"), 1)
            .unwrap();
        assert_eq!(d.fresh.anchors, ["keep.rs", "gone.rs"]);
        std::fs::remove_file(f.repo.join("gone.rs")).unwrap();
        let d = f
            .store
            .review(
                &f.repo,
                1,
                &ReviewAction::Confirm,
                None,
                Some("abc1234".into()),
                2,
            )
            .unwrap();
        assert_eq!(d.fresh.anchors, ["keep.rs"]);
        assert_eq!(
            (d.fresh.confirmed, d.provenance.commit.as_deref()),
            (2, Some("abc1234"))
        );
        // A commit that isn't one is never stored.
        let mut sneaky = f.new_decision("x");
        sneaky.cwd = f.repo.clone();
        let p = Provenance {
            by: "human".into(),
            commit: Some("--output=/x".into()),
            ..Provenance::default()
        };
        assert_eq!(
            f.store.decide(&sneaky, p, 3).unwrap().provenance.commit,
            None
        );
    }

    #[test]
    fn worktrees_share_their_repos_decisions() {
        let f = Fixture::new("share");
        let private = f.repo.join(".git/worktrees/wt");
        std::fs::create_dir_all(&private).unwrap();
        std::fs::write(private.join("commondir"), "../..").unwrap();
        let wt = f.base.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}", private.display())).unwrap();
        let mut from_wt = f.new_decision("Shared");
        from_wt.cwd = wt;
        let d = f.store.decide(&from_wt, by("human"), 1).unwrap();
        assert_eq!(d.project, f.repo);
        assert_eq!(f.store.load(&f.repo).len(), 1);
    }

    #[test]
    fn outcomes_are_kept_in_the_policys_scope_and_flag_it_after_three_sessions() {
        let f = Fixture::new("outcomes");
        let policy = NewDecision {
            kind: DecisionKind::Policy,
            body: "template: ship\nuntil: pr".into(),
            ..f.new_decision("Ship")
        };
        let d = f.store.decide(&policy, by("human"), 10).unwrap();
        let rec = |session: &str, at: u64, overridden: bool| outcome::Record {
            at,
            session: session.into(),
            policy: d.id,
            scope: "project".into(),
            project: f.repo.display().to_string(),
            overridden,
            how: "interrupted".into(),
            via: None,
        };
        for (s, at) in [("a", 20), ("b", 30)] {
            f.store
                .record_outcome(&f.repo, &rec(s, at, true), 40)
                .unwrap();
        }
        f.store
            .record_outcome(&f.repo, &rec("c", 35, false), 40)
            .unwrap();
        let list = f.store.load(&f.repo);
        assert!(f.store.overridden(&f.repo, &list, 40).is_empty());
        // Another scope's outcomes aren't this one's.
        f.store
            .record_outcome(&project::user(), &rec("u", 36, true), 40)
            .unwrap();
        assert!(f.store.overridden(&f.repo, &list, 40).is_empty());
        f.store
            .record_outcome(&f.repo, &rec("c", 37, true), 40)
            .unwrap();
        let flags = f.store.overridden(&f.repo, &list, 40);
        assert_eq!(
            flags,
            [outcome::Flag {
                id: d.id,
                overridden: 3,
                blocked: 3
            }]
        );
        // Past OUTCOME_DAYS they no longer count.
        let later = 37 + outcome::OUTCOME_DAYS * 86_400 + 1;
        assert!(f.store.overridden(&f.repo, &list, later).is_empty());
    }

    #[test]
    fn export_writes_only_active_decisions_and_replaces_its_own_files() {
        let f = Fixture::new("export");
        f.store
            .decide(&f.new_decision("Keep"), by("human"), 1)
            .unwrap();
        f.store
            .decide(&f.new_decision("Pending"), by("claude"), 2)
            .unwrap();
        let out = f.repo.join(".valkyrie/decisions");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("README.md"), "not a decision").unwrap();
        // A teammate's export: parses, but isn't one of ours.
        let theirs = "---\nid: 9\nstatus: active\ncreated: 5\n---\n# Theirs\n";
        std::fs::write(out.join("0009-theirs.md"), theirs).unwrap();
        std::fs::write(
            out.join("0001-stale-name.md"),
            file::render(&f.store.load(&f.repo)[0]),
        )
        .unwrap();
        let written = f.store.export(&f.repo, &f.repo).unwrap();
        assert_eq!(written, vec![out.join("0001-keep.md")]);
        let mut names: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, vec!["0001-keep.md", "0009-theirs.md", "README.md"]);
        let empty = Fixture::new("export-empty");
        assert!(empty.store.export(&empty.repo, &empty.repo).is_err());
    }

    #[test]
    fn a_failure_that_keeps_happening_after_acceptance_is_reported() {
        let f = Fixture::new("recurred");
        let mut proposal = f.new_decision("Kill exact pids");
        proposal.propose = true;
        let d = f.store.decide(&proposal, by("valkyrie"), 10).unwrap();
        f.store.record_proposal(&f.repo, 0xabc, d.id, 10).unwrap();
        // Before acceptance: the failures it came from.
        f.store
            .record_failures(&f.repo, &[(0xabc, "c1"), (0xabc, "c2")], Some("s1"), 10)
            .unwrap();
        let accepted = f
            .store
            .review(&f.repo, d.id, &ReviewAction::Accept, None, None, 20)
            .unwrap();
        let all = || f.store.load(&f.repo);
        assert!(f.store.recurred(&f.repo, &all()).is_empty());
        f.store
            .record_failures(&f.repo, &[(0xabc, "c3"), (0xdef, "c4")], Some("s2"), 30)
            .unwrap();
        assert!(
            f.store.recurred(&f.repo, &all()).is_empty(),
            "once isn't a pattern"
        );
        // Again in the same session: still one session.
        f.store
            .record_failures(&f.repo, &[(0xabc, "c6")], Some("s2"), 35)
            .unwrap();
        assert!(f.store.recurred(&f.repo, &all()).is_empty());
        f.store
            .record_failures(&f.repo, &[(0xabc, "c5")], Some("s3"), 40)
            .unwrap();
        assert_eq!(f.store.recurred(&f.repo, &all()), [(accepted.id, 2)]);
        // Confirming it (reworded, say) starts the count again.
        f.store
            .review(&f.repo, d.id, &ReviewAction::Confirm, None, None, 50)
            .unwrap();
        assert!(f.store.recurred(&f.repo, &all()).is_empty());
    }

    #[test]
    fn skill_use_round_trips_and_forgets_quiet_sessions() {
        let f = Fixture::new("skill-use");
        assert_eq!(f.store.skill_use(&f.repo), SkillUse::default());
        let day = 86_400;
        f.store
            .update_skill_use(&f.repo, 10 * day, |u| {
                u.reads.insert(1, 9 * day);
                u.sessions.insert("old".into(), day);
                u.sessions.insert("s1".into(), 10 * day);
                u.flagged.insert((2, 77), 10 * day);
                u.learned.push("uuid-1".into());
            })
            .unwrap();
        let got = f.store.skill_use(&f.repo);
        assert_eq!(got.since, 10 * day);
        assert_eq!(got.reads, BTreeMap::from([(1, 9 * day)]));
        assert_eq!(got.flagged, BTreeMap::from([((2, 77), 10 * day)]));
        assert_eq!(got.learned, ["uuid-1"]);
        assert_eq!(got.sessions_since(5 * day), 1);
        // A later update keeps when it started; a session quiet for long goes.
        f.store
            .update_skill_use(&f.repo, (FAILURE_DAYS + 5) * day, |_| {})
            .unwrap();
        let got = f.store.skill_use(&f.repo);
        assert_eq!(got.since, 10 * day);
        assert!(!got.sessions.contains_key("old"));
    }

    #[test]
    fn serves_are_appended_and_old_ones_dont_count() {
        use served::{Layer, Serve};
        let f = Fixture::new("served");
        let day = 86_400;
        assert!(f.store.served(&f.repo, day).is_empty());
        let serve = |id, layer, at| Serve {
            session: "s1".into(),
            id,
            layer,
            at,
        };
        f.store
            .record_served(&f.repo, &[serve(1, Layer::Block, day)], day)
            .unwrap();
        f.store
            .record_served(&f.repo, &[serve(2, Layer::Title, 50 * day)], 50 * day)
            .unwrap();
        f.store.record_served(&f.repo, &[], 50 * day).unwrap();
        assert_eq!(
            f.store.served(&f.repo, 50 * day),
            [
                serve(1, Layer::Block, day),
                serve(2, Layer::Title, 50 * day)
            ]
        );
        // Past the window, the first one is gone.
        let later = (served::SERVED_DAYS + 2) * day;
        assert_eq!(
            f.store.served(&f.repo, later),
            [serve(2, Layer::Title, 50 * day)]
        );
        // A week past it, the next write drops it from the file too.
        let file = f.store.served_file(&f.repo);
        let lines = || std::fs::read_to_string(&file).unwrap().lines().count();
        f.store
            .record_served(&f.repo, &[serve(3, Layer::Show, later)], later)
            .unwrap();
        assert_eq!(lines(), 3);
        let much_later = (served::SERVED_DAYS + 9) * day;
        f.store
            .record_served(&f.repo, &[serve(4, Layer::Show, much_later)], much_later)
            .unwrap();
        assert_eq!(lines(), 3);
        assert_eq!(f.store.served(&f.repo, much_later)[0].id, 2);
    }

    #[test]
    fn a_prune_keeps_serves_another_session_appends_meanwhile() {
        use served::{Layer, Serve};
        let f = Fixture::new("served-race");
        let day = 86_400;
        let now = (served::SERVED_DAYS + 30) * day;
        let file = f.store.served_file(&f.repo);
        let serve = |session: &str, id, at| Serve {
            session: session.into(),
            id,
            layer: Layer::Show,
            at,
        };
        // Lets the store make its own folders.
        f.store
            .record_served(&f.repo, &[serve("old", 0, now)], now)
            .unwrap();
        // Recent lines the prune keeps, so its read-and-rewrite takes a while.
        let recent: String = (0..2_000)
            .map(|i| format!("{}\n", serve("old", i, now - day).to_json()))
            .collect();
        const APPENDERS: u32 = 8;
        for round in 0..40 {
            // An ancient first line, so the next write prunes.
            std::fs::write(&file, format!("{}\n{recent}", serve("old", 0, 0).to_json())).unwrap();
            let start = std::sync::Barrier::new(APPENDERS as usize + 1);
            std::thread::scope(|s| {
                s.spawn(|| {
                    start.wait();
                    f.store
                        .record_served(&f.repo, &[serve("pruner", 0, now)], now)
                        .unwrap();
                });
                for n in 0..APPENDERS {
                    let (start, store, repo) = (&start, &f.store, &f.repo);
                    s.spawn(move || {
                        start.wait();
                        for k in 0..20 {
                            store
                                .record_served(repo, &[serve("other", n * 100 + k, now)], now)
                                .unwrap();
                        }
                    });
                }
            });
            let got = f.store.served(&f.repo, now);
            let others = got.iter().filter(|s| s.session == "other").count();
            assert_eq!(
                others,
                APPENDERS as usize * 20,
                "round {round}: serves lost"
            );
            assert_eq!(got.iter().filter(|s| s.session == "pruner").count(), 1);
        }
    }

    #[test]
    fn ci_agent_blocks_once_per_head_and_twice_per_pull_request() {
        let f = Fixture::new("ci-agent");
        let pr = "https://github.com/o/r/pull/1";
        let record = |head, at| f.store.record_ci_agent(&f.repo, pr, head, at, 2).unwrap();
        assert_eq!(record("aaa", 1), Some(1));
        assert_eq!(record("aaa", 2), None, "same head: once");
        assert_eq!(record("bbb", 3), Some(2));
        assert_eq!(record("ccc", 4), None, "two blocks is the limit");
        let other = "https://github.com/o/r/pull/2";
        assert_eq!(
            f.store
                .record_ci_agent(&f.repo, other, "aaa", 5, 2)
                .unwrap(),
            Some(1),
            "another pull request has its own count"
        );
        let later = 3 + FAILURE_DAYS * 86_400 + 1;
        assert_eq!(record("ccc", later), Some(1), "forgotten after a while");
    }

    #[test]
    fn ci_fix_blocks_count_per_pull_request_and_are_forgotten() {
        let f = Fixture::new("ci-fix");
        let (a, b) = (
            "https://github.com/o/r/pull/1",
            "https://github.com/o/r/pull/2",
        );
        let day = 86_400;
        let record = |pr, at| f.store.record_ci_fix(&f.repo, pr, at, 2).unwrap();
        assert_eq!(record(a, 1), Some(1));
        assert_eq!(record(a, 2), Some(2));
        // At the limit: not counted, not blocked.
        assert_eq!(record(a, 3), None);
        assert_eq!(f.store.ci_fix_blocks(&f.repo, a, 3), 2);
        assert_eq!(record(b, 10 * day), Some(1));
        // Quiet for long, even one left at the limit starts over; a recent one
        // keeps its count.
        let later = (FAILURE_DAYS + 5) * day;
        assert_eq!(record(b, later), Some(2));
        assert_eq!(f.store.ci_fix_blocks(&f.repo, a, later), 0);
        assert_eq!(record(a, later), Some(1));
    }

    #[test]
    fn skills_export_where_claude_loads_them() {
        let f = Fixture::new("export-skills");
        let skill = |title: &str| NewDecision {
            kind: DecisionKind::Skill,
            body: "1. Do it.".into(),
            ..f.new_decision(title)
        };
        f.store
            .decide(&skill("Cut a release"), by("human"), 1)
            .unwrap();
        f.store
            .decide(&skill("Cut a release!"), by("human"), 2)
            .unwrap();
        // Hand-written ones, one by the name the second would fall back to.
        for name in ["verify", "cut-a-release-2"] {
            let theirs = f.repo.join(".claude/skills").join(name);
            std::fs::create_dir_all(&theirs).unwrap();
            std::fs::write(
                theirs.join("SKILL.md"),
                format!("---\nname: {name}\n---\nhand-written\n"),
            )
            .unwrap();
        }
        let written = f.store.export(&f.repo, &f.repo).unwrap();
        let skills = f.repo.join(".claude/skills");
        assert_eq!(
            written,
            [
                skills.join("cut-a-release/SKILL.md"),
                skills.join("cut-a-release-2-2/SKILL.md")
            ]
        );
        assert!(
            std::fs::read_to_string(skills.join("cut-a-release-2/SKILL.md"))
                .unwrap()
                .contains("hand-written")
        );
        // A file added beside an exported skill survives the next export.
        std::fs::write(skills.join("cut-a-release/notes.md"), "mine").unwrap();
        assert!(
            !f.repo
                .join(".valkyrie/decisions/0001-cut-a-release.md")
                .exists()
        );
        let text = std::fs::read_to_string(&written[0]).unwrap();
        assert!(text.contains("name: cut-a-release\ndescription: \"Cut a release\""));
        // Again, after one is retired: its directory goes, the hand-written one stays.
        f.store
            .review(&f.repo, 2, &ReviewAction::Retire, None, None, 3)
            .unwrap();
        f.store.export(&f.repo, &f.repo).unwrap();
        let mut names: Vec<_> = std::fs::read_dir(&skills)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["cut-a-release", "cut-a-release-2", "verify"]);
        assert!(skills.join("cut-a-release/notes.md").exists());
    }

    #[test]
    fn one_replacement_waits_at_a_time_and_never_two_become_active() {
        let f = Fixture::new("supersede-race");
        f.store
            .decide(&f.new_decision("Cut a release"), by("human"), 1)
            .unwrap();
        let rewrite = |title: &str| NewDecision {
            supersedes: Some(1),
            propose: true,
            ..f.new_decision(title)
        };
        f.store
            .decide(&rewrite("Cut it v2"), by("claude"), 2)
            .unwrap();
        let second = f.store.decide(&rewrite("Cut it v3"), by("valkyrie"), 3);
        assert!(format!("{:#}", second.unwrap_err()).contains("#2 already proposes replacing #1"));
        // One that got in anyway (written by an older binary, say) can't be
        // accepted once the first is.
        let mut raced = f.store.load(&f.repo)[1].clone();
        raced.id = 3;
        raced.title = "Cut it v3".into();
        f.store.write(&f.repo, &raced, None).unwrap();
        f.store
            .review(&f.repo, 2, &ReviewAction::Accept, None, None, 4)
            .unwrap();
        let late = f
            .store
            .review(&f.repo, 3, &ReviewAction::Accept, None, None, 5);
        assert!(format!("{:#}", late.unwrap_err()).contains("which #2 already replaced"));
    }
}

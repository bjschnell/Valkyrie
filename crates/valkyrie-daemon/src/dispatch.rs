//! Dispatch (ADR-0019 §1, §2, §4, phase 4): the *item ready* watcher and its
//! `launch` step. An open item nobody's on, whose blockers have all shipped or
//! been dropped, is offered to start on the agent and model it was last launched
//! with (claude on its own default if never), as the board's Launch would. Never
//! while `max_running` (default 3) or more agent sessions already run in its
//! repository, the user's and the supervisor's alike, and only accepted items.
//!
//! When nothing is ready, the first item still waiting may not really need its
//! unfinished blocker (waiting is advisory, DESIGN §8.12): the extractor's
//! locked-down model is asked once, under `valk decisions auto` and the
//! supervisor's hourly limit, and an item it judges free to start is offered
//! with the blocker it waived.
//!
//! An offer is the user's to take or leave, from the board. Taken on two items,
//! a `launch` policy is proposed (ADR-0019 §3); accepted, the next ready item
//! starts without asking, once per item per daemon, never with the Context
//! setting off or `valk supervise off`.

use crate::Registry;
use crate::context::Peer;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use valkyrie_context::policy::{self, Trigger};
use valkyrie_context::{blocker, project, steps};
use valkyrie_proto::{AutoMode, Item, ItemStatus, LaunchOffer, Reply, Size, StepAction, order};

/// The default for `max_running`: agent sessions in one repository before the
/// supervisor stops starting more.
pub(crate) const MAX_RUNNING: usize = 3;
/// How often the watcher looks without being asked: sessions ending don't kick it.
const EVERY: Duration = Duration::from_secs(30);
/// A kick waits this long for the rest of a burst of changes.
const SETTLE: Duration = Duration::from_millis(500);
const MODEL_TIMEOUT: Duration = Duration::from_secs(90);
/// A started session's screen until a client attaches and sizes it.
const SIZE: Size = Size {
    cols: 120,
    rows: 40,
};

/// The offers waiting on the user, pushed with the items.
pub(crate) type Launches = Arc<Vec<LaunchOffer>>;

/// What an item and its unfinished blockers looked like when the model was asked:
/// any edit to one asks again.
type Sig = Vec<(u32, u64)>;

/// The model's answer to "does it need its blocker".
#[derive(Debug, Clone)]
struct Needs {
    sig: Sig,
    needs: bool,
    why: String,
}

/// Kept in `<state>/dispatch.json`, so an offer and what was left outlive a
/// restart.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Book {
    pending: Vec<LaunchOffer>,
    /// Items the user left, with their `updated` then: offered again once edited.
    #[serde(default)]
    left: HashMap<u32, u64>,
    #[serde(default)]
    last_id: u64,
    #[serde(skip)]
    needs: HashMap<u32, Needs>,
    /// Items a blocker call is under way for.
    #[serde(skip)]
    asking: HashSet<u32>,
    /// Items a policy started this boot: once each, then they're the user's.
    #[serde(skip)]
    automatic: HashSet<u32>,
    /// Tests set `max_running` here instead of in settings.
    #[serde(skip)]
    pub(crate) max: Option<usize>,
    #[serde(skip)]
    path: PathBuf,
}

impl Book {
    pub(crate) fn load(path: PathBuf) -> Self {
        let mut book: Book = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        book.path = path;
        book
    }

    fn save(&self) {
        let write = || -> Result<()> {
            if let Some(dir) = self.path.parent() {
                valkyrie_proto::ensure_private_dir(dir)?;
            }
            let tmp = self.path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
            std::fs::rename(&tmp, &self.path)?;
            Ok(())
        };
        if let Err(e) = write() {
            tracing::warn!("couldn't save launch offers: {e:#}");
        }
    }

    pub(crate) fn pending(&self) -> Vec<LaunchOffer> {
        self.pending.clone()
    }

    /// Milliseconds since the epoch, past any before: exact as a JavaScript
    /// number, as `order::Book::next_id` explains.
    fn next_id(&mut self) -> u64 {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_id = ms.max(self.last_id + 1);
        self.last_id
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// `max_running` from `$VALK_MAX_RUNNING` or `settings.toml`, else
/// [`MAX_RUNNING`]. Read when it matters, as the hooks read their settings.
fn max_running() -> usize {
    let parse = |v: &str| v.trim().trim_matches('"').parse::<usize>().ok();
    if let Some(n) = std::env::var("VALK_MAX_RUNNING")
        .ok()
        .as_deref()
        .and_then(parse)
    {
        return n;
    }
    let path = valkyrie_proto::config_dir().join("settings.toml");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split('#').next()?.split_once('='))
        .find(|(k, _)| k.trim() == "max_running")
        .and_then(|(_, v)| parse(v))
        .unwrap_or(MAX_RUNNING)
}

/// What the watcher found for one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pick {
    /// The first item that waits on nothing unfinished.
    Ready(u32),
    /// Nothing's ready, but the model judged this one doesn't need these
    /// unfinished blockers, and why.
    Waived(u32, Vec<u32>, String),
    /// Nothing's ready: ask the model about this one and these blockers.
    Ask(u32, Vec<u32>),
    Nothing,
}

/// An item and its unfinished blockers as they are now.
fn sig(item: &Item, blockers: &[u32], items: &[Item]) -> Sig {
    std::iter::once((item.id, item.updated))
        .chain(blockers.iter().map(|b| {
            let updated = items.iter().find(|i| i.id == *b).map_or(0, |i| i.updated);
            (*b, updated)
        }))
        .collect()
}

/// The pure part of the watcher: `project`'s first item to start, in `valk todo
/// next` order. Open (accepted), in the repository, nobody on it (`busy`), not
/// waiting on its pull request's review, and not left by the user since its last
/// edit. When none waits on nothing unfinished, the first waiting one the model
/// said doesn't need its blockers, or the first not asked about yet.
fn pick(
    project: &Path,
    items: &[Item],
    busy: &HashSet<u32>,
    left: &HashMap<u32, u64>,
    needs: &HashMap<u32, Needs>,
) -> Pick {
    let mut open: Vec<&Item> = items
        .iter()
        .filter(|i| i.status == ItemStatus::Open && i.project.as_deref() == Some(project))
        .filter(|i| !busy.contains(&i.id))
        .filter(|i| i.pr.is_none() || i.pr_closed)
        .filter(|i| left.get(&i.id) != Some(&i.updated))
        .collect();
    open.sort_by(|a, b| order(items)(a, b));
    for item in open {
        let waits = item.waiting_on(items);
        if waits.is_empty() {
            return Pick::Ready(item.id);
        }
        match needs.get(&item.id) {
            Some(n) if n.sig == sig(item, &waits, items) => {
                if !n.needs {
                    return Pick::Waived(item.id, waits, n.why.clone());
                }
            }
            _ => return Pick::Ask(item.id, waits),
        }
    }
    Pick::Nothing
}

/// Why an item's offered, in a line.
fn why(item: &Item, waived: &[u32], why: Option<&str>, running: usize, max: usize) -> String {
    let room = format!("{running} of {max} agent sessions running");
    let list = |ids: &[u32]| {
        ids.iter()
            .map(|b| format!("#{b}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if !waived.is_empty() {
        return format!(
            "waits on {}, but {}; {room}",
            list(waived),
            why.unwrap_or("the model says it needn't")
        );
    }
    if item.blocked_by.is_empty() {
        format!("next in the backlog; {room}")
    } else {
        format!("{} shipped or dropped; {room}", list(&item.blocked_by))
    }
}

impl Registry {
    /// Pushes the launch offers waiting to queue watchers, if they changed.
    pub(crate) fn refresh_launches(&self) {
        let pending = self.dispatch.lock().unwrap().pending();
        self.launches.send_if_modified(|current| {
            let changed = **current != pending;
            if changed {
                *current = Arc::new(pending);
            }
            changed
        });
    }

    /// Agent sessions running in each repository, with items being launched
    /// counted where they'll run.
    fn running(&self, items: &[Item]) -> HashMap<PathBuf, usize> {
        let mut out: HashMap<PathBuf, usize> = HashMap::new();
        for s in self.all() {
            if s.exited() || s.agent_behind().is_none() {
                continue;
            }
            *out.entry(project::root(&s.info().cwd)).or_default() += 1;
        }
        let launching = self.launching.lock().unwrap().clone();
        for item in items.iter().filter(|i| launching.contains(&i.id)) {
            if let Some(p) = &item.project {
                *out.entry(p.clone()).or_default() += 1;
            }
        }
        out
    }

    /// One look: per repository, the offer to show, the policy's launch, or the
    /// blocker question to ask.
    pub(crate) async fn dispatch_tick(self: &Arc<Self>) {
        let items = self.backlog.lock().unwrap().load();
        if !self.supervisor.lock().unwrap().on() {
            let cleared = {
                let mut book = self.dispatch.lock().unwrap();
                let had = !book.pending.is_empty();
                book.pending.clear();
                had
            };
            if cleared {
                self.dispatch.lock().unwrap().save();
                self.refresh_launches();
            }
            return;
        }
        let busy: HashSet<u32> = {
            let launching = self.launching.lock().unwrap().clone();
            items
                .iter()
                .filter(|i| launching.contains(&i.id) || !self.holders(i, None).is_empty())
                .map(|i| i.id)
                .collect()
        };
        let running = self.running(&items);
        let projects: BTreeSet<PathBuf> = items
            .iter()
            .filter(|i| i.status == ItemStatus::Open)
            .filter_map(|i| i.project.clone())
            .collect();
        let max = self
            .dispatch
            .lock()
            .unwrap()
            .max
            .unwrap_or_else(max_running);
        let mut changed = false;
        let mut starts: Vec<LaunchOffer> = Vec::new();
        let mut asks: Vec<(u32, Vec<u32>)> = Vec::new();
        {
            let mut book = self.dispatch.lock().unwrap();
            // Offers in repositories with nothing open any more.
            let before = book.pending.len();
            book.pending.retain(|o| projects.contains(&o.project));
            changed |= book.pending.len() != before;
            for project in &projects {
                let n = running.get(project).copied().unwrap_or(0);
                let found = if n < max {
                    pick(project, &items, &busy, &book.left, &book.needs)
                } else {
                    Pick::Nothing
                };
                let (id, waived, model_why) = match found {
                    Pick::Ready(id) => (id, Vec::new(), None),
                    Pick::Waived(id, waived, why) => (id, waived, Some(why)),
                    Pick::Ask(id, waits) => {
                        asks.push((id, waits));
                        (0, Vec::new(), None)
                    }
                    Pick::Nothing => (0, Vec::new(), None),
                };
                let at = book.pending.iter().position(|o| &o.project == project);
                if id == 0 {
                    if let Some(at) = at {
                        book.pending.remove(at);
                        changed = true;
                    }
                    continue;
                }
                if at.is_some_and(|at| book.pending[at].item == id) {
                    continue;
                }
                let Some(item) = items.iter().find(|i| i.id == id) else {
                    continue;
                };
                let (agent, model) = item
                    .launch
                    .as_ref()
                    .map_or(("claude".to_owned(), String::new()), |l| {
                        (l.agent.clone(), l.model.clone())
                    });
                let offer = LaunchOffer {
                    id: book.next_id(),
                    project: project.clone(),
                    item: id,
                    title: item.title.clone(),
                    agent,
                    model,
                    why: why(item, &waived, model_why.as_deref(), n, max),
                    waived,
                    made: now_secs(),
                };
                tracing::info!(
                    project = %project.display(),
                    item = id,
                    why = offer.why,
                    "supervisor offers a launch"
                );
                if let Some(at) = at {
                    book.pending.remove(at);
                }
                book.pending.push(offer.clone());
                changed = true;
                if !book.automatic.contains(&id) {
                    starts.push(offer);
                }
            }
            if changed {
                book.save();
            }
        }
        if changed {
            self.refresh_launches();
        }
        for offer in starts {
            self.launch_by_policy(offer).await;
        }
        for (id, waits) in asks {
            self.ask_blocker(id, waits, &items);
        }
    }

    /// Starts `offer` without asking if an active `launch` policy says to, and the
    /// Context setting is on.
    async fn launch_by_policy(self: &Arc<Self>, offer: LaunchOffer) {
        let context_on = {
            let sup = self.supervisor.lock().unwrap();
            sup.context_on.unwrap_or(crate::supervise::context_on)()
        };
        if !context_on {
            return;
        }
        let root = project::root(&offer.project);
        let decisions = self.context.lock().unwrap().load(&root);
        let Some(policy) = policy::step_policy(&decisions, StepAction::Launch, Trigger::Ready)
        else {
            return;
        };
        self.dispatch.lock().unwrap().automatic.insert(offer.item);
        match self.start(&offer, Some(policy), None).await {
            Ok(id) => tracing::info!(
                item = offer.item,
                session = id,
                policy,
                "a policy started a ready item"
            ),
            // It stays offered: the user's to take.
            Err(e) => tracing::info!(
                item = offer.item,
                policy,
                "offering the launch instead: {e:#}"
            ),
        }
    }

    /// Asks the model whether item `id` needs its unfinished `waits`: not with
    /// auto off, past the hourly limit, or while it's being asked already. The
    /// answer is kept until one of them changes, and the watcher looks again.
    fn ask_blocker(self: &Arc<Self>, id: u32, waits: Vec<u32>, items: &[Item]) {
        let mode = crate::extract::mode(&self.host.context_dir);
        if mode == AutoMode::Off {
            return;
        }
        let Some(item) = items.iter().find(|i| i.id == id).cloned() else {
            return;
        };
        let blockers: Vec<Item> = items
            .iter()
            .filter(|i| waits.contains(&i.id))
            .cloned()
            .collect();
        let sig = sig(&item, &waits, items);
        {
            let mut book = self.dispatch.lock().unwrap();
            if book.asking.contains(&id) {
                return;
            }
            if !self.supervisor.lock().unwrap().room() {
                return;
            }
            book.asking.insert(id);
        }
        let registry = Arc::clone(self);
        let dir = self.host.context_dir.clone();
        tokio::spawn(async move {
            let asked = tokio::task::spawn_blocking(move || {
                let refs: Vec<&Item> = blockers.iter().collect();
                let prompt =
                    blocker::prompt(&item, &refs, |by| mode == AutoMode::All || by != "codex");
                valkyrie_proto::ensure_private_dir(&dir)?;
                let command = valkyrie_context::model::command(blocker::SYSTEM);
                let reply = valkyrie_context::model::ask(&command, &prompt, &dir, MODEL_TIMEOUT)
                    .context("the model didn't answer")?;
                Ok::<_, anyhow::Error>(blocker::parse_reply(&reply))
            })
            .await;
            let answer = match asked {
                Ok(Ok(Some(answer))) => Some(answer),
                Ok(Ok(None)) => {
                    tracing::info!(item = id, "the blocker call gave no answer");
                    None
                }
                Ok(Err(e)) => {
                    tracing::warn!(item = id, "the blocker call failed: {e:#}");
                    None
                }
                Err(_) => None,
            };
            let mut book = registry.dispatch.lock().unwrap();
            book.asking.remove(&id);
            // No answer counts as needing it: waiting stays the safe call.
            let (needs, why) = answer.unwrap_or((true, String::new()));
            tracing::info!(item = id, needs, why, "blocker call");
            book.needs.insert(id, Needs { sig, needs, why });
            drop(book);
            registry.dispatch_kick.notify_one();
        });
    }

    /// Starts `offer`'s item as the board's Launch would, for the user or for
    /// `policy`, and records it: in the new session's event log and in the
    /// project's `supervisor.jsonl`. Returns the new session.
    async fn start(
        self: &Arc<Self>,
        offer: &LaunchOffer,
        policy: Option<u32>,
        peer: Option<Peer>,
    ) -> Result<valkyrie_proto::SessionId> {
        let reply = self
            .launch_item(offer.item, &offer.agent, None, true, SIZE, Vec::new(), peer)
            .await?;
        let Reply::Session { info } = reply else {
            bail!("starting #{} gave no session", offer.item);
        };
        {
            let mut book = self.dispatch.lock().unwrap();
            book.pending.retain(|o| o.id != offer.id);
            book.save();
        }
        self.refresh_launches();
        let mut fields = json!({
            "trigger": Trigger::Ready.as_str(),
            "reason": format!("#{} is ready", offer.item),
            "action": StepAction::Launch.as_str(),
            "offered": StepAction::Launch.as_str(),
            "why": offer.why,
            "item": offer.item,
            "auto": policy.is_some(),
        });
        if let Some(p) = policy {
            fields["policy"] = p.into();
        }
        let key = match self.get(info.id) {
            Ok(session) => {
                session.log_step(fields);
                session.key()
            }
            Err(_) => info.id.to_string(),
        };
        self.record_launch(offer, StepAction::Launch, policy, &key);
        Ok(info.id)
    }

    /// Records an answer to a launch offer; the user's may make a policy worth
    /// proposing. Answers count per item, so two items started are two sessions'
    /// worth (ADR-0019 §3).
    fn record_launch(&self, offer: &LaunchOffer, taken: StepAction, by: Option<u32>, key: &str) {
        let root = project::root(&offer.project);
        let now = crate::session::now_ms() / 1000;
        let record = steps::Record {
            at: now,
            session: format!("item #{}", offer.item),
            trigger: Trigger::Ready,
            offered: StepAction::Launch,
            taken,
            pr: String::new(),
            policy: by,
            features: None,
        };
        if let Err(e) = self
            .context
            .lock()
            .unwrap()
            .record_step(&root, &record, now)
        {
            tracing::warn!("couldn't record a launch answer: {e:#}");
        }
        if by.is_none() && taken == StepAction::Launch {
            self.learn(&root, key, now);
        }
    }

    /// `ClientMsg::AnswerLaunch`: the user only, on the offer waiting.
    pub(crate) async fn answer_launch(
        self: &Arc<Self>,
        project: PathBuf,
        id: u64,
        launch: bool,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        self.caller(peer).vouch(if launch {
            "starting a ready item the supervisor offers"
        } else {
            "leaving a ready item the supervisor offers"
        })?;
        let offer = self
            .dispatch
            .lock()
            .unwrap()
            .pending
            .iter()
            .find(|o| o.id == id && o.project == project)
            .cloned();
        let Some(offer) = offer else {
            bail!("that launch offer was answered or replaced: look again");
        };
        if launch {
            let session = self.start(&offer, None, peer).await?;
            let info = self.get(session)?.info();
            return Ok(Reply::Session { info });
        }
        let updated = self
            .backlog
            .lock()
            .unwrap()
            .get(offer.item)
            .map_or(0, |i| i.updated);
        {
            let mut book = self.dispatch.lock().unwrap();
            book.pending.retain(|o| o.id != id);
            book.left.insert(offer.item, updated);
            book.save();
        }
        self.refresh_launches();
        self.record_launch(&offer, StepAction::Leave, None, "");
        // The next ready one, if any.
        self.dispatch_kick.notify_one();
        tracing::info!(item = offer.item, "launch offer left");
        Ok(Reply::Text {
            text: format!("left #{}; it's offered again once it changes", offer.item),
        })
    }
}

/// The watcher: looks on every item change and every [`EVERY`].
pub(crate) async fn run(registry: Arc<Registry>) {
    // In a debug build, `$VALK_DISPATCH_EVERY_MS` for trying it out.
    let every = std::env::var("VALK_DISPATCH_EVERY_MS")
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .and_then(|ms| ms.parse().ok())
        .map_or(EVERY, Duration::from_millis);
    let mut interval = tokio::time::interval(every);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            _ = registry.dispatch_kick.notified() => tokio::time::sleep(SETTLE).await,
        }
        registry.dispatch_tick().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u32, status: ItemStatus, blocked_by: &[u32]) -> Item {
        let mut i: Item = serde_json::from_value(json!({
            "id": id, "title": format!("item {id}"), "created": id, "updated": id,
            "project": "/repo",
        }))
        .unwrap();
        i.status = status;
        i.blocked_by = blocked_by.to_vec();
        i
    }

    #[test]
    fn the_first_open_item_nobodys_on_whose_blockers_finished_is_ready() {
        use ItemStatus::{Dropped, Open, Proposed, Shipped};
        let p = Path::new("/repo");
        let none = HashMap::new();
        let mut items = vec![
            item(1, Shipped, &[]),
            item(2, Dropped, &[]),
            item(3, Proposed, &[]),
            item(4, Open, &[3]),
            item(5, Open, &[1, 2]),
            item(6, Open, &[]),
        ];
        // #4 waits on a proposed item; #5's blockers are done, and it's older.
        assert_eq!(
            pick(p, &items, &HashSet::new(), &HashMap::new(), &none),
            Pick::Ready(5)
        );
        // Someone's on #5: #6.
        let busy = HashSet::from([5]);
        assert_eq!(
            pick(p, &items, &busy, &HashMap::new(), &none),
            Pick::Ready(6)
        );
        // Left as it was: skipped; edited since: back.
        let left = HashMap::from([(5, 5)]);
        assert_eq!(
            pick(p, &items, &HashSet::new(), &left, &none),
            Pick::Ready(6)
        );
        items[4].updated = 50;
        assert_eq!(
            pick(p, &items, &HashSet::new(), &left, &none),
            Pick::Ready(5)
        );
        // Waiting on its PR's review isn't ready; a closed PR is.
        items[4].pr = Some("https://github.com/o/r/pull/1".into());
        items[5].pr = Some("https://github.com/o/r/pull/2".into());
        items[5].pr_closed = true;
        assert_eq!(
            pick(p, &items, &HashSet::new(), &HashMap::new(), &none),
            Pick::Ready(6)
        );
        // Another repository's items aren't this one's.
        assert_eq!(
            pick(
                Path::new("/other"),
                &items,
                &HashSet::new(),
                &HashMap::new(),
                &none
            ),
            Pick::Nothing
        );
    }

    #[test]
    fn with_nothing_ready_the_model_is_asked_about_the_first_waiting_once() {
        use ItemStatus::Open;
        let p = Path::new("/repo");
        let items = vec![item(1, Open, &[]), item(2, Open, &[1]), item(3, Open, &[1])];
        let busy = HashSet::from([1]);
        let left = HashMap::new();
        let mut needs = HashMap::new();
        assert_eq!(pick(p, &items, &busy, &left, &needs), Pick::Ask(2, vec![1]));
        // It needs it: the next one is asked about.
        needs.insert(
            2,
            Needs {
                sig: sig(&items[1], &[1], &items),
                needs: true,
                why: String::new(),
            },
        );
        assert_eq!(pick(p, &items, &busy, &left, &needs), Pick::Ask(3, vec![1]));
        needs.insert(
            3,
            Needs {
                sig: sig(&items[2], &[1], &items),
                needs: false,
                why: "only thematic".into(),
            },
        );
        assert_eq!(
            pick(p, &items, &busy, &left, &needs),
            Pick::Waived(3, vec![1], "only thematic".into())
        );
        // An edit to the blocker asks again.
        let mut edited = items.clone();
        edited[0].updated = 99;
        assert_eq!(
            pick(p, &edited, &busy, &left, &needs),
            Pick::Ask(2, vec![1])
        );
    }

    /// The watcher offers the first ready item while there's room, never past
    /// `max_running`; the user leaves one and starts two; the `launch` policy
    /// proposed for it, accepted, starts the next on its own.
    #[cfg(unix)]
    #[tokio::test]
    async fn ready_items_are_offered_with_room_then_started_by_a_learned_policy() {
        use crate::context::{AS_CALLER, Caller};
        use std::process::Command;
        use valkyrie_proto::{
            DecisionKind, DecisionStatus, ItemChange, NewItem, ReviewAction, SpawnSpec,
        };
        let dir = std::env::temp_dir().join(format!("valkyrie-dispatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Worktrees go beside the repository: keep them in `dir` too.
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "start"]);
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
        // No model call from a test: the blocker question isn't asked.
        crate::extract::set_mode(&registry.host.context_dir, AutoMode::Off).unwrap();
        registry.supervisor.lock().unwrap().context_on = Some(|| true);
        registry.dispatch.lock().unwrap().max = Some(2);
        *crate::backlog::AGENT_STANDIN.lock().unwrap() = Some(vec!["sleep".into(), "30".into()]);
        AS_CALLER.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(|| Caller {
                agent: None,
                session: None,
                bridge: false,
            }))
        });
        let ids: Vec<u32> = (1..=4)
            .map(|n| {
                let b = registry.backlog.lock().unwrap();
                let new = NewItem {
                    title: format!("item {n}"),
                    cwd: Some(repo.clone()),
                    ..Default::default()
                };
                let item = b.add(&new, "human", 1).unwrap();
                let open = ItemChange::Status {
                    status: ItemStatus::Open,
                };
                b.change(item.id, &open, 1).unwrap();
                item.id
            })
            .collect();
        let project = project::root(&repo);
        let offered = || registry.dispatch.lock().unwrap().pending();

        registry.dispatch_tick().await;
        let first = offered();
        assert_eq!(first.len(), 1, "{first:?}");
        assert_eq!((first[0].item, first[0].agent.as_str()), (ids[0], "claude"));
        assert!(first[0].why.contains("0 of 2"), "{}", first[0].why);
        assert_eq!(*registry.launches.borrow(), Arc::new(first.clone()));

        // Two agent sessions in the repository, the user's own: no room.
        let agent = || {
            let spec = SpawnSpec {
                command: vec!["sleep".into(), "30".into()],
                cwd: Some(repo.clone()),
                name: None,
                size: Size { cols: 40, rows: 10 },
                env: Vec::new(),
                worktree: None,
                hidden: false,
            };
            let Reply::Session { info } = crate::spawn(&registry, spec).unwrap() else {
                panic!("no session");
            };
            let s = registry.get(info.id).unwrap();
            s.driven_by("claude");
            s
        };
        let (a, b) = (agent(), agent());
        registry.dispatch_tick().await;
        assert!(offered().is_empty(), "offered past max_running");
        b.kill();
        for _ in 0..100 {
            if b.exited() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        registry.dispatch_tick().await;
        let again = offered();
        assert_eq!(again.len(), 1);
        assert!(again[0].why.contains("1 of 2"), "{}", again[0].why);

        // Left: the next one. An agent can't answer for the user.
        let leave = registry
            .answer_launch(project.clone(), again[0].id, false, None)
            .await
            .unwrap();
        assert!(matches!(leave, Reply::Text { .. }));
        registry.dispatch_tick().await;
        let second = offered();
        assert_eq!(second[0].item, ids[1]);
        // Started: the item has its session, the next is offered.
        let started = registry
            .answer_launch(project.clone(), second[0].id, true, None)
            .await
            .unwrap();
        let Reply::Session { info } = started else {
            panic!("{started:?}");
        };
        let item = registry.backlog.lock().unwrap().get(ids[1]).unwrap();
        assert!(item.sessions.contains(&info.id));
        registry.dispatch_tick().await;
        let third = offered();
        assert_eq!(third[0].item, ids[2]);
        let stale = registry
            .answer_launch(project.clone(), second[0].id, true, None)
            .await;
        assert!(stale.is_err_and(|e| e.to_string().contains("answered or replaced")));
        registry
            .answer_launch(project.clone(), third[0].id, true, None)
            .await
            .unwrap();

        // Two items started: a launch policy is proposed; accepted, it runs.
        let root = project::root(&repo);
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
        let proposed = policies();
        assert_eq!(proposed.len(), 1, "{proposed:?}");
        assert_eq!(proposed[0].status, DecisionStatus::Proposed);
        assert!(proposed[0].body.starts_with("template: launch\non: ready"));
        let now = crate::session::now_ms() / 1000;
        registry
            .context
            .lock()
            .unwrap()
            .review(
                &root,
                proposed[0].id,
                &ReviewAction::Accept,
                None,
                None,
                now,
            )
            .unwrap();
        registry.dispatch_tick().await;
        let fourth = registry.backlog.lock().unwrap().get(ids[3]).unwrap();
        assert!(!fourth.sessions.is_empty(), "the policy didn't start #4");
        assert!(offered().iter().all(|o| o.item != ids[3]));
        let answers = registry.context.lock().unwrap().steps(&root, now + 1);
        let taken: Vec<_> = answers
            .iter()
            .map(|r| (r.session.as_str(), r.taken, r.policy.is_some()))
            .collect();
        let item = |n: u32| format!("item #{n}");
        let (i1, i2, i3, i4) = (item(ids[0]), item(ids[1]), item(ids[2]), item(ids[3]));
        assert_eq!(
            taken,
            [
                (i1.as_str(), StepAction::Leave, false),
                (i2.as_str(), StepAction::Launch, false),
                (i3.as_str(), StepAction::Launch, false),
                (i4.as_str(), StepAction::Launch, true),
            ]
        );

        // The left one, once edited, is offered again; off, nothing is.
        let edit = ItemChange::Edit {
            title: Some("item 1, again".into()),
            body: None,
            kind: None,
            cwd: None,
            priority: None,
            blocked_by: None,
        };
        registry
            .backlog
            .lock()
            .unwrap()
            .change(ids[0], &edit, 2)
            .unwrap();
        registry.dispatch.lock().unwrap().automatic.insert(ids[0]);
        registry.dispatch_tick().await;
        assert_eq!(offered().first().map(|o| o.item), Some(ids[0]));
        registry.supervise(Some(false), None).unwrap();
        registry.dispatch_tick().await;
        assert!(offered().is_empty());
        registry.supervise(Some(true), None).unwrap();

        AS_CALLER.with(|cell| *cell.borrow_mut() = None);
        *crate::backlog::AGENT_STANDIN.lock().unwrap() = None;
        for s in registry.all() {
            s.kill();
        }
        drop(a);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn max_running_reads_the_setting() {
        // SAFETY: no other test reads this variable.
        unsafe { std::env::set_var("VALK_MAX_RUNNING", "5") };
        assert_eq!(max_running(), 5);
        unsafe { std::env::remove_var("VALK_MAX_RUNNING") };
    }
}

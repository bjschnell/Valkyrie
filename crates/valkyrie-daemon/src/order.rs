//! Build order (ADR-0019 §2, `order`): asks the extractor's locked-down model for
//! one repository's build order and keeps it as a proposal until the user answers.
//! Accepting applies its priority and `after #N` changes as the user's own edits,
//! all or some of them; declining changes nothing. Nothing here applies a change
//! on its own, and no answer is learned into a policy (decision #47): priority is
//! the user's call each time.
//!
//! It runs when asked (`valk todo plan`, the board's button) and when a watcher
//! sees unordered work: three or more items landing in one repository within a
//! day, or a note or body naming a build order.

use crate::Registry;
use crate::context::Peer;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use valkyrie_context::order;
use valkyrie_proto::{AutoMode, Item, ItemStatus, OrderApplied, OrderProposal, Reply};

/// Most build orders asked for per hour, by anyone: each reads the whole backlog.
const PER_HOUR: usize = 6;
const MODEL_TIMEOUT: Duration = Duration::from_secs(120);
/// Items filed in a burst land seconds apart: wait for the rest before asking.
const SETTLE: Duration = Duration::from_secs(60);
/// A burst: this many items in one repository within `BURST_SECS`, all since its
/// last build order.
const BURST: usize = 3;
const BURST_SECS: u64 = 24 * 3600;
/// A note naming an order asks again at most this often per repository.
const NOTE_EVERY_SECS: u64 = 3600;

/// The build orders waiting on the user, pushed with the items.
pub(crate) type Orders = Arc<Vec<OrderProposal>>;

/// What made a watcher fire.
#[derive(Debug, Clone)]
pub(crate) enum Trigger {
    /// An item was added to this repository.
    Added(PathBuf),
    /// Item `id`'s note or body names an order.
    Named(PathBuf, u32),
}

/// Kept in `<state>/orders.json`, so a proposal outlives a restart.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Book {
    pending: Vec<OrderProposal>,
    /// When each repository's last build order was made, answered or not.
    #[serde(default)]
    last: HashMap<PathBuf, u64>,
    /// The newest proposal's id, so the next is always larger.
    #[serde(default)]
    last_id: u64,
    #[serde(skip)]
    calls: VecDeque<Instant>,
    /// Repositories with a model call under way.
    #[serde(skip)]
    asking: Vec<PathBuf>,
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
            tracing::warn!("couldn't save build orders: {e:#}");
        }
    }

    pub(crate) fn pending(&self) -> Vec<OrderProposal> {
        self.pending.clone()
    }

    /// A new proposal's id: milliseconds since the epoch, past any before. Not
    /// nanoseconds: the web app reads numbers as doubles, exact only below 2^53, so
    /// a larger id would come back rounded and never match.
    fn next_id(&mut self) -> u64 {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_id = ms.max(self.last_id + 1);
        self.last_id
    }

    /// Takes one model call from the hourly budget, if there's room.
    fn spend(&mut self) -> bool {
        while self
            .calls
            .front()
            .is_some_and(|t| t.elapsed() > Duration::from_secs(3600))
        {
            self.calls.pop_front();
        }
        let room = self.calls.len() < PER_HOUR;
        if room {
            self.calls.push_back(Instant::now());
        }
        room
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Whether the watcher should ask about `project` now (the pure part).
fn due(trigger: &Trigger, items: &[Item], last: Option<u64>, pending: bool, now: u64) -> bool {
    let (Trigger::Added(project) | Trigger::Named(project, _)) = trigger;
    let open = items
        .iter()
        .filter(|i| i.status == ItemStatus::Open && i.project.as_ref() == Some(project));
    if pending || open.clone().count() < 2 {
        return false;
    }
    match trigger {
        Trigger::Added(_) => {
            let since = now.saturating_sub(BURST_SECS).max(last.unwrap_or(0));
            open.filter(|i| i.created > since).count() >= BURST
        }
        Trigger::Named(..) => last.is_none_or(|t| now.saturating_sub(t) >= NOTE_EVERY_SECS),
    }
}

/// The files each of `open`'s branches changes since it left `project`'s `HEAD`.
fn touches(project: &Path, open: &[Item]) -> HashMap<u32, Vec<String>> {
    let mut out = HashMap::new();
    for item in open {
        let Some(branch) = item.branch.as_deref() else {
            continue;
        };
        let range = format!("HEAD...{branch}");
        if let Ok(files) = crate::worktree::git_out(project, &["diff", "--name-only", &range]) {
            out.insert(item.id, files.lines().map(str::to_owned).collect());
        }
    }
    out
}

impl Registry {
    /// Pushes the build orders waiting to queue watchers, if they changed.
    pub(crate) fn refresh_orders(&self) {
        let pending = self.order_book.lock().unwrap().pending();
        self.orders.send_if_modified(|current| {
            let changed = **current != pending;
            if changed {
                *current = Arc::new(pending);
            }
            changed
        });
    }

    /// `ClientMsg::PlanOrder`.
    pub(crate) async fn plan_order(
        self: &Arc<Self>,
        cwd: PathBuf,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        if !valkyrie_context::project::in_git(&cwd) {
            bail!("{} isn't in a git repository", cwd.display());
        }
        let project = valkyrie_context::project::root(&cwd);
        let trigger = match self.caller(peer).agent {
            Some(agent) => format!("{agent} asked"),
            None => "you asked".into(),
        };
        let order = self.propose(&project, trigger).await?;
        Ok(Reply::Order {
            order: Box::new(order),
        })
    }

    /// Asks the model for `project`'s order and keeps it, replacing any waiting.
    async fn propose(self: &Arc<Self>, project: &Path, trigger: String) -> Result<OrderProposal> {
        let mode = crate::extract::mode(&self.host.context_dir);
        if mode == AutoMode::Off {
            bail!("build orders are asked of a model, which `valk decisions auto off` turned off");
        }
        let items = self.backlog.lock().unwrap().load();
        let open = order::open_in(&items, project);
        if open.len() < 2 {
            bail!(
                "{} has fewer than two open items: nothing to order",
                project.display()
            );
        }
        {
            let mut book = self.order_book.lock().unwrap();
            if book.asking.iter().any(|p| p == project) {
                bail!(
                    "a build order for {} is already being worked out",
                    project.display()
                );
            }
            if !book.spend() {
                bail!("{PER_HOUR} build orders asked for in the last hour: try again later");
            }
            book.asking.push(project.to_owned());
        }
        let asked = self.ask(project, mode, open, items).await;
        let mut book = self.order_book.lock().unwrap();
        book.asking.retain(|p| p != project);
        let steps = asked?;
        if steps.is_empty() {
            bail!("the model proposed no order");
        }
        let made = now_secs();
        let proposal = OrderProposal {
            id: book.next_id(),
            project: project.to_owned(),
            made,
            trigger,
            steps,
        };
        book.pending.retain(|p| p.project != project);
        book.pending.push(proposal.clone());
        book.last.insert(project.to_owned(), made);
        book.save();
        drop(book);
        tracing::info!(project = %project.display(), steps = proposal.steps.len(), trigger = proposal.trigger, "build order proposed");
        self.refresh_orders();
        Ok(proposal)
    }

    async fn ask(
        &self,
        project: &Path,
        mode: AutoMode,
        open: Vec<Item>,
        items: Vec<Item>,
    ) -> Result<Vec<valkyrie_proto::OrderStep>> {
        let command = valkyrie_context::model::command(order::SYSTEM);
        let dir = self.host.context_dir.clone();
        let project = project.to_owned();
        tokio::task::spawn_blocking(move || {
            let name = project
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let touched = touches(&project, &open);
            let prompt = order::prompt(&name, &open, &items, &touched, |by| {
                mode == AutoMode::All || by != "codex"
            });
            valkyrie_proto::ensure_private_dir(&dir)?;
            let reply = valkyrie_context::model::ask(&command, &prompt, &dir, MODEL_TIMEOUT)
                .context("the model didn't answer")?;
            Ok(order::parse_reply(&reply, &open, &items))
        })
        .await?
    }

    /// `ClientMsg::AnswerOrder`: the user only. Each accepted item's change goes
    /// through `change_item` as the user's own edit; one edited since the proposal
    /// keeps its edit, and one the backlog now refuses (a loop since) is skipped,
    /// so the rest still apply.
    pub(crate) async fn answer_order(
        self: &Arc<Self>,
        project: PathBuf,
        id: u64,
        accept: bool,
        only: Option<Vec<u32>>,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        self.caller(peer).vouch(if accept {
            "accepting a build order"
        } else {
            "declining a build order"
        })?;
        let proposal = {
            let mut book = self.order_book.lock().unwrap();
            let Some(at) = book
                .pending
                .iter()
                .position(|p| p.id == id && p.project == project)
            else {
                bail!("that build order was answered or replaced: look again");
            };
            if let Some(only) = &only
                && let Some(stray) = only
                    .iter()
                    .find(|i| !book.pending[at].steps.iter().any(|s| s.item == **i))
            {
                bail!("#{stray} isn't in this build order");
            }
            let proposal = book.pending.remove(at);
            book.save();
            proposal
        };
        self.refresh_orders();
        let mut applied = OrderApplied::default();
        if !accept {
            tracing::info!(project = %project.display(), "build order declined");
            return Ok(Reply::OrderApplied { applied });
        }
        for step in &proposal.steps {
            if only.as_ref().is_some_and(|o| !o.contains(&step.item)) {
                continue;
            }
            let Some(change) = step.change() else {
                continue;
            };
            let current = self.backlog.lock().unwrap().get(step.item);
            let why = match current {
                Err(_) => Some("deleted since".to_string()),
                Ok(i) if i.status != ItemStatus::Open => {
                    Some(format!("{} since", i.status.as_str()))
                }
                Ok(i) if i.updated > proposal.made => Some("edited since".into()),
                Ok(_) => None,
            };
            if let Some(why) = why {
                applied.skipped.push((step.item, why));
                continue;
            }
            match self.change_item(step.item, change, peer).await {
                Ok(_) => applied.changed.push(step.item),
                Err(e) => applied.skipped.push((step.item, format!("{e:#}"))),
            }
        }
        tracing::info!(
            project = %project.display(),
            changed = applied.changed.len(),
            skipped = applied.skipped.len(),
            "build order accepted"
        );
        Ok(Reply::OrderApplied { applied })
    }

    /// Tells the watcher about an item added, or a note or body naming an order.
    pub(crate) fn noticed(&self, trigger: Trigger) {
        let _ = self.order_tx.send(trigger);
    }
}

/// The watcher: takes what `noticed` saw and asks for an order when it's due.
pub(crate) async fn run(registry: Arc<Registry>, mut rx: mpsc::UnboundedReceiver<Trigger>) {
    let settle = settle();
    while let Some(first) = rx.recv().await {
        tokio::time::sleep(settle).await;
        // One per repository; a named order outranks a burst, for its reason.
        let mut due_now: Vec<Trigger> = Vec::new();
        for t in std::iter::once(first).chain(std::iter::from_fn(|| rx.try_recv().ok())) {
            let (Trigger::Added(p) | Trigger::Named(p, _)) = &t;
            match due_now.iter().position(|d| {
                let (Trigger::Added(q) | Trigger::Named(q, _)) = d;
                q == p
            }) {
                Some(at) if matches!(t, Trigger::Named(..)) => due_now[at] = t,
                Some(_) => {}
                None => due_now.push(t),
            }
        }
        if crate::extract::mode(&registry.host.context_dir) == AutoMode::Off {
            continue;
        }
        for trigger in due_now {
            let (Trigger::Added(project) | Trigger::Named(project, _)) = &trigger;
            let items = registry.backlog.lock().unwrap().load();
            let (last, pending) = {
                let book = registry.order_book.lock().unwrap();
                (
                    book.last.get(project).copied(),
                    book.pending.iter().any(|p| &p.project == project),
                )
            };
            if !due(&trigger, &items, last, pending, now_secs()) {
                continue;
            }
            let why = match &trigger {
                Trigger::Added(_) => format!("{BURST}+ items added within a day"),
                Trigger::Named(_, id) => format!("#{id} names a build order"),
            };
            if let Err(e) = registry.propose(project, why).await {
                tracing::info!(project = %project.display(), "no build order: {e:#}");
            }
        }
    }
}

/// `SETTLE`; in a debug build, `$VALK_ORDER_SETTLE_MS` for tests.
fn settle() -> Duration {
    std::env::var("VALK_ORDER_SETTLE_MS")
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .and_then(|ms| ms.parse().ok())
        .map_or(SETTLE, Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u32, project: &str, created: u64, status: ItemStatus) -> Item {
        let mut i: Item = serde_json::from_value(serde_json::json!({
            "id": id, "title": "t", "created": created, "updated": created, "project": project,
        }))
        .unwrap();
        i.status = status;
        i
    }

    #[test]
    fn a_burst_of_three_in_a_day_since_the_last_order_is_due_once() {
        let p = PathBuf::from("/repo");
        let added = Trigger::Added(p.clone());
        let now = 100_000;
        let mut items = vec![
            item(1, "/repo", now - 50, ItemStatus::Open),
            item(2, "/repo", now - 40, ItemStatus::Open),
            item(3, "/other", now - 30, ItemStatus::Open),
            item(4, "/repo", now - BURST_SECS - 1, ItemStatus::Open),
        ];
        // Two today here, one elsewhere, one too old.
        assert!(!due(&added, &items, None, false, now));
        items.push(item(5, "/repo", now - 10, ItemStatus::Open));
        assert!(due(&added, &items, None, false, now));
        // Not while one waits, nor for items from before the last order.
        assert!(!due(&added, &items, None, true, now));
        assert!(!due(&added, &items, Some(now - 45), false, now));
        // A shipped one doesn't count.
        items[4].status = ItemStatus::Shipped;
        assert!(!due(&added, &items, None, false, now));
    }

    /// Two daemons on one state dir (an admin terminal's beside the user's, on
    /// Windows) each keep their build orders: one's save doesn't wipe the other's.
    #[test]
    fn two_daemons_keep_their_own_orders() {
        let dir = std::env::temp_dir().join(format!("valkyrie-orders2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let open = |socket: &str| {
            crate::new_registry(
                dir.join(socket),
                &dir.join("state"),
                Path::new("/bin/true"),
                1,
                0,
                7,
                false,
            )
            .unwrap()
            .0
        };
        let proposal = |id| OrderProposal {
            id,
            project: PathBuf::from("/repo"),
            made: 0,
            trigger: "you asked".into(),
            steps: Vec::new(),
        };
        let (user, admin) = (open("u.sock"), open("a.sock"));
        {
            let mut book = user.order_book.lock().unwrap();
            book.pending.push(proposal(1));
            book.save();
        }
        {
            let mut book = admin.order_book.lock().unwrap();
            book.pending.push(proposal(2));
            book.save();
        }
        // The user's daemon restarts.
        drop(user);
        let ids: Vec<u64> = open("u.sock")
            .order_book
            .lock()
            .unwrap()
            .pending
            .iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(
            ids,
            [1],
            "the other daemon's save replaced this one's orders"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only the user answers a build order, only the one waiting, and only for
    /// items in it; an agent's try leaves it waiting.
    #[tokio::test]
    async fn only_the_user_answers_the_order_waiting() {
        use crate::context::{AS_CALLER, Caller};
        let dir = std::env::temp_dir().join(format!("valkyrie-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
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
        let project = PathBuf::from("/repo");
        let proposal = OrderProposal {
            id: 9,
            project: project.clone(),
            made: 0,
            trigger: "you asked".into(),
            steps: vec![valkyrie_proto::OrderStep {
                item: 1,
                reason: String::new(),
                priority: Some(valkyrie_proto::Priority::P0),
                blocked_by: None,
            }],
        };
        registry.order_book.lock().unwrap().pending.push(proposal);
        AS_CALLER.with(|c| {
            *c.borrow_mut() = Some(Box::new(|| Caller {
                agent: Some("claude".into()),
                session: None,
                bridge: false,
                desktop: false,
            }))
        });
        let refused = registry
            .answer_order(project.clone(), 9, true, None, None)
            .await;
        assert!(
            refused.is_err_and(|e| e.to_string().contains("is for the user")),
            "an agent accepted a build order"
        );
        AS_CALLER.with(|c| {
            *c.borrow_mut() = Some(Box::new(|| Caller {
                agent: None,
                session: None,
                bridge: false,
                desktop: false,
            }))
        });
        let stale = registry
            .answer_order(project.clone(), 8, false, None, None)
            .await;
        assert!(
            stale.is_err_and(|e| e.to_string().contains("answered or replaced")),
            "answered one that isn't waiting"
        );
        let stray = registry
            .answer_order(project.clone(), 9, true, Some(vec![2]), None)
            .await;
        assert!(
            stray.is_err_and(|e| e.to_string().contains("#2 isn't in this build order")),
            "accepted an item not in it"
        );
        assert_eq!(registry.order_book.lock().unwrap().pending.len(), 1);
        let declined = registry.answer_order(project, 9, false, None, None).await;
        assert!(
            matches!(&declined, Ok(Reply::OrderApplied { applied }) if applied.changed.is_empty()),
            "{declined:?}"
        );
        assert!(registry.order_book.lock().unwrap().pending.is_empty());
        // Kept on disk too: a restart doesn't bring it back.
        let file = registry.order_book.lock().unwrap().path.clone();
        let saved = std::fs::read_to_string(file).unwrap();
        assert!(saved.contains("\"pending\": []"), "{saved}");
        AS_CALLER.with(|c| *c.borrow_mut() = None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The web app reads numbers as doubles: an id past 2^53 comes back rounded.
    #[test]
    fn proposal_ids_grow_and_survive_a_javascript_number() {
        let mut book = Book::default();
        let (a, b) = (book.next_id(), book.next_id());
        assert!(b > a);
        assert!(b < (1 << 53), "{b} isn't exact as a JavaScript number");
    }

    #[test]
    fn a_named_order_asks_at_most_hourly() {
        let p = PathBuf::from("/repo");
        let named = Trigger::Named(p, 1);
        let items = vec![
            item(1, "/repo", 0, ItemStatus::Open),
            item(2, "/repo", 0, ItemStatus::Open),
        ];
        let now = 100_000;
        assert!(due(&named, &items, None, false, now));
        assert!(!due(&named, &items, Some(now - 60), false, now));
        assert!(due(&named, &items, Some(now - NOTE_EVERY_SECS), false, now));
        assert!(!due(&named, &items[..1], None, false, now));
    }
}

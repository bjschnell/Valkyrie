//! The backlog (DESIGN §8.12): items written down for later, each of which starts
//! an agent on itself in a worktree of its own.

use crate::context::Peer;
use crate::{Registry, worktree};
use anyhow::{Result, bail};
use std::path::PathBuf;
use std::sync::Arc;
use valkyrie_context::backlog;
use valkyrie_proto::{
    Item, ItemChange, ItemStatus, KeptItem, NewItem, Reply, SessionId, SessionInfo, ShippedItem,
    Size, SpawnSpec,
};

/// Every item, pushed to queue watchers with the queue.
pub(crate) type Items = Arc<Vec<Item>>;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl Registry {
    pub(crate) fn items(&self) -> Reply {
        Reply::Items {
            items: self.backlog.lock().unwrap().load(),
        }
    }

    /// Pushes the items to queue watchers, if they changed.
    pub(crate) fn refresh_items(&self) {
        let items = self.backlog.lock().unwrap().load();
        self.items.send_if_modified(|current| {
            let changed = **current != items;
            if changed {
                *current = Arc::new(items);
            }
            changed
        });
    }

    /// Writes an item down: an agent's is proposed until the user accepts it. With
    /// `take` (its branch and pull request), it's also taken for the caller's
    /// session in the same step, as `take_item` would: an agent tracking work it
    /// does that nobody planned. The card follows that session; its text still
    /// never becomes a prompt the user launches until they accept it (ADR-0012).
    pub(crate) fn add_item(
        &self,
        new: &NewItem,
        take: Option<(Option<&str>, Option<&str>)>,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        let caller = self.caller(peer);
        let by = caller.agent.unwrap_or_else(|| "human".into());
        let item = match take {
            None => self.backlog.lock().unwrap().add(new, &by, now_secs())?,
            Some((branch, pr)) => {
                let Some(session) = caller.session else {
                    bail!("add --take from inside a valk session");
                };
                let info = session.info();
                if let Some(branch) = branch {
                    worktree::valid(&info.cwd, branch)?;
                }
                if let Some(pr) = pr {
                    backlog::pr_url(pr)?;
                }
                let project = valkyrie_context::project::in_git(&info.cwd)
                    .then(|| valkyrie_context::project::root(&info.cwd));
                let backlog = self.backlog.lock().unwrap();
                let now = now_secs();
                let filed = backlog.add_filed(new, &by, Some((self.boot, info.id)), now)?;
                backlog.taken(filed.id, info.id, self.boot, &[], branch, pr, project, now)?
            }
        };
        tracing::info!(
            id = item.id,
            by,
            taken = take.is_some(),
            "backlog item added"
        );
        self.refresh_items();
        Ok(Reply::Item {
            item: Box::new(item),
            note: None,
        })
    }

    /// An agent may reword an item still proposed, which the user reads before
    /// accepting, and drop one; the rest (accepting, shipping, rewording one the user
    /// will launch) is the user's.
    pub(crate) async fn change_item(
        self: &Arc<Self>,
        id: u32,
        change: ItemChange,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        let current = self.backlog.lock().unwrap().get(id)?;
        match &change {
            ItemChange::Status {
                status: ItemStatus::Dropped,
            } => {}
            ItemChange::Edit { .. } if current.status == ItemStatus::Proposed => {}
            ItemChange::Edit { .. } => self.vouch(peer, "rewording an accepted backlog item")?,
            ItemChange::Status { status } => {
                let what = match (current.status, status) {
                    (ItemStatus::Proposed, ItemStatus::Open) => "accepting a backlog item",
                    (_, ItemStatus::Shipped) => "shipping a backlog item",
                    _ => "reopening a backlog item",
                };
                self.vouch(peer, what)?;
            }
        }
        let (item, note) = self.apply(id, &change).await?;
        self.refresh_items();
        Ok(Reply::Item {
            item: Box::new(item),
            note,
        })
    }

    /// Makes a change the caller may make, taking a shipped item's worktree away;
    /// also returns what happened to that.
    async fn apply(&self, id: u32, change: &ItemChange) -> Result<(Item, Option<String>)> {
        let item = self
            .backlog
            .lock()
            .unwrap()
            .change(id, change, now_secs())?;
        self.after_change(item).await
    }

    /// Takes the worktree of an item just shipped away, if it has one; returns the
    /// item as it is then, and what happened to that.
    async fn after_change(&self, mut item: Item) -> Result<(Item, Option<String>)> {
        let id = item.id;
        let mut note = None;
        if item.status == ItemStatus::Shipped && item.checkout.is_some() {
            let (removed, said) = self.take_checkout(&item).await?;
            if removed {
                item = self.backlog.lock().unwrap().cleared(id, now_secs())?;
            }
            note = Some(said);
        }
        Ok((item, note))
    }

    /// The sessions other than `mine` still running on `item` on this daemon, by
    /// launch or take, newest first.
    fn holders(&self, item: &Item, mine: Option<SessionId>) -> Vec<SessionId> {
        if item.boot != self.boot {
            return Vec::new();
        }
        item.sessions
            .iter()
            .rev()
            .copied()
            .filter(|&s| Some(s) != mine && self.get(s).is_ok_and(|s| !s.exited()))
            .collect()
    }

    /// Whether `item` may be taken or launched (`how`) by the caller, whose own
    /// session (`mine`) doesn't count: several agents on one backlog mustn't end up
    /// on the same item. Refused while a launch is starting a session on it, or
    /// while another live session is on it, naming that one, unless the caller
    /// hands it over: the user's steal, or the user's launch over sessions that
    /// only took it (a take never locks the user out). Returns the sessions it's
    /// handed over from, which leave the item; they keep running.
    fn claim(
        &self,
        item: &Item,
        mine: Option<SessionId>,
        agent: bool,
        how: Claim,
    ) -> Result<Vec<SessionId>> {
        let id = item.id;
        if self.launching.lock().unwrap().contains(&id) {
            bail!("#{id} is being launched right now; try again in a moment");
        }
        let holders = self.holders(item, mine);
        let Some(&newest) = holders.first() else {
            return Ok(holders);
        };
        let launched = holders.iter().find(|s| !item.taken.contains(s)).copied();
        let hand_over = match how {
            Claim::Take { steal } => steal,
            Claim::Launch => !agent && launched.is_none(),
        };
        if hand_over {
            return Ok(holders);
        }
        let or = match how {
            _ if agent => "leave it to that session",
            Claim::Take { .. } => "stop it first or take it with --steal",
            Claim::Launch => "attach to it, or stop it first",
        };
        let blocker = match how {
            Claim::Launch => launched.unwrap_or(newest),
            Claim::Take { .. } => newest,
        };
        let Ok(session) = self.get(blocker) else {
            bail!("#{id} is already being worked on in session {blocker}; {or}");
        };
        let info = session.info();
        let agent = session.agent_behind().unwrap_or_else(|| {
            let program = info.command.first().map(PathBuf::from).unwrap_or_default();
            program
                .file_name()
                .map_or("shell".into(), |n| n.to_string_lossy().into_owned())
        });
        let mut name = backlog::one_line(&info.name);
        if name.chars().count() > 24 {
            name = format!("{}…", name.chars().take(23).collect::<String>());
        }
        bail!(
            "#{id} is already being worked on in session {} ({}, \"{name}\"); {or}",
            info.id,
            backlog::one_line(&agent)
        );
    }

    /// Claims item `id` for the caller's session, as a launch links one, so its
    /// lane follows that session and the session may leave notes on it. Agents may:
    /// it's how one working through accepted items outside a launch says which it's
    /// on. No checkout is recorded, since the session may be in the main one. One
    /// session at a time: another live one on it refuses the take, unless the user
    /// `steal`s it, which hands it to their session.
    pub(crate) fn take_item(
        &self,
        id: u32,
        branch: Option<&str>,
        pr: Option<&str>,
        steal: bool,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        if steal {
            self.vouch(peer, "taking an item from another session")?;
        }
        let caller = self.caller(peer);
        let Some(session) = caller.session else {
            bail!("take it from inside a valk session");
        };
        let item = self.backlog.lock().unwrap().get(id)?;
        let info = session.info();
        match item.status {
            ItemStatus::Open => {}
            // Its own work, written down with `add --take`: the session that filed
            // it may take it again (to record the pull request) before it's accepted.
            ItemStatus::Proposed if item.filed_by == Some((self.boot, info.id)) => {}
            ItemStatus::Proposed => bail!("#{id} is proposed; accept it first"),
            other => bail!("#{id} is {}; reopen it first", other.as_str()),
        }
        // Ship merged judges an item by its branch, so which branch it is can't be
        // an agent's to steer: a launch's is fixed, and one a take recorded only
        // the user replaces.
        if let Some(branch) = branch.filter(|b| item.branch.as_deref() != Some(*b)) {
            let current = item.branch.as_deref().unwrap_or_default();
            if item.checkout.is_some() || (item.branch.is_some() && !item.branch_taken) {
                bail!("#{id} is on {current}, its launch's branch, which stays");
            }
            if item.branch.is_some() && caller.agent.is_some() {
                bail!("#{id} is already on {current}; only the user changes that");
            }
            worktree::valid(&info.cwd, branch)?;
        }
        let project = valkyrie_context::project::in_git(&info.cwd)
            .then(|| valkyrie_context::project::root(&info.cwd));
        // Checked and recorded under one lock, so two takes can't both find it free.
        let item = {
            let backlog = self.backlog.lock().unwrap();
            let item = backlog.get(id)?;
            let from = self.claim(
                &item,
                Some(info.id),
                caller.agent.is_some(),
                Claim::Take { steal },
            )?;
            backlog.taken(
                id,
                info.id,
                self.boot,
                &from,
                branch,
                pr,
                project,
                now_secs(),
            )?
        };
        let by = caller.agent.unwrap_or_else(|| "human".into());
        tracing::info!(id, session = info.id, by, "backlog item taken");
        self.refresh_items();
        Ok(Reply::Item {
            item: Box::new(item),
            note: None,
        })
    }

    /// Ships every open item whose branch is merged into the default branch as
    /// last fetched, the user only: shipping is theirs, this is just many at once.
    /// Anyone may ask what would ship (`dry_run`); `only` ships just those items,
    /// what the user was shown, each judged again. An item with a launch's
    /// worktree is judged by what that checkout holds. Each goes as `valk todo
    /// ship` would, so a launch's clean worktree goes too. The daemon doesn't
    /// fetch: it has no credentials of the user's to fetch with, and the CLI and
    /// TUI fetch before asking.
    pub(crate) async fn ship_merged(
        &self,
        dry_run: bool,
        only: Option<Vec<u32>>,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        if !dry_run {
            self.vouch(peer, "shipping backlog items")?;
        }
        let items = self.backlog.lock().unwrap().load();
        let mut shipped = Vec::new();
        let mut kept = Vec::new();
        let mut judged = Vec::new();
        for item in items {
            if only.as_ref().is_some_and(|ids| !ids.contains(&item.id)) {
                continue;
            }
            let (Some(project), Some(branch)) = (item.project.clone(), item.branch.clone()) else {
                continue;
            };
            // An agent's own work it filed and took ships too: the user's confirm
            // is the acceptance.
            if item.status != ItemStatus::Open && !item.agent_filed() {
                continue;
            }
            let filed = item.agent_filed();
            let (id, title) = (item.id, item.title.clone());
            judged.push(id);
            let checkout = item.checkout.clone().filter(|c| c.is_dir());
            let (judged_branch, branch_taken) = (branch.clone(), item.branch_taken);
            let merged = tokio::task::spawn_blocking(move || match checkout {
                Some(checkout) => worktree::checkout_merged(&project, &checkout, &branch),
                None => worktree::branch_merged(&project, &branch),
            })
            .await?;
            let against = match merged {
                Ok(against) => against,
                Err(why) => {
                    kept.push(KeptItem { id, title, why });
                    continue;
                }
            };
            if dry_run {
                shipped.push(ShippedItem {
                    id,
                    title,
                    branch: judged_branch.clone(),
                    branch_taken,
                    filed,
                    against,
                    note: None,
                });
                continue;
            }
            // Shipped only as it was judged: not if it was shipped, dropped, or
            // moved to another branch or checkout meanwhile.
            let changed = {
                let backlog = self.backlog.lock().unwrap();
                match backlog.get(id) {
                    Ok(now)
                        if now.status == item.status
                            && now.branch == item.branch
                            && now.checkout == item.checkout =>
                    {
                        let ship = ItemChange::Status {
                            status: ItemStatus::Shipped,
                        };
                        Some(backlog.change(id, &ship, now_secs()))
                    }
                    _ => None,
                }
            };
            let done = match changed {
                Some(Ok(item)) => self.after_change(item).await,
                Some(Err(e)) => Err(e),
                None => {
                    let why = "changed since it was checked".to_owned();
                    kept.push(KeptItem { id, title, why });
                    continue;
                }
            };
            match done {
                Ok((_, note)) => shipped.push(ShippedItem {
                    id,
                    title,
                    branch: judged_branch.clone(),
                    branch_taken,
                    filed,
                    against,
                    note: note.filter(|n| !n.is_empty()),
                }),
                Err(e) => kept.push(KeptItem {
                    id,
                    title,
                    why: format!("{e:#}"),
                }),
            }
        }
        // Asked for by number, but no longer open with a branch.
        for id in only.unwrap_or_default() {
            if !judged.contains(&id) {
                let why = "no longer open with a branch".to_owned();
                kept.push(KeptItem {
                    id,
                    title: String::new(),
                    why,
                });
            }
        }
        if !shipped.is_empty() && !dry_run {
            tracing::info!(shipped = shipped.len(), "shipped merged backlog items");
            self.refresh_items();
        }
        Ok(Reply::Shipped { shipped, kept })
    }

    /// Deletes item `id`, for the user only, since it can't be undone (drop is the
    /// one an agent may use). Its worktree goes as when shipped; one with work
    /// still in it keeps the item too.
    pub(crate) async fn delete_item(
        self: &Arc<Self>,
        id: u32,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        self.vouch(peer, "deleting a backlog item")?;
        let item = self.backlog.lock().unwrap().get(id)?;
        // A session that took it may be on other work: only a launch's counts.
        let running = item.boot == self.boot
            && item
                .sessions
                .iter()
                .filter(|s| !item.taken.contains(s))
                .any(|&s| self.get(s).is_ok_and(|s| !s.exited()));
        if running {
            bail!("an agent is still running on #{id}; stop it first");
        }
        let mut note = None;
        if item.checkout.is_some() {
            let (removed, said) = self.take_checkout(&item).await?;
            if !removed {
                bail!("{said}; ship or drop #{id} instead");
            }
            note = Some(said);
        }
        let item = self.backlog.lock().unwrap().delete(id)?;
        tracing::info!(id, "backlog item deleted");
        self.refresh_items();
        Ok(Reply::Item {
            item: Box::new(item),
            note,
        })
    }

    /// Takes an item's worktree away if nothing runs in it and it's clean, with its
    /// branch if that's merged. Says whether the worktree is gone, and what happened.
    async fn take_checkout(&self, item: &Item) -> Result<(bool, String)> {
        let (Some(project), Some(checkout)) = (item.project.clone(), item.checkout.clone()) else {
            return Ok((true, String::new()));
        };
        let busy = self
            .listed()
            .iter()
            .any(|s| !s.exited() && s.info().cwd.starts_with(&checkout));
        if busy {
            return Ok((
                false,
                format!(
                    "kept {}: a session is still running in it",
                    checkout.display()
                ),
            ));
        }
        let branch = item.branch.clone();
        Ok(tokio::task::spawn_blocking(move || {
            // Merged, squash merges included: then the branch goes too, though
            // `git branch -d` would keep a squashed one.
            let merged = worktree::merged_head(&project, &checkout);
            worktree::remove(&project, &checkout, branch.as_deref(), merged.as_deref())
        })
        .await?)
    }

    /// Leaves a handoff note. An agent may only on an item its own session was
    /// launched for or took: a note goes into the brief of the item's next launch,
    /// so one from anywhere else would be a way into a prompt the user starts.
    pub(crate) fn note_item(&self, id: u32, text: &str, peer: Option<Peer>) -> Result<Reply> {
        let caller = self.caller(peer);
        let by = caller.agent.clone().unwrap_or_else(|| "human".into());
        let mut taken = false;
        if caller.agent.is_some() {
            let item = self.backlog.lock().unwrap().get(id)?;
            let session = caller
                .session
                .as_ref()
                .map(|s| s.info().id)
                .filter(|s| item.boot == self.boot && item.sessions.contains(s));
            let Some(session) = session else {
                bail!("an agent notes only the item its session was launched for, not #{id}");
            };
            // Any agent may take any accepted item, so its notes inform the user but
            // never reach a prompt the user starts.
            taken = item.taken.contains(&session);
        }
        let item = self
            .backlog
            .lock()
            .unwrap()
            .note(id, &by, text, taken, now_secs())?;
        tracing::info!(id, by, "backlog note left");
        self.refresh_items();
        Ok(Reply::Item {
            item: Box::new(item),
            note: None,
        })
    }

    /// What item `id`'s checkout holds now, from git.
    pub(crate) async fn item_facts(&self, id: u32) -> Result<Reply> {
        let item = self.backlog.lock().unwrap().get(id)?;
        let facts = match (item.project, item.checkout) {
            (Some(repo), Some(checkout)) => {
                tokio::task::spawn_blocking(move || worktree::facts(&repo, &checkout)).await?
            }
            _ => None,
        };
        Ok(Reply::ItemFacts { facts })
    }

    /// Starts `agent` on item `id`: in the checkout an earlier launch made if it's
    /// still there, else in a new worktree on the item's branch (or the repository
    /// itself without `worktree`). Returns the spawn's reply.
    pub(crate) async fn launch_item(
        self: &Arc<Self>,
        id: u32,
        agent: &str,
        worktree: bool,
        size: Size,
        env: Vec<(String, Option<String>)>,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        if !matches!(agent, "claude" | "codex") {
            bail!("an item starts claude or codex, not {agent}");
        }
        let item = self.backlog.lock().unwrap().get(id)?;
        match item.status {
            ItemStatus::Open => {}
            ItemStatus::Proposed => bail!("#{id} is proposed; accept it first"),
            other => bail!("#{id} is {}; reopen it first", other.as_str()),
        }
        let Some(project) = item.project.clone() else {
            bail!("#{id} has no folder yet; pick the repository it's for");
        };
        // One session on an item at a time; held from here until the new one is
        // recorded on it, so a take or launch meanwhile is refused too.
        let caller = self.caller(peer);
        let mine = caller.session.as_ref().map(|s| s.info().id);
        let (from, _launching) = {
            let backlog = self.backlog.lock().unwrap();
            let from = self.claim(
                &backlog.get(id)?,
                mine,
                caller.agent.is_some(),
                Claim::Launch,
            )?;
            self.launching.lock().unwrap().insert(id);
            (from, Launching(self, id))
        };
        let branch = item
            .branch
            .clone()
            .unwrap_or_else(|| backlog::branch(&item));
        let (cwd, new_worktree) = match &item.checkout {
            Some(checkout) if checkout.is_dir() => (checkout.clone(), None),
            _ if worktree => (project.clone(), Some(branch.clone())),
            _ => (project.clone(), None),
        };
        let mut name: String = format!("#{id} {}", item.title).chars().take(40).collect();
        name = name.trim_end().to_owned();
        let mut env = env;
        // The item it's on, for `valk todo show` in the session.
        env.push(("VALK_ITEM".into(), Some(id.to_string())));
        let spec = SpawnSpec {
            command: vec![agent.to_owned(), backlog::brief(&item)],
            cwd: Some(cwd),
            name: Some(name),
            size,
            env,
            worktree: new_worktree.clone(),
        };
        let registry = Arc::clone(self);
        let reply = crate::in_worktree(spec, move |spec| crate::spawn(&registry, spec)).await?;
        let Reply::Session { info } = &reply else {
            return Ok(reply);
        };
        let checkout = new_worktree.map(|b| (b, checkout_of(info)));
        self.backlog.lock().unwrap().launched(
            id,
            info.id,
            self.boot,
            &from,
            checkout,
            now_secs(),
        )?;
        tracing::info!(id, session = info.id, "launched a backlog item");
        self.refresh_items();
        Ok(reply)
    }
}

/// What `Registry::claim` is asked for.
#[derive(Clone, Copy)]
enum Claim {
    /// A take; `steal` (the user's) hands the item over from whoever has it.
    Take {
        steal: bool,
    },
    Launch,
}

/// Marks an item as being launched until dropped, whatever becomes of the launch.
struct Launching<'a>(&'a Registry, u32);

impl Drop for Launching<'_> {
    fn drop(&mut self) {
        self.0.launching.lock().unwrap().remove(&self.1);
    }
}

/// The new worktree's top: the session starts there, since it was asked for at the
/// repository's root.
fn checkout_of(info: &SessionInfo) -> PathBuf {
    valkyrie_context::project::checkout(&info.cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_claims_an_item_while_a_launch_is_starting_on_it() {
        let dir = std::env::temp_dir().join(format!("valkyrie-claim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (registry, _) = crate::new_registry(
            dir.join("v.sock"),
            &dir.join("state"),
            std::path::Path::new("/bin/true"),
            1,
            0,
            7,
            false,
        )
        .unwrap();
        let new = NewItem {
            title: "x".into(),
            body: String::new(),
            kind: Default::default(),
            cwd: None,
            attachments: Vec::new(),
        };
        let item = registry
            .backlog
            .lock()
            .unwrap()
            .add(&new, "human", 1)
            .unwrap();
        let take = |steal| registry.claim(&item, Some(3), false, Claim::Take { steal });
        assert_eq!(take(false).unwrap(), Vec::<SessionId>::new());
        {
            registry.launching.lock().unwrap().insert(item.id);
            let _launching = Launching(&registry, item.id);
            for steal in [false, true] {
                let refused = take(steal).unwrap_err().to_string();
                assert!(refused.contains("being launched right now"), "{refused}");
            }
            let refused = registry.claim(&item, None, false, Claim::Launch);
            assert!(refused.is_err(), "nor a second launch");
        }
        // Its guard dropped, whatever became of the launch: free again.
        assert!(take(false).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

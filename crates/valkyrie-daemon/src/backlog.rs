//! The backlog (DESIGN §8.12): items written down for later, each of which starts
//! an agent on itself in a worktree of its own.

use crate::context::Peer;
use crate::work::{Check, Pr, PrLookup};
use crate::{Registry, worktree};
use anyhow::{Result, bail};
use std::path::PathBuf;
use std::sync::Arc;
use valkyrie_context::backlog;
use valkyrie_proto::{
    Item, ItemChange, ItemLaunch, ItemStatus, KeptItem, NewItem, Reply, SessionId, SessionInfo,
    ShippedItem, Size, SpawnSpec,
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
            orders: self.order_book.lock().unwrap().pending(),
            launches: self.dispatch.lock().unwrap().pending(),
        }
    }

    /// Pushes the items to queue watchers, if they changed.
    pub(crate) fn refresh_items(&self) {
        // The dispatch watcher looks again: an item may be ready, or taken.
        self.dispatch_kick.notify_one();
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
            // An agent in a session records which, so it may take its own find
            // once the user says to go ahead (ADR-0018).
            None => {
                let filed_by = caller
                    .session
                    .as_ref()
                    .filter(|_| by != "human")
                    .map(|s| (self.boot, s.info().id));
                self.backlog
                    .lock()
                    .unwrap()
                    .add_filed(new, &by, filed_by, now_secs())?
            }
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
        if let Some(project) = item.project.clone() {
            if valkyrie_context::order::names_order(&item.body) {
                self.noticed(crate::order::Trigger::Named(project.clone(), item.id));
            }
            self.noticed(crate::order::Trigger::Added(project));
        }
        Ok(Reply::Item {
            item: Box::new(item),
            note: None,
        })
    }

    /// An agent may drop an item it filed itself (not once shipped, which is the
    /// user's), and reword one it filed that's still proposed, which the user
    /// reads before accepting; the rest (accepting, shipping, rewording one the
    /// user will launch, touching the user's or another agent's card) is the
    /// user's (ADR-0018). The caller is worked out first, and the change judged
    /// on the item under the same lock that writes it: an accept or ship landing
    /// meanwhile can't let through what it would refuse (backlog #61).
    pub(crate) async fn change_item(
        self: &Arc<Self>,
        id: u32,
        change: ItemChange,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        let caller = self.caller(peer);
        let session = caller.session.as_ref().map(|s| (self.boot, s.info().id));
        let (item, was) = {
            let backlog = self.backlog.lock().unwrap();
            let current = backlog.get(id)?;
            let own = caller.agent.is_some() && session.is_some() && current.filed_by == session;
            match &change {
                ItemChange::Status {
                    status: ItemStatus::Dropped,
                } if own && current.status != ItemStatus::Shipped => {}
                ItemChange::Status {
                    status: ItemStatus::Dropped,
                } => caller.vouch("dropping a backlog item another filed, or a shipped one")?,
                ItemChange::Edit { .. } if own && current.status == ItemStatus::Proposed => {}
                ItemChange::Edit { .. } if current.status == ItemStatus::Proposed => {
                    caller.vouch("rewording a proposal another filed")?
                }
                ItemChange::Edit { .. } => caller.vouch("rewording an accepted backlog item")?,
                ItemChange::Return => caller.vouch("returning a backlog item")?,
                ItemChange::Status { status } => caller.vouch(match (current.status, status) {
                    (ItemStatus::Proposed, ItemStatus::Open) => "accepting a backlog item",
                    (_, ItemStatus::Shipped) => "shipping a backlog item",
                    _ => "reopening a backlog item",
                })?,
            }
            // Asked before the item forgets them; stopped only once it agrees to go back.
            let running = match change {
                ItemChange::Return => self.holders(&current, None),
                _ => Vec::new(),
            };
            let item = backlog.change(id, &change, now_secs())?;
            drop(backlog);
            for s in running {
                // Gone already is as good as stopped.
                if let Err(e) = self.kill_session(s) {
                    tracing::warn!(id, session = s, "returned item's session: {e}");
                }
            }
            (item, current.status)
        };
        if was == ItemStatus::Proposed && item.status == ItemStatus::Open {
            self.tell_filer(&item);
        }
        let (item, note) = self.after_change(item).await?;
        self.refresh_items();
        Ok(Reply::Item {
            item: Box::new(item),
            note,
        })
    }

    /// Tells the session that filed `item`, if it's still running, that the user
    /// accepted it, on its next prompt, so the loop ADR-0018 starts closes without
    /// the user relaying it by hand (backlog #53).
    fn tell_filer(&self, item: &Item) {
        let Some((boot, id)) = item.filed_by else {
            return;
        };
        if boot != self.boot {
            return;
        }
        if let Some(session) = self.get(id).ok().filter(|s| !s.exited()) {
            session.filed_accepted(item.id);
        }
    }

    /// What `session`'s agent is told of the items it filed that the user
    /// accepted since its last prompt: one line each, for those still open and
    /// nobody's on (it may have taken one already, or another session did).
    pub(crate) fn accepted_for(&self, session: &crate::session::Session) -> Vec<String> {
        let ids = session.take_accepted();
        if ids.is_empty() {
            return Vec::new();
        }
        let backlog = self.backlog.lock().unwrap();
        ids.into_iter()
            .filter_map(|id| backlog.get(id).ok())
            .filter(|item| item.status == ItemStatus::Open && self.holders(item, None).is_empty())
            .map(|item| backlog::accepted(&item))
            .collect()
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
    pub(crate) fn holders(&self, item: &Item, mine: Option<SessionId>) -> Vec<SessionId> {
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
    /// session (`mine`: on a launch, only the user's) doesn't count: several agents on one backlog mustn't end up
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
        caller: Option<SessionId>,
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
            _ if agent && caller == Some(blocker_of(how, launched, newest)) => {
                "you're already on it"
            }
            _ if agent => "leave it to that session",
            Claim::Take { .. } => "stop it first or take it with --steal",
            Claim::Launch => "attach to it, or stop it first",
        };
        let blocker = blocker_of(how, launched, newest);
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
                Some(info.id),
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

    /// Writes an item down for a live agent session started outside the backlog
    /// and links it there, the user only (`ClientMsg::AdoptSession`). Its agent is
    /// told on its next prompt, having no `VALK_ITEM`.
    pub(crate) async fn adopt_session(
        &self,
        id: SessionId,
        title: Option<String>,
        kind: valkyrie_proto::ItemKind,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        self.vouch(peer, "adopting a session into the backlog")?;
        let session = self.get(id)?;
        let info = session.info();
        if session.exited() {
            bail!("session {id} has exited");
        }
        if !matches!(info.status.agent.as_str(), "claude" | "codex") {
            bail!("session {id} isn't running claude or codex; only an agent's lane can follow it");
        }
        if let Some(on) = self.item_of(id) {
            bail!("session {id} is already on #{}", on.id);
        }
        let title = title
            .or_else(|| session.own_name())
            .or_else(|| session.activity().prompt().map(short_title))
            .filter(|t| !t.trim().is_empty());
        let Some(title) = title else {
            bail!("session {id} has no name or prompt to title it by; give it a title");
        };
        let cwd = info.cwd.clone();
        let branch = tokio::task::spawn_blocking(move || adopted_branch(&cwd)).await?;
        let new = NewItem {
            title,
            body: String::new(),
            kind,
            cwd: Some(info.cwd.clone()),
            ..NewItem::default()
        };
        let item = {
            let backlog = self.backlog.lock().unwrap();
            // Checked again under the lock that records it, as a take is.
            if let Some(on) = self.item_of_in(&backlog.load(), id) {
                bail!("session {id} is already on #{}", on.id);
            }
            let now = now_secs();
            let added = backlog.add(&new, "human", now)?;
            backlog.adopted(added.id, id, self.boot, branch.as_deref(), now)?
        };
        session.adopt_into_item(item.id);
        tracing::info!(
            id = item.id,
            session = id,
            "adopted a session into the backlog"
        );
        self.refresh_items();
        Ok(Reply::Item {
            item: Box::new(item),
            note: None,
        })
    }

    /// The item the caller's session is on (`ClientMsg::MyItem`).
    pub(crate) fn my_item(&self, peer: Option<Peer>) -> Result<Reply> {
        let Some(session) = self.caller(peer).session else {
            bail!("not in a valk session; say which item");
        };
        let id = session.info().id;
        let Some(item) = self.item_of(id) else {
            bail!("this session isn't on a backlog item; say which");
        };
        Ok(Reply::Item {
            item: Box::new(item),
            note: None,
        })
    }

    /// The item live session `id` is on: the one it took or was launched on last,
    /// when several.
    pub(crate) fn item_of(&self, id: SessionId) -> Option<Item> {
        let items = self.backlog.lock().unwrap().load();
        self.item_of_in(&items, id)
    }

    fn item_of_in(&self, items: &[Item], id: SessionId) -> Option<Item> {
        items
            .iter()
            .filter(|i| i.boot == self.boot && i.sessions.contains(&id))
            .filter(|i| matches!(i.status, ItemStatus::Open | ItemStatus::Proposed))
            // Written before `joined` was: when it was last touched, as it was.
            .max_by_key(|i| (i.joined_at(id).unwrap_or(i.updated), i.updated))
            .cloned()
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
            let Some(done) = self.ship_judged(&item).await else {
                let why = "changed since it was checked".to_owned();
                kept.push(KeptItem { id, title, why });
                continue;
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

    /// Ships `judged` as `ship` does (its worktree and merged branch go by the same
    /// rules), but only as it was judged: `None` if it was shipped, dropped, or
    /// moved to another branch, checkout or pull request meanwhile. Shipped while
    /// proposed, it's marked unaccepted (ADR-0012 §3). Who may is the caller's to
    /// check: the user's `ship --merged`, or the daemon on a merged PR.
    async fn ship_judged(&self, judged: &Item) -> Option<Result<(Item, Option<String>)>> {
        let id = judged.id;
        let changed = {
            let backlog = self.backlog.lock().unwrap();
            match backlog.get(id) {
                Ok(now)
                    if now.status == judged.status
                        && now.branch == judged.branch
                        && now.checkout == judged.checkout
                        && now.pr == judged.pr =>
                {
                    let ship = ItemChange::Status {
                        status: ItemStatus::Shipped,
                    };
                    backlog.change(id, &ship, now_secs())
                }
                _ => return None,
            }
        };
        Some(match changed {
            Ok(item) => self.after_change(item).await,
            Err(e) => Err(e),
        })
    }

    /// Ships every open or proposed item whose pull request GitHub reports merged,
    /// with a note on its card naming the PR (ADR-0015 §2): the user's merge is
    /// the review, and the merge guard (#40) keeps merging theirs. A PR closed
    /// without merging ships nothing and is marked `pr_closed` for the board (open
    /// again, the mark goes); one `lookup` can't read (no `gh`, another host,
    /// offline) changes nothing, and `ship --merged` still covers those. `lookup` asks
    /// about a PR's URL from a directory, and may block. Returns how many shipped.
    pub(crate) async fn ship_merged_prs(&self, lookup: PrLookup) -> usize {
        let items = self.backlog.lock().unwrap().load();
        let mut shipped = 0;
        let mut changed = false;
        for item in items {
            if !matches!(item.status, ItemStatus::Open | ItemStatus::Proposed) {
                continue;
            }
            let Some(url) = item.pr.clone() else {
                continue;
            };
            // `gh pr view <url>` needs no repository, but its own is the likeliest
            // to have the host's credentials configured.
            let dir = item
                .project
                .clone()
                .filter(|p| p.is_dir())
                .unwrap_or_else(std::env::temp_dir);
            let ask = Arc::clone(&lookup);
            let asked = url.clone();
            let pr = tokio::task::spawn_blocking(move || ask(&dir, &asked))
                .await
                .unwrap_or(Pr::Unknown);
            let id = item.id;
            // How the steps policies took turned out, before a new flag can follow.
            self.judge_steps(&url, &pr);
            if let Pr::Open {
                failed, conflicts, ..
            } = &pr
                && let Some(summary) = pr_summary(&url, failed, conflicts.as_deref())
            {
                for (session, seq) in self.flag_pr(&item, &url, &summary) {
                    self.offer_step(
                        &crate::supervise::Watched::of_item(&item),
                        session,
                        seq,
                        &summary,
                        &pr,
                    )
                    .await;
                }
            }
            let base = match &pr {
                Pr::Merged { base, .. } => base.clone(),
                _ => String::new(),
            };
            let closed = match pr {
                Pr::Merged { .. } => None,
                Pr::Closed { .. } => Some(true),
                Pr::Open { .. } => Some(false),
                Pr::None | Pr::Unknown => continue,
            };
            if let Some(closed) = closed {
                match self.backlog.lock().unwrap().pr_closed(id, &url, closed) {
                    Ok(Some(_)) => changed = true,
                    Ok(None) => {}
                    Err(e) => tracing::warn!(id, "couldn't record its PR's state: {e:#}"),
                }
                continue;
            }
            // Before it ships: shipping takes the checkout off the card.
            let own = self.item_sessions(&item);
            let note = match self.ship_judged(&item).await {
                None => continue,
                Some(Err(e)) => {
                    tracing::warn!(id, "couldn't ship on a merged PR: {e:#}");
                    continue;
                }
                Some(Ok((_, note))) => note.filter(|n| !n.is_empty()),
            };
            let mut text = format!("Shipped: PR {url} merged.");
            if let Some(note) = note {
                text = format!("{text}\n{note}");
            }
            if let Err(e) =
                self.backlog
                    .lock()
                    .unwrap()
                    .note(id, "valkyrie", &text, false, now_secs())
            {
                tracing::warn!(id, "couldn't note the ship on its card: {e:#}");
            }
            tracing::info!(id, pr = url, "shipped a backlog item: its PR merged");
            shipped += 1;
            // Its sessions are the branch watch's from now on: not news there again.
            self.supervisor.lock().unwrap().open_seen.remove(&url);
            if let Some(root) = Self::root_of(&item) {
                self.announce_merge(&url, &base, &own, &root, Some(&item));
            }
        }
        if shipped > 0 || changed {
            self.refresh_items();
        }
        shipped
    }

    /// Shows an item's idle sessions as blocked on its pull request: red CI
    /// (backlog #67), since ci-fix only sees CI that finished before the agent
    /// stopped and CI usually fails minutes after; or a conflict with its base
    /// (#69), which comes when other work merges, long after the agent's last
    /// stop. Seen once, the same flag isn't raised again; it's raised anew when the
    /// session goes idle again or `summary` changes (`Session::flag_blocked`).
    /// Returns the sessions it flagged, each with the transition it raised.
    fn flag_pr(&self, item: &Item, url: &str, summary: &str) -> Vec<(SessionId, u64)> {
        if item.boot != self.boot {
            return Vec::new();
        }
        let mut flagged = Vec::new();
        for &id in &item.sessions {
            if let Ok(session) = self.get(id)
                && let Some(seq) = session.flag_blocked(summary)
            {
                tracing::info!(
                    item = item.id,
                    session = id,
                    pr = url,
                    summary,
                    "flagged a PR"
                );
                flagged.push((id, seq));
            }
        }
        flagged
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
        if let Some(project) = item.project.clone()
            && valkyrie_context::order::names_order(text)
        {
            self.noticed(crate::order::Trigger::Named(project, id));
        }
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
    /// itself without `worktree`). `model` as `ClientMsg::LaunchItem` has it.
    /// Returns the spawn's reply.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn launch_item(
        self: &Arc<Self>,
        id: u32,
        agent: &str,
        model: Option<String>,
        worktree: bool,
        size: Size,
        env: Vec<(String, Option<String>)>,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        if !matches!(agent, "claude" | "codex") {
            bail!("an item starts claude or codex, not {agent}");
        }
        let item = self.backlog.lock().unwrap().get(id)?;
        let model = match model {
            Some(model) => model.trim().to_owned(),
            None => item
                .launch
                .as_ref()
                .filter(|l| l.agent == agent)
                .map(|l| l.model.clone())
                .unwrap_or_default(),
        };
        check_model(&model)?;
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
        // The user's own session doesn't count; an agent's does, so it can't put a
        // second live session on an item it holds.
        let mine = match caller.agent {
            None => caller.session.as_ref().map(|s| s.info().id),
            Some(_) => None,
        };
        let (from, _launching) = {
            let backlog = self.backlog.lock().unwrap();
            let from = self.claim(
                &backlog.get(id)?,
                mine,
                caller.agent.is_some(),
                caller.session.as_ref().map(|s| s.info().id),
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
        let items = self.backlog.lock().unwrap().load();
        let spec = SpawnSpec {
            command: agent_command(agent, &model, backlog::brief(&item, &items)),
            cwd: Some(cwd),
            name: Some(name),
            size,
            env,
            worktree: new_worktree.clone(),
            hidden: false,
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
        let launch = ItemLaunch {
            agent: agent.to_owned(),
            model,
        };
        self.backlog.lock().unwrap().launched_with(id, launch)?;
        tracing::info!(id, session = info.id, "launched a backlog item");
        self.refresh_items();
        Ok(reply)
    }
}

/// An item's title from what its session was last asked: one line, kept short.
/// What's wrong with an open pull request, short enough for the queue: "CI red
/// on PR #N: <the checks that failed>", "PR #N conflicts with <base>", or both.
/// `None` when it's neither.
pub(crate) fn pr_summary(url: &str, failed: &[Check], conflicts: Option<&str>) -> Option<String> {
    const SHOWN: usize = 3;
    let pr = url
        .rsplit_once("/pull/")
        .map_or("its pull request".to_owned(), |(_, n)| format!("PR #{n}"));
    let red = (!failed.is_empty()).then(|| {
        let mut names: Vec<&str> = failed.iter().take(SHOWN).map(|c| c.name.as_str()).collect();
        let more = failed.len().saturating_sub(SHOWN);
        let more = (more > 0).then(|| format!("{more} more"));
        names.extend(more.as_deref());
        names.join(", ")
    });
    Some(match (conflicts, red) {
        (None, None) => return None,
        (None, Some(red)) => format!("CI red on {pr}: {red}"),
        (Some(base), None) => format!("{pr} conflicts with {base}"),
        (Some(base), Some(red)) => format!("{pr} conflicts with {base}; CI red: {red}"),
    })
}

fn short_title(prompt: &str) -> String {
    const MAX: usize = 80;
    let prompt = backlog::one_line(prompt);
    let prompt = prompt.trim();
    if prompt.chars().count() <= MAX {
        return prompt.to_owned();
    }
    let cut: String = prompt.chars().take(MAX - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The branch an adopted session's checkout is on, unless it's the repository's
/// default (work there has no branch of its own to ship by) or detached.
fn adopted_branch(cwd: &std::path::Path) -> Option<String> {
    if !valkyrie_context::project::in_git(cwd) {
        return None;
    }
    let branch = worktree::git_out(cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()?;
    let default = worktree::default_branch(&valkyrie_context::project::root(cwd));
    (!branch.is_empty() && default.as_deref() != Some(branch.as_str())).then_some(branch)
}

/// The command that starts `agent` on `brief`, on `model` unless it's empty.
/// What tests start instead of a real agent: `sleep`, never `claude` itself.
#[cfg(test)]
pub(crate) static AGENT_STANDIN: std::sync::Mutex<Option<Vec<String>>> =
    std::sync::Mutex::new(None);

fn agent_command(agent: &str, model: &str, brief: String) -> Vec<String> {
    #[cfg(test)]
    if let Some(standin) = AGENT_STANDIN.lock().unwrap().clone() {
        return standin;
    }
    let mut command = vec![agent.to_owned()];
    if !model.is_empty() {
        let flag = if agent == "codex" { "-m" } else { "--model" };
        command.extend([flag.to_owned(), model.to_owned()]);
    }
    command.push(brief);
    command
}

/// Refuses a model name that isn't one: it goes on the agent's command line, so
/// it mustn't read as a flag or carry spaces.
fn check_model(model: &str) -> Result<()> {
    const MAX: usize = 100;
    let fits = |c: char| c.is_ascii_alphanumeric() || "._-:/[]@".contains(c);
    if model.len() > MAX || model.starts_with('-') || !model.chars().all(fits) {
        bail!("{model:?} isn't a model name");
    }
    Ok(())
}

/// The session a refused claim names: the launch's own, else the newest holder.
fn blocker_of(how: Claim, launched: Option<SessionId>, newest: SessionId) -> SessionId {
    match how {
        Claim::Launch => launched.unwrap_or(newest),
        Claim::Take { .. } => newest,
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
    use std::sync::Mutex;

    #[test]
    fn a_model_goes_on_the_command_line_as_each_agent_takes_it() {
        let brief = || "the brief".to_owned();
        assert_eq!(
            agent_command("claude", "", brief()),
            ["claude", "the brief"]
        );
        assert_eq!(
            agent_command("claude", "opus", brief()),
            ["claude", "--model", "opus", "the brief"]
        );
        assert_eq!(
            agent_command("codex", "gpt-5-codex", brief()),
            ["codex", "-m", "gpt-5-codex", "the brief"]
        );
    }

    #[test]
    fn only_a_model_name_reaches_the_command_line() {
        for ok in ["", "opus", "claude-opus-5-5", "claude-sonnet-4-5[1m]", "o3"] {
            assert!(check_model(ok).is_ok(), "{ok}");
        }
        for bad in ["--dangerously-skip-permissions", "opus haiku", "a;b", "x\n"] {
            assert!(check_model(bad).is_err(), "{bad}");
        }
        assert!(check_model(&"a".repeat(101)).is_err());
    }

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
            ..Default::default()
        };
        let item = registry
            .backlog
            .lock()
            .unwrap()
            .add(&new, "human", 1)
            .unwrap();
        let take = |steal| registry.claim(&item, Some(3), false, Some(3), Claim::Take { steal });
        assert_eq!(take(false).unwrap(), Vec::<SessionId>::new());
        {
            registry.launching.lock().unwrap().insert(item.id);
            let _launching = Launching(&registry, item.id);
            for steal in [false, true] {
                let refused = take(steal).unwrap_err().to_string();
                assert!(refused.contains("being launched right now"), "{refused}");
            }
            let refused = registry.claim(&item, None, false, None, Claim::Launch);
            assert!(refused.is_err(), "nor a second launch");
        }
        // Its guard dropped, whatever became of the launch: free again.
        assert!(take(false).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_merged_pr_ships_its_item_and_nothing_else_does() {
        let dir = std::env::temp_dir().join(format!("valkyrie-prship-{}", std::process::id()));
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
        let new = |title: &str| NewItem {
            title: title.into(),
            body: String::new(),
            kind: Default::default(),
            cwd: None,
            attachments: Vec::new(),
            ..Default::default()
        };
        let url = |n: u32| format!("https://github.com/o/r/pull/{n}");
        let (merged, closed, unknown, proposed, none) = {
            let b = registry.backlog.lock().unwrap();
            let with_pr = |title: &str, n: u32, filed_by: Option<(u64, SessionId)>| {
                let by = if filed_by.is_some() {
                    "claude"
                } else {
                    "human"
                };
                let item = b.add_filed(&new(title), by, filed_by, 1).unwrap();
                if filed_by.is_none() {
                    let open = ItemChange::Status {
                        status: ItemStatus::Open,
                    };
                    b.change(item.id, &open, 1).unwrap();
                }
                let pr = url(n);
                b.taken(item.id, 3, 7, &[], None, Some(&pr), None, 1)
                    .unwrap()
                    .id
            };
            let ids = (
                with_pr("merged", 1, None),
                with_pr("closed", 2, None),
                with_pr("unknown", 3, None),
                with_pr("an agent's own", 4, Some((7, 3))),
            );
            let none = b.add(&new("no pr"), "human", 1).unwrap().id;
            (ids.0, ids.1, ids.2, ids.3, none)
        };
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&asked);
        let lookup: PrLookup = Arc::new(move |_, pr| {
            seen.lock().unwrap().push(pr.to_owned());
            let url = pr.to_owned();
            match pr.rsplit('/').next() {
                Some("1" | "4") => Pr::merged(url),
                Some("2") => Pr::Closed { url },
                _ => Pr::Unknown,
            }
        });
        assert_eq!(registry.ship_merged_prs(lookup.clone()).await, 2);
        let get = |id| registry.backlog.lock().unwrap().get(id).unwrap();
        let item = get(merged);
        assert_eq!(item.status, ItemStatus::Shipped);
        assert!(!item.unaccepted);
        let note = item.notes.last().unwrap();
        assert_eq!(note.by, "valkyrie");
        assert!(note.text.contains(&url(1)), "{}", note.text);
        // Merged accepts the work, not the proposal: reopened, it's proposed again.
        let item = get(proposed);
        assert_eq!((item.status, item.unaccepted), (ItemStatus::Shipped, true));
        for id in [closed, unknown] {
            assert_eq!(get(id).status, ItemStatus::Open);
            assert!(get(id).notes.is_empty());
        }
        assert_eq!(get(none).status, ItemStatus::Open);
        // Closed is marked for the board; unreadable says nothing either way.
        assert!(get(closed).pr_closed);
        assert!(!get(unknown).pr_closed);
        // Only items with a PR are asked about; shipped ones aren't asked again.
        assert_eq!(asked.lock().unwrap().len(), 4);
        assert_eq!(registry.ship_merged_prs(lookup).await, 0);
        assert_eq!(*asked.lock().unwrap(), [1, 2, 3, 4, 2, 3].map(url).to_vec());
        // Reopened on GitHub, the mark goes.
        let reopened: PrLookup = Arc::new(|_, pr| Pr::Open {
            url: pr.to_owned(),
            failed: vec![],
            conflicts: None,
            head: String::new(),
            settled: true,
            author: String::new(),
        });
        assert_eq!(registry.ship_merged_prs(reopened).await, 0);
        assert!(!get(closed).pr_closed);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A PR that goes red after its session's last stop shows that session as
    /// blocked (backlog #67), once, and so does one that conflicts with its base
    /// after main moved (#69); a busy session, or another boot's, is left be.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_red_or_conflicting_pr_flags_its_idle_session() {
        use valkyrie_proto::AgentState;
        let dir = std::env::temp_dir().join(format!("valkyrie-red-{}", std::process::id()));
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
        let (idle, busy, old, behind) = (spawn(), spawn(), spawn(), spawn());
        let url = |n: u32| format!("https://github.com/o/r/pull/{n}");
        let with_pr = |session: &Arc<crate::session::Session>, n: u32, boot: u64| {
            let b = registry.backlog.lock().unwrap();
            let new = NewItem {
                title: format!("item {n}"),
                ..Default::default()
            };
            let item = b.add(&new, "human", 1).unwrap();
            let open = ItemChange::Status {
                status: ItemStatus::Open,
            };
            b.change(item.id, &open, 1).unwrap();
            let id = session.info().id;
            b.taken(item.id, id, boot, &[], None, Some(&url(n)), None, 1)
                .unwrap()
                .id
        };
        with_pr(&idle, 1, registry.boot);
        with_pr(&busy, 2, registry.boot);
        with_pr(&old, 3, registry.boot + 1);
        with_pr(&behind, 4, registry.boot);
        // A turn under way: news of its PR waits for it to stop.
        busy.hook(
            "claude",
            1,
            serde_json::json!({"hook_event_name": "UserPromptSubmit", "prompt": "fix it"}),
        );
        let check = |name: &str| Check {
            name: name.into(),
            url: String::new(),
            run: None,
        };
        // PR #4 is green but six merges landed on main since it was pushed.
        let red: PrLookup = Arc::new(move |_, pr| {
            let behind = pr.ends_with("/4");
            Pr::Open {
                url: pr.to_owned(),
                failed: if behind {
                    vec![]
                } else {
                    vec![check("windows"), check("lint")]
                },
                conflicts: behind.then(|| "main".to_owned()),
                head: String::new(),
                settled: true,
                author: String::new(),
            }
        });
        let state = |s: &Arc<crate::session::Session>| s.info().status;
        assert_eq!(registry.ship_merged_prs(red.clone()).await, 0);
        let flagged = state(&idle);
        assert_eq!(flagged.state, AgentState::Blocked);
        assert_eq!(
            flagged.summary.as_deref(),
            Some("CI red on PR #1: windows, lint")
        );
        assert!(!flagged.seen);
        assert_eq!(state(&busy).state, AgentState::Working);
        assert_eq!(state(&old).state, AgentState::Idle);
        let conflicted = state(&behind);
        assert_eq!(conflicted.state, AgentState::Blocked);
        assert_eq!(
            conflicted.summary.as_deref(),
            Some("PR #4 conflicts with main")
        );
        // Each flag offers its step (ADR-0019 §2): red outside Actions can only be
        // fixed, a conflict merges the base in; nothing on a busy session.
        {
            use crate::context::{AS_CALLER, Caller};
            use valkyrie_proto::StepAction;
            let flag = |s: &Arc<crate::session::Session>| registry.flag_of(s, s.info().id);
            let red_flag = flag(&idle).unwrap();
            let red_step = red_flag.step.clone().unwrap();
            assert_eq!(red_step.action, StepAction::CiFix);
            assert!(
                red_step.why.contains("isn't GitHub Actions"),
                "{}",
                red_step.why
            );
            assert_eq!(red_flag.pr, url(1));
            let conflict = flag(&behind).unwrap();
            assert_eq!(
                conflict.step.as_ref().unwrap().action,
                StepAction::MergeBase
            );
            assert!(flag(&busy).is_none() && flag(&old).is_none());
            let queued = registry.compute_queue();
            let row = queued
                .iter()
                .find(|q| q.session == behind.info().id)
                .unwrap();
            assert_eq!(row.flag.as_ref(), Some(&conflict));
            let as_caller = |agent: Option<&str>| {
                let agent = agent.map(str::to_owned);
                AS_CALLER.with(|c| {
                    *c.borrow_mut() = Some(Box::new(move || Caller {
                        agent: agent.clone(),
                        session: None,
                        bridge: false,
                        desktop: false,
                    }))
                });
            };
            let behind_id = behind.info().id;
            as_caller(Some("claude"));
            let refused = registry
                .take_step(behind_id, conflict.seq, StepAction::MergeBase, None)
                .await;
            assert!(refused.is_err_and(|e| e.to_string().contains("is for the user")));
            as_caller(None);
            let stale = registry
                .take_step(behind_id, conflict.seq + 1, StepAction::MergeBase, None)
                .await;
            assert!(stale.is_err_and(|e| e.to_string().contains("moved on")));
            let other = registry
                .take_step(behind_id, conflict.seq, StepAction::Rerun, None)
                .await;
            assert!(other.is_err_and(|e| e.to_string().contains("isn't offered")));
            let Reply::StepTaken { said } = registry
                .take_step(behind_id, conflict.seq, StepAction::MergeBase, None)
                .await
                .unwrap()
            else {
                panic!("no StepTaken");
            };
            assert_eq!(said, format!("asked session {behind_id} to merge main in"));
            // Taken, it isn't offered again; the flag stays until the agent starts.
            let taken = flag(&behind).unwrap();
            assert!(taken.step.is_none() && !taken.working);
            // Left: the flag stays, the offer goes.
            registry
                .take_step(idle.info().id, red_flag.seq, StepAction::Leave, None)
                .await
                .unwrap();
            let left = flag(&idle).unwrap();
            assert_eq!(left.reason, red_flag.reason);
            assert!(left.step.is_none());
            AS_CALLER.with(|c| *c.borrow_mut() = None);
        }
        // Looked at, it isn't raised again while the PR stays red.
        idle.mark_seen(flagged.seq);
        assert_eq!(registry.ship_merged_prs(red).await, 0);
        assert_eq!(state(&idle).seq, flagged.seq);
        assert!(state(&idle).seen);
        // Green, or still running, says nothing.
        let green: PrLookup = Arc::new(|_, pr| Pr::Open {
            url: pr.to_owned(),
            failed: vec![],
            conflicts: None,
            head: String::new(),
            settled: true,
            author: String::new(),
        });
        registry.ship_merged_prs(green).await;
        assert_eq!(state(&busy).state, AgentState::Working);
        assert_eq!(state(&idle).seq, flagged.seq);
        // Still blocked on its red CI when main moves: the new reason replaces the
        // old flag, and shows as news again.
        let conflicting: PrLookup = Arc::new(move |_, pr| Pr::Open {
            url: pr.to_owned(),
            failed: vec![check("windows")],
            conflicts: Some("main".to_owned()),
            head: String::new(),
            settled: true,
            author: String::new(),
        });
        registry.ship_merged_prs(conflicting.clone()).await;
        let both = state(&idle);
        assert_eq!(
            both.summary.as_deref(),
            Some("PR #1 conflicts with main; CI red: windows")
        );
        assert!(both.seq > flagged.seq && !both.seen);
        idle.mark_seen(both.seq);
        registry.ship_merged_prs(conflicting).await;
        assert_eq!(state(&idle).seq, both.seq);

        for s in [idle, busy, old, behind] {
            s.kill();
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_pr_summary_names_the_pr_its_conflict_and_a_few_checks() {
        let check = |name: &str| Check {
            name: name.into(),
            url: String::new(),
            run: None,
        };
        let url = "https://github.com/o/r/pull/101";
        let one = [check("windows")];
        let summary = |url, failed, conflicts| pr_summary(url, failed, conflicts).unwrap();
        assert_eq!(summary(url, &one, None), "CI red on PR #101: windows");
        let five: Vec<Check> = ["a", "b", "c", "d", "e"].map(check).to_vec();
        assert_eq!(
            summary("https://example.com/x", &five, None),
            "CI red on its pull request: a, b, c, 2 more"
        );
        // Conflicting with its base (backlog #69), alone or with red CI.
        assert_eq!(
            summary(url, &[], Some("main")),
            "PR #101 conflicts with main"
        );
        assert_eq!(
            summary(url, &one, Some("main")),
            "PR #101 conflicts with main; CI red: windows"
        );
        // Green, or still running, and merging cleanly: nothing to say.
        assert_eq!(pr_summary(url, &[], None), None);
    }

    /// The user accepts an agent's proposal while the daemon works out that the
    /// agent rewording or dropping it filed it (backlog #61): the change is judged
    /// against the item as it is when written, not as it was a moment before.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_change_is_judged_on_the_item_as_written() {
        use crate::context::{AS_CALLER, Caller};
        let dir = std::env::temp_dir().join(format!("valkyrie-race-{}", std::process::id()));
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
        let session = registry.get(info.id).unwrap();
        let new = |title: &str| NewItem {
            title: title.into(),
            body: String::new(),
            kind: Default::default(),
            cwd: None,
            attachments: Vec::new(),
            ..Default::default()
        };
        let filed = |title: &str| {
            registry
                .backlog
                .lock()
                .unwrap()
                .add_filed(&new(title), "claude", Some((registry.boot, info.id)), 1)
                .unwrap()
                .id
        };
        let (reworded, dropped) = (filed("reworded"), filed("dropped"));
        let open = ItemChange::Status {
            status: ItemStatus::Open,
        };
        let ship = ItemChange::Status {
            status: ItemStatus::Shipped,
        };
        // The agent that filed both; as it's worked out, the user accepts #1 and
        // ships #2.
        let (r2, s2) = (Arc::clone(&registry), Arc::clone(&session));
        AS_CALLER.with(|c| {
            *c.borrow_mut() = Some(Box::new(move || {
                let b = r2.backlog.lock().unwrap();
                for (id, change) in [(reworded, &open), (dropped, &ship)] {
                    if b.get(id).unwrap().status == ItemStatus::Proposed {
                        b.change(id, change, 2).unwrap();
                    }
                }
                Caller {
                    agent: Some("claude".into()),
                    session: Some(Arc::clone(&s2)),
                    bridge: false,
                    desktop: false,
                }
            }))
        });
        let edit = ItemChange::Edit {
            priority: None,
            blocked_by: None,
            title: Some("Hijacked".into()),
            body: None,
            kind: None,
            cwd: None,
        };
        let refused = registry.change_item(reworded, edit, None).await;
        assert!(refused.is_err(), "reworded an accepted item: {refused:?}");
        let drop = ItemChange::Status {
            status: ItemStatus::Dropped,
        };
        let refused = registry.change_item(dropped, drop, None).await;
        assert!(refused.is_err(), "dropped a shipped item: {refused:?}");
        AS_CALLER.with(|c| *c.borrow_mut() = None);
        let get = |id| registry.backlog.lock().unwrap().get(id).unwrap();
        assert_eq!(get(reworded).title, "reworded");
        assert_eq!(get(reworded).status, ItemStatus::Open);
        assert_eq!(get(dropped).status, ItemStatus::Shipped);
        session.kill();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn the_session_that_filed_an_item_hears_once_when_the_user_accepts_it() {
        use crate::context::{AS_CALLER, Caller};
        let dir = std::env::temp_dir().join(format!("valkyrie-accepted-{}", std::process::id()));
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
        let (filer, gone) = (spawn(), spawn());
        let file = |title: &str, by: Option<SessionId>| {
            let new = NewItem {
                title: title.into(),
                ..Default::default()
            };
            let filed_by = by.map(|s| (registry.boot, s));
            let who = if by.is_some() { "claude" } else { "human" };
            registry
                .backlog
                .lock()
                .unwrap()
                .add_filed(&new, who, filed_by, 1)
                .unwrap()
                .id
        };
        let mine = file("Mine", Some(filer.info().id));
        let taken = file("Taken", Some(filer.info().id));
        let users = file("The user's", None);
        let orphan = file("Orphan", Some(gone.info().id));
        gone.kill();
        while !gone.exited() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        // The user, at the board.
        AS_CALLER.with(|c| {
            *c.borrow_mut() = Some(Box::new(|| Caller {
                agent: None,
                session: None,
                bridge: false,
                desktop: false,
            }))
        });
        let accept = || ItemChange::Status {
            status: ItemStatus::Open,
        };
        for id in [mine, taken, users, orphan] {
            registry.change_item(id, accept(), None).await.unwrap();
        }
        // An edit to an accepted item isn't an accept: no second notice.
        registry
            .change_item(
                mine,
                ItemChange::Edit {
                    priority: None,
                    blocked_by: None,
                    title: Some("Mine".into()),
                    body: None,
                    kind: None,
                    cwd: None,
                },
                None,
            )
            .await
            .unwrap();
        assert!(
            gone.take_accepted().is_empty(),
            "told a session that's gone"
        );
        // Taken by the filer before its next prompt: nothing to tell about that one.
        registry
            .backlog
            .lock()
            .unwrap()
            .taken(
                taken,
                filer.info().id,
                registry.boot,
                &[],
                None,
                None,
                None,
                2,
            )
            .unwrap();
        let said = registry.accepted_for(&filer);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0].contains(&format!("accepted #{mine} “Mine” you filed")),
            "{}",
            said[0]
        );
        assert!(
            said[0].contains(&format!("valk todo take {mine}")),
            "{}",
            said[0]
        );
        assert!(registry.accepted_for(&filer).is_empty(), "said twice");
        filer.kill();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_session_is_on_the_item_it_took_last_not_the_one_it_last_noted() {
        let dir = std::env::temp_dir().join(format!("valkyrie-item-of-{}", std::process::id()));
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
        let session = 5;
        let (first, second) = {
            let backlog = registry.backlog.lock().unwrap();
            let add = |title: &str| {
                let new = NewItem {
                    title: title.into(),
                    ..Default::default()
                };
                backlog.add(&new, "human", 1).unwrap().id
            };
            let (first, second) = (add("First"), add("Second"));
            let boot = registry.boot;
            backlog
                .taken(first, session, boot, &[], Some("fix/a"), None, None, 10)
                .unwrap();
            backlog
                .taken(second, session, boot, &[], Some("fix/b"), None, None, 20)
                .unwrap();
            (first, second)
        };
        assert_eq!(registry.item_of(session).map(|i| i.id), Some(second));
        // A handoff left on the first, still open, after moving on (backlog #74).
        registry
            .backlog
            .lock()
            .unwrap()
            .note(first, "claude", "done here", true, 30)
            .unwrap();
        assert_eq!(registry.item_of(session).map(|i| i.id), Some(second));
        // Taking the first again, say to record its PR, is moving back to it.
        registry
            .backlog
            .lock()
            .unwrap()
            .taken(
                first,
                session,
                registry.boot,
                &[],
                None,
                Some("https://github.com/o/r/pull/1"),
                None,
                40,
            )
            .unwrap();
        assert_eq!(registry.item_of(session).map(|i| i.id), Some(first));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

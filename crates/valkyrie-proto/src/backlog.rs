//! The backlog (DESIGN §8.12): bugs, features and chores written down for later,
//! each of which can start an agent session on itself.

use crate::SessionId;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What sort of work an item is; it picks the brief its agent starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Bug,
    #[default]
    Feature,
    Chore,
}

impl ItemKind {
    pub const ALL: [ItemKind; 3] = [Self::Bug, Self::Feature, Self::Chore];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Bug => "bug",
            Self::Feature => "feature",
            Self::Chore => "chore",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// How soon an item should be done: `P0` first. Within a lane, items are worked
/// in this order (`order`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    P0,
    P1,
    #[default]
    P2,
    P3,
}

impl Priority {
    pub const ALL: [Priority; 4] = [Self::P0, Self::P1, Self::P2, Self::P3];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::P0 => "p0",
            Self::P1 => "p1",
            Self::P2 => "p2",
            Self::P3 => "p3",
        }
    }

    /// `p1` or `P1`; a bare `1` too.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        let s = s.strip_prefix('p').unwrap_or(&s);
        Self::ALL.into_iter().find(|p| &p.as_str()[1..] == s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    /// Not done yet, whether or not an agent has started on it.
    #[default]
    Open,
    Shipped,
    /// Not going to happen; kept so the list remembers it.
    Dropped,
    /// Written down by an agent, waiting for the user to accept it before it can
    /// be started.
    Proposed,
}

impl ItemStatus {
    pub const ALL: [ItemStatus; 4] = [Self::Open, Self::Shipped, Self::Dropped, Self::Proposed];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Shipped => "shipped",
            Self::Dropped => "dropped",
            Self::Proposed => "proposed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Numbered from 1 across every project, so `valk todo launch 12` needs no folder.
    pub id: u32,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub kind: ItemKind,
    #[serde(default)]
    pub status: ItemStatus,
    #[serde(default)]
    pub priority: Priority,
    /// Items to finish first. Advisory: one still open sorts this below the ready
    /// ones and is named in its brief, but nothing refuses a take or launch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<u32>,
    /// The repository it's for; `None` until one is picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<PathBuf>,
    /// Screenshots and files, copied into the backlog's own folder.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<PathBuf>,
    /// `human`, or the agent that wrote it down.
    #[serde(default)]
    pub by: String,
    /// Sessions started on it, oldest first. Ids are only meaningful to the daemon
    /// that started them: a restarted one reuses them, so clients match on `boot`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SessionId>,
    /// The daemon `boot` that `sessions` belong to.
    #[serde(default)]
    pub boot: u64,
    /// Those of `sessions` that took it (`valk todo take`) rather than being
    /// launched on it: open to any agent, so their notes stay out of the brief.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub taken: Vec<SessionId>,
    /// When each of `sessions` last took or was launched on it, in seconds: the
    /// item a session is on is the one it joined last, which a note on an older
    /// one doesn't change (`updated` does). Pairs, not a map: a map's number keys
    /// are strings in JSON, which a tagged message can't read back as numbers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joined: Vec<(SessionId, u64)>,
    /// The branch and checkout its first launch made, reused by later ones; or
    /// the branch a take recorded (`branch_taken`), with no checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// `branch` was set by `valk todo take`, not a launch: the user may replace it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub branch_taken: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<PathBuf>,
    /// The pull request, an `https://` URL, as `valk todo take --pr` recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<String>,
    /// The daemon last read `pr` as closed without merging: it waits for the user
    /// to drop or relaunch the item. Cleared when the PR reads open again, or a
    /// take records another.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pr_closed: bool,
    /// The daemon `boot` and session of the agent that wrote it down (`valk todo
    /// add`, with or without `--take`). That session may take it while it's still
    /// proposed: its own find, which the user said to go ahead with in that
    /// session (ADR-0018), or its own work tracked (ADR-0012).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filed_by: Option<(u64, SessionId)>,
    /// Shipped or dropped while still proposed: the user never accepted it, so
    /// reopening it makes it proposed again rather than open.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unaccepted: bool,
    /// Handoffs left on it, oldest first: what an agent did and what's left, or
    /// the user's own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<ItemNote>,
    /// The agent and model its last launch ran: the next launch of that agent
    /// keeps the model unless it's given another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<ItemLaunch>,
    /// Seconds since the epoch.
    pub created: u64,
    pub updated: u64,
}

impl Item {
    /// When `session` last took or was launched on it (`joined`).
    pub fn joined_at(&self, session: SessionId) -> Option<u64> {
        self.joined
            .iter()
            .find_map(|&(s, at)| (s == session).then_some(at))
    }

    /// Records `session` taking or being launched on it `now`.
    pub fn join(&mut self, session: SessionId, now: u64) {
        self.joined.retain(|&(s, _)| s != session);
        self.joined.push((session, now));
    }

    /// Written down and taken by an agent for its own work, and not accepted yet:
    /// it follows its session like an open item, and shipping it accepts it.
    /// Filed but not taken, it waits in Proposed like any other proposal.
    pub fn agent_filed(&self) -> bool {
        self.status == ItemStatus::Proposed && self.filed_by.is_some() && !self.taken.is_empty()
    }

    /// Those of `blocked_by` not shipped or dropped yet, as found in `items`; one
    /// deleted is gone, so it waits on nothing.
    pub fn waiting_on(&self, items: &[Item]) -> Vec<u32> {
        self.blocked_by
            .iter()
            .copied()
            .filter(|id| {
                items.iter().any(|i| {
                    i.id == *id && matches!(i.status, ItemStatus::Open | ItemStatus::Proposed)
                })
            })
            .collect()
    }
}

/// The order to work items in, the first first: those waiting on nothing, then by
/// priority, then the oldest. Every client sorts a lane by it, so the top of the
/// backlog is the same wherever you look.
pub fn order(items: &[Item]) -> impl Fn(&Item, &Item) -> std::cmp::Ordering + '_ {
    move |a, b| {
        let waits = |i: &Item| !i.waiting_on(items).is_empty();
        waits(a)
            .cmp(&waits(b))
            .then(a.priority.cmp(&b.priority))
            .then(a.id.cmp(&b.id))
    }
}

/// What an item was last launched with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemLaunch {
    /// `claude` or `codex`.
    pub agent: String,
    /// Empty for the agent's own default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemNote {
    /// Seconds since the epoch.
    pub at: u64,
    /// `human`, or the agent that left it.
    pub by: String,
    pub text: String,
    /// Left by an agent on an item it took rather than was launched for: shown,
    /// but never put into a launch's brief (ADR-0007).
    #[serde(default, skip_serializing_if = "is_false")]
    pub taken: bool,
    /// Left by an agent on an item it was neither launched for nor took, from
    /// work elsewhere: shown, but never put into a launch's brief, as `taken`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub aside: bool,
    /// For an aside, the item that agent was on, when it was on one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<u32>,
}

impl ItemNote {
    /// Whether it goes into the brief of the item's next launch: the user's, or
    /// from an earlier launch of the item. Anything else could be a way into a
    /// prompt the user starts.
    pub fn in_brief(&self) -> bool {
        !self.taken && !self.aside
    }

    /// Where an agent's note came from when that kept it out of the brief, as
    /// the CLI and TUI label it.
    pub fn origin(&self) -> Option<String> {
        match (self.taken, self.aside, self.from) {
            (true, _, _) => Some("from a take".into()),
            (_, true, Some(from)) => Some(format!("while on #{from}")),
            (_, true, None) => Some("from outside the item".into()),
            _ => None,
        }
    }
}

fn is_false(b: &bool) -> bool {
    !b
}

/// What an item's checkout holds, read from git rather than taken from the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemFacts {
    pub branch: String,
    /// Commits on it that the repository's own checkout doesn't have.
    pub ahead: u32,
    /// The newest of them, `<short hash> <subject>`, at most ten.
    pub commits: Vec<String>,
    /// `git diff --shortstat` against where it started, e.g. `3 files changed, 40
    /// insertions(+), 2 deletions(-)`; empty when nothing changed.
    pub diff: String,
    /// Files changed or untracked but not committed.
    pub dirty: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct NewItem {
    /// May be empty: then the body's first line is the title.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub kind: ItemKind,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<u32>,
    /// A folder inside the repository it's for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Files to keep with it (an upload's path, or any file the daemon can read).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum ItemChange {
    /// Rewords it, or moves it to another repository. Only what's given changes,
    /// so an edit made from an older copy doesn't put back what it didn't touch.
    Edit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<ItemKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        priority: Option<Priority>,
        /// The whole list, replacing the one it has; empty clears it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocked_by: Option<Vec<u32>>,
    },
    /// Ships, drops or reopens it. Shipping also takes away a worktree its launch
    /// made, when nothing is left in it.
    Status { status: ItemStatus },
    /// Puts a started item back in the backlog (protocol 28): the user only. The
    /// daemon stops every live session on it and the item forgets them, so it
    /// reads as not started; its branch and checkout stay for the next launch.
    /// Refused for one with a pull request, or that nothing started on.
    Return,
}

/// A build order for one repository's open items (ADR-0019 §2, `order`): what to
/// do first, why, and the priority and `after #N` changes that make the board
/// show it. Never applied on its own: the user accepts all of it, some of it, or
/// none (`ClientMsg::AnswerOrder`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderProposal {
    /// Changes whenever the repository's proposal is replaced, so an answer meant
    /// for an older one is refused.
    pub id: u64,
    /// The repository's root.
    pub project: PathBuf,
    /// Seconds since the epoch. An item edited after this keeps its edit: the
    /// proposal's change to it is skipped.
    pub made: u64,
    /// What asked for it: `you asked`, `3 items added today`, `a note on #49
    /// names an order`.
    pub trigger: String,
    /// The order, the first first. Items it leaves out keep what they have.
    pub steps: Vec<OrderStep>,
}

/// One item's place in an `OrderProposal`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderStep {
    pub item: u32,
    /// One line: why it goes here.
    pub reason: String,
    /// The priority to give it; `None` keeps the one it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
    /// The whole `blocked_by` list to give it; `None` keeps the one it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<Vec<u32>>,
}

impl OrderStep {
    /// The edit accepting it makes; `None` when it only names a place.
    pub fn change(&self) -> Option<ItemChange> {
        (self.priority.is_some() || self.blocked_by.is_some()).then(|| ItemChange::Edit {
            title: None,
            body: None,
            kind: None,
            cwd: None,
            priority: self.priority,
            blocked_by: self.blocked_by.clone(),
        })
    }
}

/// A ready item the supervisor offers to start (ADR-0019 §1, §2 `launch`): open,
/// nobody on it, its blockers shipped or dropped (or the model judged it doesn't
/// need the one left), with room under the repository's `max_running`. One per
/// repository at most. The user starts it or leaves it (`ClientMsg::AnswerLaunch`);
/// once they've started two this way, a policy can start them without asking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchOffer {
    /// Changes whenever the repository's offer is replaced, so an answer meant for
    /// an older one is refused.
    pub id: u64,
    /// The repository's root.
    pub project: PathBuf,
    pub item: u32,
    /// The item's title, for a push or a banner.
    pub title: String,
    /// The agent and model it starts on: the item's last launch, else claude on
    /// its own default model.
    pub agent: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// One line: why this item, now.
    pub why: String,
    /// Unfinished blockers the model judged it doesn't need first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waived: Vec<u32>,
    /// Seconds since the epoch.
    pub made: u64,
}

/// What accepting an `OrderProposal` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OrderApplied {
    /// Items changed.
    pub changed: Vec<u32>,
    /// Items left as they are, and why (edited since, shipped, the change now
    /// refused).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<(u32, String)>,
}

/// An item `ShipMerged` shipped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShippedItem {
    pub id: u32,
    pub title: String,
    /// The branch judged, and whether a take set it rather than a launch.
    #[serde(default)]
    pub branch: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub branch_taken: bool,
    /// An agent filed it and the user hasn't accepted it: shipping it is the
    /// acceptance (`Item::agent_filed`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub filed: bool,
    /// What its branch was found merged into, e.g. `origin/main`.
    pub against: String,
    /// What else happened (its worktree taken away, or why it was kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// An open item with a branch that `ShipMerged` left open, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptItem {
    pub id: u32,
    pub title: String,
    /// `not merged`, `branch fix/x not found`, `no commits of its own`, …
    pub why: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_return_is_a_bare_tag() {
        let back: ItemChange = serde_json::from_str(r#"{"do":"return"}"#).unwrap();
        assert_eq!(back, ItemChange::Return);
        assert_eq!(serde_json::to_string(&back).unwrap(), r#"{"do":"return"}"#);
    }

    #[test]
    fn an_edit_carries_only_what_it_changes() {
        let edit: ItemChange = serde_json::from_str(r#"{"do":"edit","title":"x"}"#).unwrap();
        let ItemChange::Edit {
            title,
            body,
            kind,
            cwd,
            ..
        } = &edit
        else {
            panic!("{edit:?}");
        };
        assert_eq!(
            (title.as_deref(), body, kind, cwd),
            (Some("x"), &None, &None, &None)
        );
        assert_eq!(
            serde_json::to_string(&edit).unwrap(),
            r#"{"do":"edit","title":"x"}"#
        );
        // An older client sends every field, and still gets them all changed.
        let full: ItemChange =
            serde_json::from_str(r#"{"do":"edit","title":"x","body":"y","kind":"bug","cwd":null}"#)
                .unwrap();
        assert!(matches!(
            full,
            ItemChange::Edit {
                body: Some(_),
                kind: Some(ItemKind::Bug),
                ..
            }
        ));
    }

    fn item(id: u32, priority: Priority, blocked_by: Vec<u32>, status: ItemStatus) -> Item {
        let mut i: Item = serde_json::from_str(&format!(
            r#"{{"id":{id},"title":"t","created":0,"updated":0}}"#
        ))
        .unwrap();
        (i.priority, i.blocked_by, i.status) = (priority, blocked_by, status);
        i
    }

    #[test]
    fn items_are_worked_ready_first_then_by_priority_then_oldest() {
        use ItemStatus::*;
        let items = vec![
            item(1, Priority::P2, vec![], Open),
            item(2, Priority::P0, vec![5], Open), // waits on #5, still open
            item(3, Priority::P1, vec![6], Open), // #6 shipped: ready
            item(4, Priority::P1, vec![], Open),
            item(5, Priority::P3, vec![], Open),
            item(6, Priority::P0, vec![], Shipped),
            item(7, Priority::P2, vec![99], Open), // #99 deleted: ready
        ];
        let mut sorted = items.clone();
        sorted.sort_by(order(&items));
        let ids: Vec<u32> = sorted.iter().map(|i| i.id).collect();
        assert_eq!(ids, [6, 3, 4, 1, 7, 5, 2]);
        assert_eq!(items[1].waiting_on(&items), [5]);
    }

    #[test]
    fn old_items_read_as_p2_waiting_on_nothing() {
        let i: Item =
            serde_json::from_str(r#"{"id":1,"title":"t","created":0,"updated":0}"#).unwrap();
        assert_eq!((i.priority, i.blocked_by.len()), (Priority::P2, 0));
        assert_eq!(serde_json::to_value(Priority::P1).unwrap(), "p1");
        assert_eq!(
            ["P0", "1", " p3 ", "p4", ""].map(Priority::parse),
            [
                Some(Priority::P0),
                Some(Priority::P1),
                Some(Priority::P3),
                None,
                None
            ]
        );
    }

    /// An item crosses the wire inside tagged messages (`Reply::Item`), which
    /// can't read a JSON map's string keys back as numbers: `joined` must survive.
    #[test]
    fn joined_survives_a_tagged_reply() {
        let mut item: Item = serde_json::from_value(serde_json::json!({
            "id": 1, "title": "t", "created": 0, "updated": 0, "sessions": [2, 3],
        }))
        .unwrap();
        item.join(2, 10);
        item.join(3, 20);
        item.join(2, 30);
        let reply = crate::ServerMsg::Ok {
            req: 1,
            reply: crate::Reply::Item {
                item: Box::new(item.clone()),
                note: None,
            },
        };
        let back: crate::ServerMsg =
            serde_json::from_slice(&serde_json::to_vec(&reply).unwrap()).unwrap();
        assert_eq!(back, reply);
        assert_eq!((item.joined_at(2), item.joined_at(3)), (Some(30), Some(20)));
        assert_eq!(item.joined_at(4), None);
    }
}

//! The backlog page (DESIGN §8.12): `b` on the home screen, or in tab mode over a
//! session. The same lanes as the web board, as one list, since four columns don't
//! fit a terminal; the selected item's detail beside or below it.

use crate::edit::LineEdit;
use crate::theme::Theme;
use crate::{fit, next_key};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{self, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use std::path::PathBuf;
use valkyrie_proto::{
    AgentState, Item, ItemFacts, ItemKind, ItemLaunch, ItemStatus, KeptItem, LaunchOffer,
    OrderProposal, Priority, SessionId, SessionInfo, ShippedItem,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lane {
    Proposed,
    Needs,
    Running,
    Review,
    Backlog,
    Done,
}

impl Lane {
    /// Top to bottom: what waits on the user first.
    const ORDER: [Lane; 6] = [
        Lane::Needs,
        Lane::Review,
        Lane::Proposed,
        Lane::Running,
        Lane::Backlog,
        Lane::Done,
    ];

    fn title(self) -> &'static str {
        match self {
            Lane::Proposed => "proposed",
            Lane::Needs => "needs you",
            Lane::Running => "in progress",
            Lane::Review => "review",
            Lane::Backlog => "backlog",
            Lane::Done => "done",
        }
    }

    fn color(self, t: &Theme) -> style::Color {
        match self {
            Lane::Proposed => t.accent2,
            Lane::Needs => t.needs,
            Lane::Running => t.working,
            Lane::Review => t.done,
            Lane::Backlog => t.accent,
            Lane::Done => t.muted,
        }
    }

    /// Lanes the home screen shows: the ones waiting on the user.
    pub(crate) fn waits(self) -> bool {
        matches!(self, Lane::Needs | Lane::Review | Lane::Proposed)
    }
}

/// The newest of an item's sessions still running on this daemon.
pub(crate) fn live<'a>(
    item: &Item,
    sessions: &'a [SessionInfo],
    boot: u64,
) -> Option<&'a SessionInfo> {
    if item.boot != boot {
        return None;
    }
    item.sessions
        .iter()
        .rev()
        .find_map(|id| sessions.iter().find(|s| s.id == *id && s.exited.is_none()))
}

/// An open item whose live sessions only took it: the user's launch hands it over,
/// as `valk todo launch` does, so `↩` launches and `o` opens the session. A take
/// never locks the user out.
pub(crate) fn taken_only(item: &Item, sessions: &[SessionInfo], boot: u64) -> bool {
    if item.status != ItemStatus::Open || item.boot != boot {
        return false;
    }
    let mut live = item
        .sessions
        .iter()
        .filter(|id| sessions.iter().any(|s| s.id == **id && s.exited.is_none()));
    let first = live.next();
    first.is_some()
        && first
            .into_iter()
            .chain(live)
            .all(|id| item.taken.contains(id))
}

/// Where an item stands, from its newest live session: as the web board works it out.
/// One with a pull request open waits on review whatever that session does, unless
/// it needs you: a session that took several items would show them all running.
/// An agent's own work it filed and took follows its session though not accepted.
pub(crate) fn lane(item: &Item, sessions: &[SessionInfo], boot: u64) -> Lane {
    match item.status {
        ItemStatus::Proposed if !item.agent_filed() => return Lane::Proposed,
        ItemStatus::Proposed => {}
        ItemStatus::Shipped | ItemStatus::Dropped => return Lane::Done,
        ItemStatus::Open => {}
    }
    if let Some(s) = live(item, sessions, boot) {
        return match s.status.state {
            AgentState::NeedsInput | AgentState::Blocked => Lane::Needs,
            _ if item.pr.is_some() => Lane::Review,
            // An Esc-interrupted turn finished nothing: it isn't up for review.
            AgentState::Working | AgentState::Stale | AgentState::Interrupted => Lane::Running,
            _ => Lane::Review,
        };
    }
    if item.pr.is_some() || !item.sessions.is_empty() {
        Lane::Review
    } else {
        Lane::Backlog
    }
}

/// Items in page order, each with its lane: by lane, then in the order to work
/// them (`valkyrie_proto::order`), the done ones newest first. Only the newest few
/// done ones.
pub(crate) fn rows(items: &[Item], sessions: &[SessionInfo], boot: u64) -> Vec<(Lane, Item)> {
    rows_of(items, items, sessions, boot)
}

/// `rows` for `shown`, those of `all` the page lists: what an item waits on may be
/// in a repository it doesn't show. Each row's `blocked_by` is cut to the items it
/// still waits on, which is all a row says of them.
pub(crate) fn rows_of(
    shown: &[Item],
    all: &[Item],
    sessions: &[SessionInfo],
    boot: u64,
) -> Vec<(Lane, Item)> {
    let mut out: Vec<(Lane, Item)> = shown
        .iter()
        .map(|i| (lane(i, sessions, boot), i.clone()))
        .collect();
    let rank = |l: Lane| Lane::ORDER.iter().position(|&o| o == l).unwrap_or(0);
    let order = valkyrie_proto::order(all);
    out.sort_by(|(la, a), (lb, b)| {
        rank(*la).cmp(&rank(*lb)).then_with(|| match la {
            Lane::Done => b.updated.cmp(&a.updated).then(b.id.cmp(&a.id)),
            _ => order(a, b),
        })
    });
    // A done item waits on nothing worth showing, as the web card has it.
    for (lane, item) in &mut out {
        item.blocked_by = match lane {
            Lane::Done => Vec::new(),
            _ => item.waiting_on(all),
        };
    }
    // Sorted newest first: the newest few done ones stay.
    let mut done = 0;
    out.retain(|(l, _)| {
        done += usize::from(*l == Lane::Done);
        *l != Lane::Done || done <= DONE_SHOWN
    });
    out
}

/// Done items shown at the bottom of the page.
/// The longest escape sequence held over to the next read while typing.
const HELD_MAX: usize = 32;

const DONE_SHOWN: usize = 5;

/// Which items the page shows (`r` cycles it): every repository's, one
/// repository's (its main root, as items keep it), or those with no folder yet.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) enum Repo {
    #[default]
    All,
    In(PathBuf),
    NoFolder,
}

impl Repo {
    pub(crate) fn holds(&self, item: &Item) -> bool {
        match self {
            Repo::All => true,
            Repo::In(root) => item.project.as_ref() == Some(root),
            Repo::NoFolder => item.project.is_none(),
        }
    }

    /// The one after this: every repository, then each one `items` are for by
    /// name, then no folder if any item has none, and round again.
    pub(crate) fn next(&self, items: &[Item]) -> Repo {
        let mut roots: Vec<&PathBuf> = items.iter().filter_map(|i| i.project.as_ref()).collect();
        roots.sort_by(|a, b| a.file_name().cmp(&b.file_name()).then(a.cmp(b)));
        roots.dedup();
        let mut cycle = vec![Repo::All];
        cycle.extend(roots.into_iter().map(|r| Repo::In(r.clone())));
        if items.iter().any(|i| i.project.is_none()) {
            cycle.push(Repo::NoFolder);
        }
        // One whose items are all gone moves on to every repository.
        match cycle.iter().position(|r| r == self) {
            Some(at) => cycle[(at + 1) % cycle.len()].clone(),
            None => Repo::All,
        }
    }

    /// What the page's title says it shows; nothing for every repository.
    fn label(&self) -> Option<String> {
        match self {
            Repo::All => None,
            Repo::In(root) => Some(root.file_name().map_or_else(
                || root.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )),
            Repo::NoFolder => Some("no folder".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Typing {
    /// A new item: Tab cycles its kind.
    Add { kind: ItemKind, text: LineEdit },
    /// A handoff note on item `id`.
    Note { id: u32, text: LineEdit },
    /// Item `id`'s title, its kind with Tab and its repository with Shift-Tab; the
    /// body stays as it is. `was` holds all three as the prompt opened, so only
    /// what changed here is sent.
    Edit {
        id: u32,
        kind: ItemKind,
        project: Option<PathBuf>,
        text: LineEdit,
        was: (String, ItemKind, Option<PathBuf>),
    },
    /// `L`'s window: what item `id` launches with.
    Launch(LaunchForm),
}

/// A field of the launch window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    Agent,
    Model,
    Worktree,
}

/// Claude Code's own aliases, which follow its newest models; Tab steps through
/// them. Codex's names change too often to keep a list of: it's typed.
const CLAUDE_MODELS: [&str; 4] = ["", "opus", "sonnet", "haiku"];

/// What `L` launches item `id` with: the agent and model its last launch ran, to
/// start with, and a new worktree unless an earlier launch's is reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LaunchForm {
    pub(crate) id: u32,
    pub(crate) agent: &'static str,
    /// Empty for the agent's own default.
    pub(crate) model: LineEdit,
    pub(crate) worktree: bool,
    /// No checkout of an earlier launch to go back to, so `worktree` decides.
    pub(crate) fresh: bool,
    pub(crate) field: Field,
    /// What it was last launched with, for the model when the agent changes back.
    last: Option<ItemLaunch>,
}

/// The agent item `item` was last launched with, claude if never (decision #46):
/// `↩` and `L` both start from it.
fn last_agent(item: &Item) -> &'static str {
    match item.launch.as_ref().map(|l| l.agent.as_str()) {
        Some("codex") => "codex",
        _ => "claude",
    }
}

impl LaunchForm {
    pub(crate) fn new(item: &Item) -> Self {
        let agent = last_agent(item);
        let mut form = Self {
            id: item.id,
            agent,
            model: LineEdit::default(),
            worktree: true,
            fresh: item.checkout.is_none(),
            field: Field::Model,
            last: item.launch.clone(),
        };
        form.model = LineEdit::new(form.last_model());
        form
    }

    /// The model the item's last launch of this agent ran.
    fn last_model(&self) -> String {
        self.last
            .as_ref()
            .filter(|l| l.agent == self.agent)
            .map(|l| l.model.clone())
            .unwrap_or_default()
    }

    pub(crate) fn fields(&self) -> &'static [Field] {
        if self.fresh {
            &[Field::Agent, Field::Model, Field::Worktree]
        } else {
            &[Field::Agent, Field::Model]
        }
    }

    fn step(&mut self, down: bool) {
        let fields = self.fields();
        let at = fields.iter().position(|f| *f == self.field).unwrap_or(0);
        let at = if down {
            (at + 1).min(fields.len() - 1)
        } else {
            at.saturating_sub(1)
        };
        self.field = fields[at];
    }

    /// Flips the agent, or the worktree; the model follows the agent.
    fn change(&mut self) {
        match self.field {
            Field::Agent => {
                self.agent = if self.agent == "claude" {
                    "codex"
                } else {
                    "claude"
                };
                self.model = LineEdit::new(self.last_model());
            }
            Field::Worktree => self.worktree = !self.worktree,
            Field::Model => {}
        }
    }

    /// The next of the agent's known models after the one typed.
    fn next_model(&mut self) {
        let known: &[&str] = if self.agent == "claude" {
            &CLAUDE_MODELS
        } else {
            &[""]
        };
        let now = self.model.text().trim();
        let next = match known.iter().position(|m| *m == now) {
            Some(at) => known[(at + 1) % known.len()],
            None => known[0],
        };
        self.model = LineEdit::new(next);
    }

    fn launch(&self) -> Do {
        Do::Launch {
            id: self.id,
            agent: self.agent,
            model: Some(self.model.text().trim().to_owned()),
            worktree: self.worktree,
        }
    }
}

impl Typing {
    /// A new item, a bug until Tab says otherwise (`n`, and the capture key).
    pub(crate) fn add() -> Typing {
        Typing::Add {
            kind: ItemKind::Bug,
            text: LineEdit::default(),
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Page {
    pub(crate) cursor: usize,
    pub(crate) typing: Option<Typing>,
    /// `x` was pressed once on this item: deleting has no undo, so it takes two.
    pub(crate) armed: Option<u32>,
    /// `B` was pressed once on this item with an agent running on it: sending it
    /// back stops that agent, so it takes two.
    pub(crate) returning: Option<u32>,
    /// What `S` found merged, waiting on a second `S` to ship exactly these.
    pub(crate) ship_armed: Option<Vec<u32>>,
    /// The selected item's facts, for `(id, updated)`, read when the cursor lands.
    pub(crate) facts: Option<(u32, u64, Option<ItemFacts>)>,
    /// Opened by the capture key straight into a new item: the page closes once
    /// it's added or cancelled.
    pub(crate) capture: bool,
    /// The repository whose items it shows; new ones are written down for it.
    pub(crate) repo: Repo,
    /// Inside a bracketed paste while typing; `true` right after a line break, so
    /// a run of them, and the indent after, becomes one space.
    paste: Option<bool>,
    /// An escape sequence a read cut off while typing, joined to the next read so
    /// a paste's end marker split in two is still seen.
    held: Vec<u8>,
    /// The start of a character a read cut off, while typing.
    partial: Vec<u8>,
    /// Build orders waiting on the user, as last pushed: the app keeps it in step.
    pub(crate) orders: Vec<OrderProposal>,
    /// The build order open over the page (`O`).
    pub(crate) order: Option<OrderView>,
    /// Ready items the supervisor offers to start, as last pushed.
    pub(crate) launches: Vec<LaunchOffer>,
    /// Every repository some item is for, as last pushed: what an edit can move to.
    pub(crate) roots: Vec<PathBuf>,
}

/// A build order open over the page: which, where the cursor is, and the items
/// whose changes the user unticked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrderView {
    pub(crate) id: u64,
    pub(crate) project: PathBuf,
    pub(crate) cursor: usize,
    pub(crate) skip: Vec<u32>,
}

/// `bytes` as text after what `partial` holds, keeping a character cut off at the
/// end for the next read.
fn decode(partial: &mut Vec<u8>, bytes: &[u8]) -> String {
    partial.extend_from_slice(bytes);
    let whole = match std::str::from_utf8(partial) {
        Ok(_) => partial.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => partial.len(),
    };
    let text = String::from_utf8_lossy(&partial[..whole]).into_owned();
    partial.drain(..whole);
    text
}

/// What a key on the page asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Do {
    Close,
    /// `model` as `ClientMsg::LaunchItem` has it: `None` is as last time.
    Launch {
        id: u32,
        agent: &'static str,
        model: Option<String>,
        worktree: bool,
    },
    Attach(SessionId),
    Status(u32, ItemStatus),
    /// Back to the backlog: stops its sessions, `ItemChange::Return`.
    Return(u32),
    Delete(u32),
    Add {
        kind: ItemKind,
        text: String,
    },
    Note {
        id: u32,
        text: String,
    },
    /// Only what was changed: `None` leaves it as it is.
    Edit {
        id: u32,
        kind: Option<ItemKind>,
        title: Option<String>,
        priority: Option<Priority>,
        /// Moves it to this repository.
        cwd: Option<PathBuf>,
    },
    /// Fetch, then ask what's merged, to show before shipping it.
    CheckMerged,
    /// Ship these, the ones shown, if they're still merged.
    ShipMerged(Vec<u32>),
    /// Show the next repository's items (`Repo::next`).
    NextRepo,
    /// Ask for a build order of this repository (`P`).
    Plan(PathBuf),
    /// Start (`R`) or leave (`X`) the ready item the supervisor offers.
    AnswerLaunch {
        project: PathBuf,
        id: u64,
        item: u32,
        launch: bool,
    },
    /// Accept (only `items`' changes, or all with `None`) or decline a build order.
    AnswerOrder {
        project: PathBuf,
        id: u64,
        accept: bool,
        items: Option<Vec<u32>>,
    },
    Say(String),
}

impl Page {
    /// Handles `keys` against `rows` (from `rows`), returning what to do, in order.
    pub(crate) fn keys(
        &mut self,
        keys: &[u8],
        rows: &[(Lane, Item)],
        sessions: &[SessionInfo],
        boot: u64,
    ) -> Vec<Do> {
        let mut out = Vec::new();
        let mut typed = Vec::new();
        let joined = [std::mem::take(&mut self.held).as_slice(), keys].concat();
        let mut keys: &[u8] = &joined;
        while !keys.is_empty() {
            let key = next_key(keys);
            let whole = key.len() == keys.len();
            keys = &keys[key.len()..];
            // Like `x x`: any other key, typing included, and it's not shipped.
            if key != b"S" || self.typing.is_some() {
                self.ship_armed = None;
            }
            if self.order.is_some() {
                out.extend(self.order_key(key, whole));
                continue;
            }
            if let Some(typing) = &mut self.typing {
                // A CSI sequence the read cut off waits for the rest, but only so
                // long: past `HELD_MAX` it's not a sequence, and Esc must still work.
                let cut = key.len() <= HELD_MAX
                    && (key.len() == 2 || !(0x40..=0x7e).contains(&key[key.len() - 1]));
                if whole && key.starts_with(b"\x1b[") && cut {
                    self.held = key.to_vec();
                    break;
                }
                // Esc or ^C on its own ends a paste whose end marker never came, and
                // then does what it always does: the prompt can't trap you.
                if (whole && key == b"\x1b") || key == [0x03] {
                    self.paste = None;
                }
                // The launch window's other fields take keys of their own; a paste
                // there goes into the model, the one field that takes text.
                if let Typing::Launch(form) = typing
                    && form.field != Field::Model
                    && self.paste.is_none()
                    && key != b"\x1b[200~"
                {
                    match key {
                        b"\r" | b"\n" => {
                            out.push(form.launch());
                            self.typing = None;
                        }
                        b"\x1b" if whole => self.typing = None,
                        [0x03] => self.typing = None,
                        b" " | b"h" | b"l" | b"\x1b[C" | b"\x1bOC" | b"\x1b[D" | b"\x1bOD" => {
                            form.change()
                        }
                        b"k" | b"\x1b[A" | b"\x1bOA" => form.step(false),
                        b"j" | b"\x1b[B" | b"\x1bOB" | b"\t" => form.step(true),
                        _ => {}
                    }
                    if self.typing.is_none() {
                        self.held.clear();
                    }
                    continue;
                }
                let launching = matches!(typing, Typing::Launch(_));
                let text = match typing {
                    Typing::Add { text, .. }
                    | Typing::Note { text, .. }
                    | Typing::Edit { text, .. } => text,
                    Typing::Launch(form) => &mut form.model,
                };
                // A paste is text, its line breaks spaces: a pasted error doesn't end
                // the prompt and run its other lines as keys.
                if let Some(after_break) = &mut self.paste {
                    match key {
                        b"\x1b[201~" => self.paste = None,
                        b"\r" | b"\n" | b"\t" => {
                            if !*after_break {
                                typed.push(b' ');
                            }
                            *after_break = true;
                        }
                        // The next line's indent goes into that one space too.
                        b" " if *after_break => {}
                        [b] if *b >= 0x20 && *b != 0x7f => {
                            typed.push(*b);
                            *after_break = false;
                        }
                        _ => {}
                    }
                    continue;
                }
                if key == b"\x1b[200~" {
                    self.paste = Some(false);
                    continue;
                }
                if key.len() == 1 && key[0] >= 0x20 && key[0] != 0x7f {
                    typed.push(key[0]);
                    continue;
                }
                text.insert(&decode(&mut self.partial, &std::mem::take(&mut typed)));
                match key {
                    b"\r" | b"\n" => {
                        let typing = self.typing.take().unwrap();
                        match typing {
                            Typing::Add { kind, text } if !text.text().trim().is_empty() => {
                                out.push(Do::Add {
                                    kind,
                                    text: text.text().trim().into(),
                                });
                            }
                            Typing::Note { id, text } if !text.text().trim().is_empty() => {
                                out.push(Do::Note {
                                    id,
                                    text: text.text().trim().into(),
                                });
                            }
                            Typing::Edit {
                                id,
                                kind,
                                project,
                                text,
                                was,
                            } if !text.text().trim().is_empty() => {
                                let title = text.text().trim();
                                let title = (title != was.0).then(|| title.to_owned());
                                let kind = (kind != was.1).then_some(kind);
                                let cwd = project.filter(|p| Some(p) != was.2.as_ref());
                                if title.is_some() || kind.is_some() || cwd.is_some() {
                                    out.push(Do::Edit {
                                        id,
                                        kind,
                                        title,
                                        priority: None,
                                        cwd,
                                    });
                                }
                            }
                            Typing::Launch(form) => out.push(form.launch()),
                            _ => {}
                        }
                    }
                    b"\x1b[Z" => {
                        if let Typing::Edit { project, .. } = typing {
                            let at = project
                                .as_ref()
                                .and_then(|p| self.roots.iter().position(|r| r == p));
                            let next = at.map_or(0, |i| i + 1);
                            if !self.roots.is_empty() {
                                *project = Some(self.roots[next % self.roots.len()].clone());
                            }
                        }
                    }
                    b"\t" => {
                        if let Typing::Add { kind, .. } | Typing::Edit { kind, .. } = typing {
                            let at = ItemKind::ALL.iter().position(|k| k == kind).unwrap_or(0);
                            *kind = ItemKind::ALL[(at + 1) % ItemKind::ALL.len()];
                        }
                        if let Typing::Launch(form) = typing {
                            form.next_model();
                        }
                    }
                    b"\x1b[A" | b"\x1bOA" | b"\x1b[B" | b"\x1bOB" if launching => {
                        if let Typing::Launch(form) = typing {
                            form.step(key[key.len() - 1] == b'B');
                        }
                    }
                    b"\x1b" if whole => self.typing = None,
                    [0x03] => self.typing = None,
                    k => {
                        text.key(k);
                    }
                }
                if self.typing.is_none() {
                    self.partial.clear();
                    self.paste = None;
                    self.held.clear();
                    // A capture is over: the rest of the read is no command.
                    if self.capture {
                        break;
                    }
                }
                continue;
            }
            let here = rows.get(self.cursor).map(|(l, i)| (*l, i));
            if key != b"x" {
                self.armed = None;
            }
            if key != b"B" {
                self.returning = None;
            }
            match key {
                b"k" | b"\x1b[A" | b"\x1bOA" => self.cursor = self.cursor.saturating_sub(1),
                b"j" | b"\x1b[B" | b"\x1bOB" => {
                    self.cursor = (self.cursor + 1).min(rows.len().saturating_sub(1))
                }
                b"g" => self.cursor = 0,
                b"G" => self.cursor = rows.len().saturating_sub(1),
                b"n" => self.typing = Some(Typing::add()),
                b"r" => out.push(Do::NextRepo),
                b"P" => out.push(match self.scope(rows) {
                    Some(project) => Do::Plan(project),
                    None => Do::Say("pick a repository first: r".into()),
                }),
                b"O" => {
                    let scope = self.scope(rows);
                    let waiting = self
                        .orders
                        .iter()
                        .find(|o| scope.as_ref().is_none_or(|p| *p == o.project));
                    match waiting {
                        Some(o) => {
                            self.order = Some(OrderView {
                                id: o.id,
                                project: o.project.clone(),
                                cursor: 0,
                                skip: Vec::new(),
                            })
                        }
                        None => out.push(Do::Say("no build order waits: P asks for one".into())),
                    }
                }
                b"R" | b"X" => out.push(match self.waiting_launch(rows) {
                    Some(o) => Do::AnswerLaunch {
                        project: o.project.clone(),
                        id: o.id,
                        item: o.item,
                        launch: key == b"R",
                    },
                    None => Do::Say("the supervisor offers nothing to start".into()),
                }),
                b"S" => out.push(match self.ship_armed.take() {
                    Some(ids) => Do::ShipMerged(ids),
                    None => Do::CheckMerged,
                }),
                b"q" | b"b" | [0x03] => out.push(Do::Close),
                b"\x1b" if whole => out.push(Do::Close),
                _ => {
                    let Some((lane, item)) = here else {
                        continue;
                    };
                    out.extend(self.on_item(key, lane, item, sessions, boot));
                }
            }
        }
        match &mut self.typing {
            Some(
                Typing::Add { text, .. } | Typing::Note { text, .. } | Typing::Edit { text, .. },
            ) => text.insert(&decode(&mut self.partial, &typed)),
            Some(Typing::Launch(form)) => form.model.insert(&decode(&mut self.partial, &typed)),
            None => {}
        }
        out
    }

    /// The repository `P` and `O` are about: the one the page shows, else the
    /// selected item's.
    fn scope(&self, rows: &[(Lane, Item)]) -> Option<PathBuf> {
        match &self.repo {
            Repo::In(root) => Some(root.clone()),
            Repo::NoFolder => None,
            Repo::All => rows.get(self.cursor).and_then(|(_, i)| i.project.clone()),
        }
    }

    /// The open build order, if it still waits.
    pub(crate) fn open_order(&self) -> Option<(&OrderView, &OrderProposal)> {
        let view = self.order.as_ref()?;
        let o = self
            .orders
            .iter()
            .find(|o| o.id == view.id && o.project == view.project)?;
        Some((view, o))
    }

    /// A key while a build order is open over the page.
    fn order_key(&mut self, key: &[u8], whole: bool) -> Vec<Do> {
        let Some(steps) = self.open_order().map(|(_, o)| o.steps.clone()) else {
            // Answered or replaced meanwhile.
            self.order = None;
            return vec![Do::Say("that build order is gone".into())];
        };
        let view = self.order.as_mut().unwrap();
        match key {
            b"k" | b"\x1b[A" | b"\x1bOA" => view.cursor = view.cursor.saturating_sub(1),
            b"j" | b"\x1b[B" | b"\x1bOB" => {
                view.cursor = (view.cursor + 1).min(steps.len().saturating_sub(1))
            }
            b" " => {
                if let Some(step) = steps.get(view.cursor).filter(|s| s.change().is_some()) {
                    match view.skip.iter().position(|&i| i == step.item) {
                        Some(at) => {
                            view.skip.remove(at);
                        }
                        None => view.skip.push(step.item),
                    }
                }
            }
            b"\r" | b"\n" | b"a" => {
                let view = self.order.take().unwrap();
                let items = (!view.skip.is_empty()).then(|| {
                    steps
                        .iter()
                        .filter(|s| s.change().is_some() && !view.skip.contains(&s.item))
                        .map(|s| s.item)
                        .collect::<Vec<_>>()
                });
                if items.as_ref().is_some_and(Vec::is_empty) {
                    return vec![Do::Say("every change unticked: d declines it".into())];
                }
                return vec![Do::AnswerOrder {
                    project: view.project,
                    id: view.id,
                    accept: true,
                    items,
                }];
            }
            b"d" => {
                let view = self.order.take().unwrap();
                return vec![Do::AnswerOrder {
                    project: view.project,
                    id: view.id,
                    accept: false,
                    items: None,
                }];
            }
            b"q" | [0x03] => self.order = None,
            b"\x1b" if whole => self.order = None,
            _ => {}
        }
        Vec::new()
    }

    fn on_item(
        &mut self,
        key: &[u8],
        lane: Lane,
        item: &Item,
        sessions: &[SessionInfo],
        boot: u64,
    ) -> Vec<Do> {
        let id = item.id;
        let live = live(item, sessions, boot).map(|s| s.id);
        let hand_over = taken_only(item, sessions, boot);
        let launchable = item.status == ItemStatus::Open && (live.is_none() || hand_over);
        let act = match key {
            b"\r" | b"\n" => match live {
                Some(s) if !hand_over => Do::Attach(s),
                _ if item.status == ItemStatus::Proposed => Do::Say("accept it first: a".into()),
                _ if !launchable => {
                    Do::Say(format!("#{id} is {}: o reopens it", item.status.as_str()))
                }
                _ if item.project.is_none() => {
                    Do::Say(format!("#{id} has no folder yet: pick one in the web app"))
                }
                _ => Do::Launch {
                    id,
                    agent: last_agent(item),
                    model: None,
                    worktree: true,
                },
            },
            b"c" if launchable && item.project.is_some() => Do::Launch {
                id,
                agent: "codex",
                model: None,
                worktree: true,
            },
            b"L" if launchable && item.project.is_some() => {
                self.typing = Some(Typing::Launch(LaunchForm::new(item)));
                return Vec::new();
            }
            b"L" if item.status == ItemStatus::Proposed => Do::Say("accept it first: a".into()),
            b"L" if !launchable && live.is_some() => {
                Do::Say(format!("an agent is on #{id}: ↩ opens it"))
            }
            b"L" if !launchable => {
                Do::Say(format!("#{id} is {}: o reopens it", item.status.as_str()))
            }
            b"L" => Do::Say(format!("#{id} has no folder yet: pick one in the web app")),
            b"a" if item.status == ItemStatus::Proposed => Do::Status(id, ItemStatus::Open),
            b"s" if item.status == ItemStatus::Open || item.agent_filed() => {
                Do::Status(id, ItemStatus::Shipped)
            }
            b"d" if item.status != ItemStatus::Dropped => Do::Status(id, ItemStatus::Dropped),
            b"B" if item.sessions.is_empty() => Do::Say(format!("#{id} isn't started")),
            b"B" if item.pr.is_some() => {
                Do::Say(format!("#{id} has a pull request: ship or drop it"))
            }
            b"B" if live.is_some() && self.returning != Some(id) => {
                self.returning = Some(id);
                Do::Say(format!("B again stops its agent and returns #{id}"))
            }
            b"B" if matches!(lane, Lane::Done | Lane::Proposed) => {
                Do::Say(format!("#{id} is {}", lane.title()))
            }
            b"B" => {
                self.returning = None;
                Do::Return(id)
            }
            // Up toward p0, down toward p3; nothing past either end.
            b"+" | b"=" | b"-" if lane != Lane::Done => {
                let at = Priority::ALL.iter().position(|&p| p == item.priority);
                let to = match (key, at) {
                    (b"-", Some(i)) => Priority::ALL.get(i + 1),
                    (_, Some(i)) => i.checked_sub(1).and_then(|i| Priority::ALL.get(i)),
                    (_, None) => None,
                };
                match to {
                    Some(&p) => Do::Edit {
                        id,
                        kind: None,
                        title: None,
                        priority: Some(p),
                        cwd: None,
                    },
                    None => Do::Say(format!("#{id} is already {}", item.priority.as_str())),
                }
            }
            b"o" if lane == Lane::Done => Do::Status(id, ItemStatus::Open),
            b"o" if live.is_some() => Do::Attach(live.unwrap_or_default()),
            b"N" => {
                self.typing = Some(Typing::Note {
                    id,
                    text: LineEdit::default(),
                });
                return Vec::new();
            }
            b"e" => {
                self.typing = Some(Typing::Edit {
                    id,
                    kind: item.kind,
                    project: item.project.clone(),
                    text: LineEdit::new(item.title.clone()),
                    was: (item.title.clone(), item.kind, item.project.clone()),
                });
                return Vec::new();
            }
            b"x" if live.is_some() => Do::Say(format!("an agent is still running on #{id}")),
            b"x" if self.armed == Some(id) => {
                self.armed = None;
                Do::Delete(id)
            }
            b"x" => {
                self.armed = Some(id);
                Do::Say(format!("x again deletes #{id} for good"))
            }
            _ => return Vec::new(),
        };
        vec![act]
    }

    pub(crate) fn draw(
        &self,
        frame: &mut ratatui::Frame,
        rows: &[(Lane, Item)],
        sessions: &[SessionInfo],
        boot: u64,
        now: u64,
        t: &Theme,
    ) {
        let area = frame.area();
        // Nearly the whole screen, a margin around it so it reads as over what's under.
        let rect = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        )
        .intersection(area);
        frame.render_widget(Clear, rect);
        let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
        let here = rows.get(self.cursor);
        let hints: Vec<_> = match (&self.typing, here) {
            (Some(Typing::Add { .. }), _) => [
                hint(" ↩ ", "add "),
                hint(" tab ", "kind "),
                hint(" esc ", "cancel "),
            ]
            .concat(),
            (Some(Typing::Edit { .. }), _) => [
                hint(" ↩ ", "save "),
                hint(" tab ", "kind "),
                hint(" ⇧tab ", "repo "),
                hint(" esc ", "cancel "),
            ]
            .concat(),
            (Some(Typing::Note { .. }), _) => {
                [hint(" ↩ ", "note "), hint(" esc ", "cancel ")].concat()
            }
            (Some(Typing::Launch(form)), _) => [
                hint(" ↩ ", "launch "),
                hint(" ↑↓ ", "field "),
                if form.field == Field::Model {
                    hint(" tab ", "next model ")
                } else {
                    hint(" ␣ ", "change ")
                },
                hint(" esc ", "cancel "),
            ]
            .concat(),
            (None, Some((Lane::Proposed, _))) => [
                hint(" a ", "accept "),
                hint(" e ", "edit "),
                hint(" +/- ", "priority "),
                hint(" x ", "delete "),
                hint(" r ", "repo "),
                hint(" n ", "new "),
                hint(" esc ", "close "),
            ]
            .concat(),
            (None, Some((Lane::Done, _))) => [
                hint(" o ", "reopen "),
                hint(" e ", "edit "),
                hint(" x ", "delete "),
                hint(" S ", "ship merged "),
                hint(" r ", "repo "),
                hint(" n ", "new "),
                hint(" esc ", "close "),
            ]
            .concat(),
            (None, Some((lane, item))) => {
                let with = hint(" L ", "with… ");
                let open = if taken_only(item, sessions, boot) {
                    [
                        hint(" ↩ ", "launch "),
                        with,
                        hint(" o ", "open its session "),
                    ]
                    .concat()
                } else if live(item, sessions, boot).is_some() {
                    hint(" ↩ ", "open ").to_vec()
                } else if item.agent_filed() {
                    hint(" a ", "accept ").to_vec()
                } else if *lane == Lane::Review {
                    [hint(" ↩ ", "relaunch "), with].concat()
                } else {
                    [hint(" ↩ ", "launch "), with].concat()
                };
                let rest = [
                    hint(" s ", "ship "),
                    hint(" S ", "merged "),
                    hint(" e ", "edit "),
                    hint(" +/- ", "priority "),
                    hint(" N ", "note "),
                    hint(" d ", "drop "),
                    hint(" B ", "to backlog "),
                    hint(" r ", "repo "),
                    hint(" n ", "new "),
                    hint(" P ", "order "),
                    hint(" esc ", "close "),
                ];
                [open, rest.concat()].concat()
            }
            (None, None) => [
                hint(" r ", "repo "),
                hint(" n ", "new "),
                hint(" esc ", "close "),
            ]
            .concat(),
        };
        let hints = fit_hints(hints, rect.width.saturating_sub(4) as usize);
        let open = rows.iter().filter(|(l, _)| *l != Lane::Done).count();
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style::Style::new().fg(t.accent))
            .title(Line::from(
                [
                    vec![" Backlog ".fg(t.fg).bold()],
                    self.repo
                        .label()
                        .map(|r| vec![format!("{r} ").fg(t.accent).bold()])
                        .unwrap_or_default(),
                    vec![format!("{open} open ").fg(t.muted)],
                    if self.waiting_order(rows) {
                        vec![" build order waits: O ".fg(t.accent).bold()]
                    } else {
                        vec![]
                    },
                    match self.waiting_launch(rows) {
                        Some(o) => vec![
                            format!(" #{} ready: R starts it, X leaves it ", o.item)
                                .fg(t.accent)
                                .bold(),
                        ],
                        None => vec![],
                    },
                ]
                .concat(),
            ))
            .title_bottom(Line::from(hints).centered())
            .style(style::Style::new().bg(t.panel).fg(t.fg));
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        let wide = inner.width >= 100;
        let (list, detail) = if wide {
            let [l, d] =
                Layout::horizontal([Constraint::Percentage(55), Constraint::Fill(1)]).areas(inner);
            (l, d)
        } else {
            let [l, d] =
                Layout::vertical([Constraint::Fill(1), Constraint::Length(inner.height / 3)])
                    .areas(inner);
            (l, d)
        };

        // The list, with a heading over each lane; rows slide to keep the cursor in view.
        let mut lines: Vec<Line> = Vec::new();
        let mut cursor_line = 0;
        let mut last = None;
        for (i, (lane, item)) in rows.iter().enumerate() {
            if last != Some(*lane) {
                if last.is_some() {
                    lines.push(Line::default());
                }
                let n = rows.iter().filter(|(l, _)| l == lane).count();
                lines.push(Line::from(vec![
                    format!(" {} ", lane.title()).fg(lane.color(t)).bold(),
                    format!("{n}").fg(t.muted),
                ]));
                last = Some(*lane);
            }
            if i == self.cursor {
                cursor_line = lines.len();
            }
            lines.push(item_line(item, *lane, i == self.cursor, list.width, t));
        }
        if rows.is_empty() {
            lines.push(Line::default());
            lines.push(Line::from(
                "  nothing written down: n adds a bug or an idea".fg(t.muted),
            ));
        }
        if let Some(typing) = self
            .typing
            .as_ref()
            .filter(|t| !matches!(t, Typing::Launch(_)))
        {
            lines.push(Line::default());
            lines.push(typing_line(typing, list.width as usize, t));
            cursor_line = lines.len() - 1;
        }
        let room = list.height.max(1) as usize;
        let top = cursor_line.saturating_sub(room.saturating_sub(2));
        frame.render_widget(Paragraph::new(lines).scroll((top as u16, 0)), list);

        if let Some((lane, item)) = here {
            let facts = self
                .facts
                .as_ref()
                .filter(|(id, updated, _)| *id == item.id && *updated == item.updated)
                .and_then(|(_, _, f)| f.as_ref());
            let text = detail_lines(item, *lane, live(item, sessions, boot), facts, now, t);
            let pane = Block::new()
                .borders(if wide {
                    ratatui::widgets::Borders::LEFT
                } else {
                    ratatui::widgets::Borders::TOP
                })
                .border_style(style::Style::new().fg(t.border));
            frame.render_widget(
                Paragraph::new(text).block(pane).wrap(Wrap { trim: false }),
                detail,
            );
        }
        if let Some(Typing::Launch(form)) = &self.typing {
            launch_window(frame, rect, form, t);
        }
        if let Some((view, order)) = self.open_order() {
            order_window(frame, rect, view, order, rows, t);
        }
    }

    /// The launch offer for what the page shows: the repository's, or with every
    /// repository shown, the selected item's repository's, else the first.
    fn waiting_launch(&self, rows: &[(Lane, Item)]) -> Option<&LaunchOffer> {
        let scope = self.scope(rows);
        let here = self
            .launches
            .iter()
            .find(|o| Some(&o.project) == scope.as_ref());
        match &self.repo {
            Repo::All => here.or(self.launches.first()),
            _ => here,
        }
    }

    /// Whether a build order waits for what the page shows.
    fn waiting_order(&self, rows: &[(Lane, Item)]) -> bool {
        match &self.repo {
            Repo::All => !self.orders.is_empty(),
            _ => {
                let scope = self.scope(rows);
                self.orders
                    .iter()
                    .any(|o| Some(&o.project) == scope.as_ref())
            }
        }
    }
}

/// A build order over the page: each step with its tick, what accepting it
/// changes, and why.
fn order_window(
    frame: &mut ratatui::Frame,
    over: Rect,
    view: &OrderView,
    order: &OrderProposal,
    rows: &[(Lane, Item)],
    t: &Theme,
) {
    let width = over.width.saturating_sub(6).min(100);
    let height = over.height.saturating_sub(4);
    let rect =
        Rect::new(over.x + (over.width - width) / 2, over.y + 2, width, height).intersection(over);
    frame.render_widget(Clear, rect);
    let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
    let hints = [
        hint(" ␣ ", "tick "),
        hint(" ↩ ", "accept ticked "),
        hint(" d ", "decline "),
        hint(" esc ", "later "),
    ]
    .concat();
    let name = order.project.file_name().map_or_else(
        || order.project.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style::Style::new().fg(t.accent))
        .title(Line::from(vec![
            format!(" Build order for {name} ").fg(t.fg).bold(),
            format!("({}) ", order.trigger).fg(t.muted),
        ]))
        .title_bottom(Line::from(fit_hints(hints, width.saturating_sub(4) as usize)).centered())
        .style(style::Style::new().bg(t.panel).fg(t.fg));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let mut lines = Vec::new();
    let mut cursor_line = 0;
    for (n, step) in order.steps.iter().enumerate() {
        let item = rows.iter().map(|(_, i)| i).find(|i| i.id == step.item);
        let changes = step_changes(step, item);
        let tick = match (step.change().is_some(), view.skip.contains(&step.item)) {
            (false, _) => "   ",
            (true, true) => "[ ]",
            (true, false) => "[x]",
        };
        if n == view.cursor {
            cursor_line = lines.len();
        }
        let title = item.map_or("(not shown)", |i| i.title.as_str());
        let head = format!(" {tick} {:>2}. #{} ", n + 1, step.item);
        let room = (inner.width as usize)
            .saturating_sub(head.chars().count() + changes.chars().count() + 2);
        let mut line = Line::from(vec![
            head.fg(if n == view.cursor { t.accent } else { t.fg })
                .bold(),
            fit(title, room).fg(t.fg),
            format!("  {changes}").fg(t.accent),
        ]);
        if n == view.cursor {
            line = line.style(style::Style::new().bg(t.selection));
        }
        lines.push(line);
        if !step.reason.is_empty() {
            lines.push(Line::from(format!("          {}", step.reason).fg(t.muted)));
        }
    }
    let room = inner.height.max(1) as usize;
    let top = cursor_line.saturating_sub(room.saturating_sub(3));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((top as u16, 0)),
        inner,
    );
}

/// What accepting `step` changes on `item`: `p2→p1, after #37`; empty for none.
pub(crate) fn step_changes(step: &valkyrie_proto::OrderStep, item: Option<&Item>) -> String {
    let mut parts = Vec::new();
    if let Some(p) = step.priority {
        parts.push(match item {
            Some(i) => format!("{}→{}", i.priority.as_str(), p.as_str()),
            None => p.as_str().to_owned(),
        });
    }
    if let Some(after) = &step.blocked_by {
        let had = item.map(|i| i.blocked_by.as_slice()).unwrap_or_default();
        let added: Vec<String> = after
            .iter()
            .filter(|b| !had.contains(b))
            .map(|b| format!("#{b}"))
            .collect();
        if !added.is_empty() {
            parts.push(format!("after {}", added.join(", ")));
        }
    }
    parts.join(", ")
}

/// The hints, key and word in pairs, that fit in `room` columns: the first ones,
/// which matter most, since a centered title too wide loses its start.
fn fit_hints(hints: Vec<Span<'static>>, room: usize) -> Vec<Span<'static>> {
    let mut used = 0;
    hints
        .chunks(2)
        .take_while(|pair| {
            used += pair.iter().map(Span::width).sum::<usize>();
            used <= room
        })
        .flatten()
        .cloned()
        .collect()
}

/// `L`'s window, over the middle of the page: a row per field, the one being
/// changed marked, and a line on what it does.
fn launch_window(frame: &mut ratatui::Frame, over: Rect, form: &LaunchForm, t: &Theme) {
    let fields = form.fields();
    let width = over.width.saturating_sub(4).min(56);
    let height = (fields.len() as u16 + 4).min(over.height);
    let rect = Rect::new(
        over.x + (over.width - width) / 2,
        over.y + over.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style::Style::new().fg(t.accent))
        .title(format!(" Launch #{} ", form.id).fg(t.fg).bold())
        .style(style::Style::new().bg(t.panel).fg(t.fg));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let room = inner.width.saturating_sub(12) as usize;
    let mut lines: Vec<Line> = fields
        .iter()
        .map(|field| {
            let here = *field == form.field;
            let name = match field {
                Field::Agent => "agent",
                Field::Model => "model",
                Field::Worktree => "worktree",
            };
            let mark = if here { "›" } else { " " };
            let label = format!(" {mark} {name:<9}");
            let mut spans = vec![if here {
                label.fg(t.accent).bold()
            } else {
                label.fg(t.muted)
            }];
            match field {
                Field::Agent => spans.extend(["claude", "codex"].map(|a| {
                    if a == form.agent {
                        format!("● {a}  ").fg(t.fg).bold()
                    } else {
                        format!("○ {a}  ").fg(t.muted)
                    }
                })),
                Field::Model if here => spans.extend(form.model.spans(room, t)),
                Field::Model if form.model.text().trim().is_empty() => {
                    spans.push("default".fg(t.muted))
                }
                Field::Model => spans.push(form.model.text().to_owned().fg(t.fg)),
                Field::Worktree => spans.push(
                    if form.worktree {
                        "its own"
                    } else {
                        "the repository itself"
                    }
                    .fg(t.fg),
                ),
            }
            Line::from(spans)
        })
        .collect();
    lines.push(Line::default());
    let note = match form.field {
        Field::Model if form.agent == "claude" => "empty is the default; tab: opus, sonnet, haiku",
        Field::Model => "as codex names it; empty is its default",
        _ if !form.fresh => "goes back to its earlier checkout",
        Field::Agent => "the model follows the agent",
        Field::Worktree => "a new worktree on its branch, or none",
    };
    lines.push(Line::from(format!("   {note}").fg(t.muted)));
    frame.render_widget(Paragraph::new(lines), inner);
}

/// What `S`'s answer was, for how to word it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Said {
    /// Really shipped.
    Shipped,
    /// Found merged, the page waiting on a second `S`.
    Confirm,
    /// Found merged, with the page closed meanwhile: nothing waits.
    Found,
}

/// What to do with `S`'s answer: the ids to arm the page with, if any, and the
/// line to say. Worded from what was asked (`asking`, a dry run), never from
/// whether the page is still open; only an open page is armed.
pub(crate) fn answer(
    asking: bool,
    page_open: bool,
    shipped: &[ShippedItem],
    kept: &[KeptItem],
    unfetched: usize,
) -> (Option<Vec<u32>>, String) {
    let said = match (asking, page_open) {
        (false, _) => Said::Shipped,
        (true, true) => Said::Confirm,
        (true, false) => Said::Found,
    };
    let arm = (said == Said::Confirm && !shipped.is_empty())
        .then(|| shipped.iter().map(|s| s.id).collect());
    (arm, shipped_summary(shipped, kept, unfetched, said))
}

/// `S`'s one line: `ship #1 fix/a, #4 fix/b (take), #6 fix/c (take, agent-filed)?
/// S again; 2 not merged; #9 branch fix/x not found`, then `shipped #1 #4`. A
/// branch a take set is marked, since an agent chose it, and an item an agent
/// filed that the user hasn't accepted, since shipping it accepts it. `unfetched` repositories were judged as last fetched.
pub(crate) fn shipped_summary(
    shipped: &[ShippedItem],
    kept: &[KeptItem],
    unfetched: usize,
    said: Said,
) -> String {
    let ids: Vec<String> = shipped.iter().map(|s| format!("#{}", s.id)).collect();
    let branches: Vec<String> = shipped
        .iter()
        .map(|s| {
            let marks: Vec<&str> = [(s.branch_taken, "take"), (s.filed, "agent-filed")]
                .into_iter()
                .filter_map(|(on, mark)| on.then_some(mark))
                .collect();
            match marks.is_empty() {
                true => format!("#{} {}", s.id, s.branch),
                false => format!("#{} {} ({})", s.id, s.branch, marks.join(", ")),
            }
        })
        .collect();
    let mut parts = vec![match (shipped, said) {
        ([], Said::Shipped) => "nothing shipped".to_owned(),
        ([], _) => "nothing merged to ship".to_owned(),
        (_, Said::Confirm) => format!("ship {}? S again", branches.join(", ")),
        (_, Said::Found) => format!("merged: {} (b, then S S to ship)", branches.join(", ")),
        (_, Said::Shipped) => format!("shipped {}", ids.join(" ")),
    }];
    let not_merged = kept.iter().filter(|k| k.why == "not merged").count();
    if not_merged > 0 {
        parts.push(format!("{not_merged} not merged"));
    }
    for k in kept.iter().filter(|k| k.why != "not merged") {
        parts.push(format!("#{} {}", k.id, k.why));
    }
    if unfetched > 0 {
        parts.push(format!("{unfetched} not fetched, judged as last fetched"));
    }
    parts.join("; ")
}

fn kind_color(kind: ItemKind, t: &Theme) -> style::Color {
    match kind {
        ItemKind::Bug => t.blocked,
        ItemKind::Feature => t.accent,
        ItemKind::Chore => t.muted,
    }
}

/// P0 and P1 stand out; the rest stay quiet.
fn priority_color(p: Priority, t: &Theme) -> style::Color {
    match p {
        Priority::P0 => t.needs,
        Priority::P1 => t.accent,
        Priority::P2 | Priority::P3 => t.muted,
    }
}

fn item_line(item: &Item, lane: Lane, selected: bool, width: u16, t: &Theme) -> Line<'static> {
    let folder = item
        .project
        .as_ref()
        .and_then(|p| p.file_name())
        .map_or("no folder".into(), |n| n.to_string_lossy().into_owned());
    let head = format!(" #{:<4}", item.id);
    let prio = format!("{:<3}", item.priority.as_str());
    let kind = format!("{:<8}", item.kind.as_str());
    // The lane as a column too, so a row still says its state when the heading scrolls away.
    let state = format!("{:<12}", lane.title());
    let folder = format!("  {}", fit(&folder, 16));
    // What it still waits on (`rows_of` cut `blocked_by` to that), before the folder.
    let after = match item.blocked_by.as_slice() {
        [] => String::new(),
        [one] => format!(" after #{one}"),
        [one, rest @ ..] => format!(" after #{one} +{}", rest.len()),
    };
    let room = (width as usize).saturating_sub(
        head.len()
            + prio.len()
            + kind.len()
            + state.len()
            + after.len()
            + folder.chars().count()
            + 1,
    );
    let title_fg = if lane == Lane::Done { t.muted } else { t.fg };
    let mut line = Line::from(vec![
        head.fg(t.muted),
        prio.fg(priority_color(item.priority, t)),
        kind.fg(kind_color(item.kind, t)),
        state.fg(lane.color(t)),
        // Padded, so the folders line up at the right.
        format!("{:<room$}", fit(&item.title, room)).fg(title_fg),
        after.fg(t.accent2),
        folder.fg(t.muted),
    ]);
    if selected {
        let pad = (width as usize).saturating_sub(line.width());
        line.spans.push(" ".repeat(pad).into());
        line = line.style(style::Style::new().bg(t.selection));
    }
    line
}

/// An item on the home screen: its lane, number and title.
pub(crate) fn home_line(item: &Item, lane: Lane, width: usize, t: &Theme) -> Line<'static> {
    let label = format!(" {:<10}", lane.title());
    let id = format!("#{:<4}", item.id);
    let room = width.saturating_sub(label.len() + id.len());
    Line::from(vec![
        label.fg(lane.color(t)).bold(),
        id.fg(t.muted),
        fit(&item.title, room).fg(t.fg),
    ])
}

fn typing_line(typing: &Typing, width: usize, t: &Theme) -> Line<'static> {
    // A new item's label takes its kind's color, as the list shows it.
    let (label, color, text) = match typing {
        Typing::Add { kind, text } => (
            format!(" new {} ", kind.as_str()),
            kind_color(*kind, t),
            text,
        ),
        Typing::Note { id, text } => (format!(" note on #{id} "), t.accent, text),
        Typing::Edit {
            id,
            kind,
            project,
            text,
            was,
        } => (
            match project.as_ref().filter(|p| Some(*p) != was.2.as_ref()) {
                Some(p) => format!(
                    " #{id} {} → {} ",
                    kind.as_str(),
                    p.file_name().map_or_else(
                        || p.display().to_string(),
                        |n| n.to_string_lossy().into_owned()
                    )
                ),
                None => format!(" #{id} {} ", kind.as_str()),
            },
            kind_color(*kind, t),
            text,
        ),
        Typing::Launch(form) => (format!(" #{} model ", form.id), t.accent, &form.model),
    };
    let room = width.saturating_sub(label.chars().count() + 1);
    let mut spans = vec![label.fg(t.bg).bg(color).bold(), " ".into()];
    spans.extend(text.spans(room, t));
    Line::from(spans)
}

/// The selected item in full: its state, body, facts and notes.
fn detail_lines(
    item: &Item,
    lane: Lane,
    live: Option<&SessionInfo>,
    facts: Option<&ItemFacts>,
    now: u64,
    t: &Theme,
) -> Vec<Line<'static>> {
    let mut out = vec![
        Line::from(vec![
            format!(" #{} ", item.id).fg(t.muted),
            item.title.clone().fg(t.fg).bold(),
        ]),
        Line::from(vec![
            format!(" {} ", lane.title()).fg(lane.color(t)).bold(),
            match (lane == Lane::Review && item.pr.is_some(), item.pr_closed) {
                (true, true) => "· PR closed ".fg(t.accent2),
                (true, false) => "· PR open ".fg(t.muted),
                (false, _) => "".into(),
            },
            format!("· {} ", item.priority.as_str()).fg(priority_color(item.priority, t)),
            match live {
                Some(s) => format!("· tab {} {} ", s.id, s.status.state.label()).fg(t.muted),
                None => "".into(),
            },
            item.branch
                .clone()
                .map_or("".into(), |b| format!("· {b}").fg(t.muted)),
        ]),
    ];
    if lane == Lane::Proposed {
        out.push(Line::from(
            format!(" by {}, waiting on you", item.by).fg(t.accent2),
        ));
    } else if item.agent_filed() {
        out.push(Line::from(
            format!(" agent-filed by {}, not accepted yet: a", item.by).fg(t.accent2),
        ));
    }
    if !item.blocked_by.is_empty() {
        let ids: Vec<String> = item.blocked_by.iter().map(|id| format!("#{id}")).collect();
        out.push(Line::from(
            format!(" after {}, not finished yet", ids.join(", ")).fg(t.accent2),
        ));
    }
    if let Some(pr) = &item.pr {
        out.push(Line::from(format!(" pr: {pr}").fg(t.muted)));
    }
    if let Some(l) = item.launch.as_ref().filter(|l| !l.model.is_empty()) {
        out.push(Line::from(
            format!(" launches {} on {}", l.agent, l.model).fg(t.muted),
        ));
    }
    if let Some(f) = facts {
        let mut parts = vec![match f.ahead {
            1 => "1 commit".to_owned(),
            n => format!("{n} commits"),
        }];
        if !f.diff.is_empty() {
            parts.push(f.diff.clone());
        }
        if f.dirty > 0 {
            parts.push(format!("{} not committed", f.dirty));
        }
        out.push(Line::default());
        out.push(Line::from(
            format!(" {}", parts.join(" · ")).fg(t.working).bold(),
        ));
        for c in f.commits.iter().take(5) {
            out.push(Line::from(format!("   {c}").fg(t.muted)));
        }
    }
    if !item.body.is_empty() {
        out.push(Line::default());
        for l in item.body.lines().take(8) {
            out.push(Line::from(format!(" {l}").fg(t.fg)));
        }
    }
    for a in &item.attachments {
        let name = a
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(Line::from(format!(" ⎘ {name}").fg(t.muted)));
    }
    for n in item.notes.iter().rev().take(3) {
        let by = if n.by == "human" { "you" } else { &n.by };
        let ago = crate::age(n.at * 1000, now);
        out.push(Line::default());
        let took = if n.taken { " · from a take" } else { "" };
        out.push(Line::from(
            format!(" {by} · {ago}{took}").fg(t.muted).bold(),
        ));
        for l in n.text.lines() {
            out.push(Line::from(format!(" {l}").fg(t.fg)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::AgentStatus;

    fn item(id: u32, updated: u64) -> Item {
        Item {
            priority: Default::default(),
            blocked_by: Vec::new(),
            id,
            title: format!("item {id}"),
            body: String::new(),
            kind: ItemKind::Bug,
            status: ItemStatus::Open,
            project: Some("/r/app".into()),
            attachments: Vec::new(),
            by: "human".into(),
            sessions: Vec::new(),
            boot: 7,
            taken: Vec::new(),
            branch: None,
            branch_taken: false,
            checkout: None,
            pr: None,
            filed_by: None,
            unaccepted: false,
            pr_closed: false,
            joined: Default::default(),
            notes: Vec::new(),
            launch: None,
            created: 0,
            updated,
        }
    }

    fn step(
        item: u32,
        priority: Option<Priority>,
        blocked_by: Option<Vec<u32>>,
    ) -> valkyrie_proto::OrderStep {
        valkyrie_proto::OrderStep {
            item,
            reason: format!("why {item}"),
            priority,
            blocked_by,
        }
    }

    fn proposal(steps: Vec<valkyrie_proto::OrderStep>) -> OrderProposal {
        OrderProposal {
            id: 42,
            project: "/r/app".into(),
            made: 0,
            trigger: "you asked".into(),
            steps,
        }
    }

    #[test]
    fn r_starts_the_offered_ready_item_and_x_leaves_it() {
        let rows = rows(&[item(1, 1), item(2, 1)], &[], 7);
        let mut page = Page::default();
        let press = |page: &mut Page, k: &[u8]| page.keys(k, &rows, &[], 7);
        assert!(matches!(&press(&mut page, b"R")[..], [Do::Say(s)] if s.contains("nothing")));
        page.launches = vec![LaunchOffer {
            id: 5,
            project: "/r/app".into(),
            item: 2,
            title: "item 2".into(),
            agent: "claude".into(),
            model: String::new(),
            why: "next in the backlog; 0 of 3 agent sessions running".into(),
            waived: Vec::new(),
            made: 0,
        }];
        let answer = |launch| Do::AnswerLaunch {
            project: "/r/app".into(),
            id: 5,
            item: 2,
            launch,
        };
        assert_eq!(press(&mut page, b"R"), [answer(true)]);
        assert_eq!(press(&mut page, b"X"), [answer(false)]);
        // Another repository's page has no offer of its own.
        page.repo = Repo::In("/r/other".into());
        assert!(matches!(&press(&mut page, b"R")[..], [Do::Say(_)]));
    }

    #[test]
    fn p_asks_for_the_shown_repositorys_order_and_o_opens_it_to_tick_and_accept() {
        let rows = rows(&[item(1, 1), item(2, 1), item(3, 1)], &[], 7);
        let mut page = Page::default();
        let press = |page: &mut Page, k: &[u8]| page.keys(k, &rows, &[], 7);
        // Every repository shown: the selected item's.
        assert_eq!(press(&mut page, b"P"), [Do::Plan("/r/app".into())]);
        assert!(matches!(&press(&mut page, b"O")[..], [Do::Say(s)] if s.contains("P asks")));
        page.orders = vec![proposal(vec![
            step(2, Some(Priority::P0), None),
            step(3, None, None), // a place only: no tick
            step(1, None, Some(vec![2])),
        ])];
        assert!(press(&mut page, b"O").is_empty());
        assert_eq!(page.order.as_ref().map(|v| v.id), Some(42));
        // Untick #2, try the place-only #3 (nothing to tick), keep #1.
        assert!(press(&mut page, b" j j").is_empty());
        assert_eq!(page.order.as_ref().unwrap().skip, [2]);
        assert_eq!(
            press(&mut page, b"\r"),
            [Do::AnswerOrder {
                project: "/r/app".into(),
                id: 42,
                accept: true,
                items: Some(vec![1]),
            }]
        );
        assert!(page.order.is_none());
        // Opened again and accepted untouched: all of it.
        press(&mut page, b"O");
        assert!(matches!(
            &press(&mut page, b"a")[..],
            [Do::AnswerOrder {
                accept: true,
                items: None,
                ..
            }]
        ));
        press(&mut page, b"O");
        assert!(matches!(
            &press(&mut page, b"d")[..],
            [Do::AnswerOrder { accept: false, .. }]
        ));
        // Esc leaves it for later; one answered elsewhere closes on the next key.
        press(&mut page, b"O");
        assert!(press(&mut page, b"\x1b").is_empty());
        assert!(page.order.is_none());
        press(&mut page, b"O");
        page.orders.clear();
        assert!(matches!(&press(&mut page, b"j")[..], [Do::Say(_)]));
        assert!(page.order.is_none());
    }

    #[test]
    fn a_step_says_what_accepting_it_changes() {
        let mut i = item(1, 1);
        i.blocked_by = vec![5];
        let step = |priority, blocked_by| valkyrie_proto::OrderStep {
            item: 1,
            reason: String::new(),
            priority,
            blocked_by,
        };
        assert_eq!(
            step_changes(&step(Some(Priority::P1), Some(vec![2, 5])), Some(&i)),
            "p2→p1, after #2"
        );
        assert_eq!(step_changes(&step(None, None), Some(&i)), "");
    }

    #[test]
    fn enter_relaunches_on_the_agent_it_last_ran() {
        let launch = |agent| {
            vec![Do::Launch {
                id: 1,
                agent,
                model: None,
                worktree: true,
            }]
        };
        let enter = |item: Item| Page::default().keys(b"\r", &rows(&[item], &[], 7), &[], 7);
        assert_eq!(
            enter(item(1, 1)),
            launch("claude"),
            "never launched: claude"
        );
        let mut codex = item(1, 1);
        codex.launch = Some(ItemLaunch {
            agent: "codex".into(),
            model: "gpt-5.5".into(),
        });
        // The daemon fills in the model it last ran with that agent.
        assert_eq!(enter(codex), launch("codex"));
    }

    #[test]
    fn l_opens_a_window_that_launches_with_a_model() {
        let rows = rows(&[item(1, 1)], &[], 7);
        let mut page = Page::default();
        let press = |page: &mut Page, k: &[u8]| page.keys(k, &rows, &[], 7);
        let launch = |agent, model: &str, worktree| {
            vec![Do::Launch {
                id: 1,
                agent,
                model: Some(model.to_owned()),
                worktree,
            }]
        };
        // It opens on the model, the agent's default; Tab steps through the aliases.
        assert!(press(&mut page, b"L").is_empty());
        assert!(matches!(&page.typing, Some(Typing::Launch(f)) if f.field == Field::Model));
        assert!(press(&mut page, b"\t\t").is_empty());
        assert_eq!(press(&mut page, b"\r"), launch("claude", "sonnet", true));
        assert_eq!(page.typing, None, "launching closes it");

        // Typed, in two reads, and the worktree turned off two fields down.
        assert!(press(&mut page, b"Lclaude-opus").is_empty());
        assert!(press(&mut page, b"-5-5\x1b[B").is_empty());
        assert!(
            press(&mut page, b" ").is_empty(),
            "a space flips the worktree"
        );
        assert_eq!(
            press(&mut page, b"\r"),
            launch("claude", "claude-opus-5-5", false)
        );

        // Up to the agent: codex, whose model starts at its default.
        assert!(press(&mut page, b"Lopus\x1b[A").is_empty());
        assert!(press(&mut page, b"l\x1b[B").is_empty());
        assert!(
            press(&mut page, b"gpt-5").is_empty(),
            "typed on the model field"
        );
        assert_eq!(press(&mut page, b"\r"), launch("codex", "gpt-5", true));

        // Esc gives up, and the keys are the page's again.
        assert!(press(&mut page, b"L").is_empty());
        assert!(press(&mut page, b"\x1b").is_empty());
        assert_eq!(page.typing, None);
        assert_eq!(press(&mut page, b"\x1b"), [Do::Close]);
    }

    #[test]
    fn the_launch_window_starts_from_the_last_launch() {
        let launched = Item {
            launch: Some(ItemLaunch {
                agent: "codex".into(),
                model: "gpt-5-codex".into(),
            }),
            checkout: Some("/r/app-fix-1".into()),
            ..item(1, 1)
        };
        let rows = rows(&[launched], &[], 7);
        let mut page = Page::default();
        let press = |page: &mut Page, k: &[u8]| page.keys(k, &rows, &[], 7);
        assert!(press(&mut page, b"L").is_empty());
        let Some(Typing::Launch(form)) = &page.typing else {
            panic!("{:?}", page.typing)
        };
        assert_eq!((form.agent, form.model.text()), ("codex", "gpt-5-codex"));
        // Its earlier checkout is reused: no worktree field to move to.
        assert_eq!(form.fields(), [Field::Agent, Field::Model]);
        assert!(press(&mut page, b"\x1b[B\x1b[B").is_empty());
        // Back to claude, which it never ran: claude's own default.
        assert!(press(&mut page, b"\x1b[A ").is_empty());
        let Some(Typing::Launch(form)) = &page.typing else {
            panic!()
        };
        assert_eq!((form.agent, form.model.text()), ("claude", ""));
        // A pasted name lands in the model, whichever field it's on.
        assert!(press(&mut page, b"\x1b[200~opus\x1b[201~").is_empty());
        assert_eq!(
            press(&mut page, b"\r"),
            [Do::Launch {
                id: 1,
                agent: "claude",
                model: Some("opus".into()),
                worktree: true,
            }]
        );
    }

    #[test]
    fn hints_too_wide_lose_their_end_not_their_start() {
        let pair = |k: &'static str, w: &'static str| [Span::raw(k), Span::raw(w)];
        let hints = [
            pair(" ↩ ", "launch "),
            pair(" L ", "with… "),
            pair(" esc ", "close "),
        ]
        .concat();
        let words = |h: Vec<Span>| h.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(
            words(fit_hints(hints.clone(), 80)),
            " ↩ launch  L with…  esc close "
        );
        assert_eq!(words(fit_hints(hints, 20)), " ↩ launch  L with… ");
    }

    #[test]
    fn l_says_why_when_it_cant_launch() {
        let proposed = Item {
            status: ItemStatus::Proposed,
            ..item(1, 1)
        };
        let rows = rows(&[proposed], &[], 7);
        let mut page = Page::default();
        assert!(matches!(page.keys(b"L", &rows, &[], 7)[..], [Do::Say(_)]));
        assert_eq!(page.typing, None);
    }

    fn session(id: SessionId, state: AgentState) -> SessionInfo {
        SessionInfo {
            id,
            name: String::new(),
            auto_name: false,
            command: vec![],
            cwd: "/".into(),
            pid: None,
            created_unix: 0,
            title: None,
            clients: 0,
            exited: None,
            status: AgentStatus {
                state,
                ..AgentStatus::default()
            },
            last_input_ms: 0,
            chat: None,
            overlap: None,
        }
    }

    #[test]
    fn rows_put_what_waits_on_you_first_and_only_a_few_done() {
        let stuck = Item {
            sessions: vec![3],
            ..item(1, 1)
        };
        let running = Item {
            sessions: vec![4],
            ..item(2, 9)
        };
        let review = Item {
            sessions: vec![9],
            checkout: Some("/r/app-fix-3".into()),
            ..item(3, 2)
        };
        let proposed = Item {
            status: ItemStatus::Proposed,
            by: "claude".into(),
            ..item(4, 3)
        };
        let fresh = item(5, 4);
        let mut all = vec![fresh, proposed, running, review, stuck];
        all.extend((10..20).map(|i| Item {
            status: ItemStatus::Shipped,
            ..item(i, i as u64)
        }));
        let sessions = [
            session(3, AgentState::NeedsInput),
            session(4, AgentState::Working),
        ];
        let rows = rows(&all, &sessions, 7);
        let order: Vec<(Lane, u32)> = rows.iter().map(|(l, i)| (*l, i.id)).take(5).collect();
        assert_eq!(
            order,
            [
                (Lane::Needs, 1),
                (Lane::Review, 3),
                (Lane::Proposed, 4),
                (Lane::Running, 2),
                (Lane::Backlog, 5)
            ]
        );
        let done: Vec<u32> = rows
            .iter()
            .filter(|(l, _)| *l == Lane::Done)
            .map(|(_, i)| i.id)
            .collect();
        assert_eq!(done, [19, 18, 17, 16, 15], "the newest five");
        // A restarted daemon's session 3 is someone else's: the item waits on review.
        assert_eq!(lane(&rows[0].1, &sessions, 8), Lane::Review);
    }

    #[test]
    fn enter_launches_over_sessions_that_only_took_it_and_o_opens_them() {
        let took = Item {
            sessions: vec![3, 4],
            taken: vec![4],
            ..item(1, 1)
        };
        let working = |id| session(id, AgentState::Working);
        // Only a take is live: the user's launch hands it over; o still attaches.
        let rows = rows(std::slice::from_ref(&took), &[working(4)], 7);
        let mut page = Page::default();
        assert_eq!(
            page.keys(b"\r", &rows, &[working(4)], 7),
            [Do::Launch {
                id: 1,
                agent: "claude",
                model: None,
                worktree: true,
            }]
        );
        assert_eq!(page.keys(b"o", &rows, &[working(4)], 7), [Do::Attach(4)]);
        // Its launched session is live too: that one holds it, so ↩ attaches.
        let live = [working(3), working(4)];
        let rows = super::rows(&[took], &live, 7);
        assert_eq!(page.keys(b"\r", &rows, &live, 7), [Do::Attach(4)]);
    }

    #[test]
    fn an_agents_own_filed_work_follows_its_session_until_accepted() {
        let filed = Item {
            status: ItemStatus::Proposed,
            by: "claude".into(),
            sessions: vec![4],
            taken: vec![4],
            filed_by: Some((7, 4)),
            ..item(1, 1)
        };
        let working = [session(4, AgentState::Working)];
        assert_eq!(lane(&filed, &working, 7), Lane::Running);
        assert_eq!(
            lane(&filed, &[session(4, AgentState::NeedsInput)], 7),
            Lane::Needs
        );
        let pr = Item {
            pr: Some("https://github.com/o/r/pull/7".into()),
            ..filed.clone()
        };
        assert_eq!(lane(&pr, &working, 7), Lane::Review);
        // Proposed by an agent without taking it: waits on the user, as before.
        let proposed = Item {
            filed_by: None,
            unaccepted: false,
            pr_closed: false,
            joined: Default::default(),
            ..filed.clone()
        };
        assert_eq!(lane(&proposed, &working, 7), Lane::Proposed);
        // Filed by an agent that hasn't taken it (yet): a proposal too.
        let untaken = Item {
            sessions: Vec::new(),
            taken: Vec::new(),
            ..filed.clone()
        };
        assert_eq!(lane(&untaken, &[], 7), Lane::Proposed);
        let t = crate::theme::DRACULA;
        let text: Vec<String> = detail_lines(&filed, Lane::Running, None, None, 0, &t)
            .iter()
            .map(|l| l.to_string())
            .collect();
        assert!(text[2].contains("agent-filed by claude"), "{text:?}");
        // Never launched before it's accepted; accepting it is a key away.
        let rows = rows(&[filed], &[], 7);
        assert_eq!(rows[0].0, Lane::Review, "taken: it waits on review");
        let mut page = Page::default();
        assert_eq!(
            page.keys(b"\r", &rows, &[], 7),
            [Do::Say("accept it first: a".into())]
        );
        assert_eq!(
            page.keys(b"a", &rows, &[], 7),
            [Do::Status(1, ItemStatus::Open)]
        );
    }

    #[test]
    fn an_interrupted_session_is_not_review_ready_until_it_has_a_pull_request() {
        let taken = Item {
            sessions: vec![4],
            ..item(1, 1)
        };
        // Esc-interrupted mid-turn: nothing in it is complete, so it isn't up for review.
        assert_eq!(
            lane(&taken, &[session(4, AgentState::Interrupted)], 7),
            Lane::Running
        );
        // A finished turn is.
        assert_eq!(
            lane(&taken, &[session(4, AgentState::ReviewReady)], 7),
            Lane::Review
        );
        let pr = Item {
            pr: Some("https://github.com/o/r/pull/7".into()),
            ..taken
        };
        assert_eq!(
            lane(&pr, &[session(4, AgentState::Interrupted)], 7),
            Lane::Review
        );
    }

    #[test]
    fn a_pull_request_open_waits_on_review_unless_its_session_needs_you() {
        let pr = |id| Item {
            sessions: vec![4],
            pr: Some("https://github.com/o/r/pull/7".into()),
            ..item(id, 1)
        };
        let lane_with = |state| lane(&pr(1), &[session(4, state)], 7);
        assert_eq!(lane_with(AgentState::Working), Lane::Review);
        assert_eq!(lane_with(AgentState::Idle), Lane::Review);
        assert_eq!(lane_with(AgentState::NeedsInput), Lane::Needs);
        assert_eq!(lane_with(AgentState::Blocked), Lane::Needs);
        // Gone, or a restarted daemon's: still waiting on review.
        assert_eq!(lane(&pr(1), &[], 7), Lane::Review);
        let shipped = Item {
            status: ItemStatus::Shipped,
            ..pr(1)
        };
        assert_eq!(lane(&shipped, &[], 7), Lane::Done);
        let t = crate::theme::DRACULA;
        let text: Vec<String> = detail_lines(&pr(1), Lane::Review, None, None, 0, &t)
            .iter()
            .map(|l| l.to_string())
            .collect();
        assert!(text[1].contains("review · PR open"), "{text:?}");
    }

    #[test]
    fn enter_launches_or_opens_and_proposals_need_accepting() {
        let fresh = item(1, 3);
        let live = Item {
            sessions: vec![4],
            ..item(2, 2)
        };
        let proposed = Item {
            status: ItemStatus::Proposed,
            ..item(3, 1)
        };
        let sessions = [session(4, AgentState::Working)];
        let rows = rows(&[fresh, live, proposed], &sessions, 7);
        let mut page = Page::default();
        let press = |page: &mut Page, k: &[u8]| page.keys(k, &rows, &sessions, 7);
        // Proposed first, then running, then the backlog.
        assert!(matches!(press(&mut page, b"\r")[..], [Do::Say(_)]));
        assert_eq!(press(&mut page, b"a"), [Do::Status(3, ItemStatus::Open)]);
        assert_eq!(press(&mut page, b"j\r"), [Do::Attach(4)]);
        assert_eq!(
            press(&mut page, b"j\r"),
            [Do::Launch {
                id: 1,
                agent: "claude",
                model: None,
                worktree: true,
            }]
        );
        assert_eq!(
            press(&mut page, b"c"),
            [Do::Launch {
                id: 1,
                agent: "codex",
                model: None,
                worktree: true,
            }]
        );
        assert_eq!(press(&mut page, b"s"), [Do::Status(1, ItemStatus::Shipped)]);
        assert_eq!(press(&mut page, b"\x1b"), [Do::Close]);
    }

    #[test]
    fn back_to_backlog_stops_a_running_agent_on_the_second_press() {
        let mut started = item(1, 1);
        started.sessions = vec![4];
        let mut kept = item(2, 1);
        kept.checkout = Some("/r/app-wt".into());
        let sessions = [session(4, AgentState::Working)];
        let rows = rows(&[started.clone(), kept.clone()], &sessions, 7);
        // A worktree alone is no start: the item is in the backlog, nothing to return.
        assert_eq!(
            rows.iter().find(|(_, i)| i.id == 2).unwrap().0,
            Lane::Backlog
        );
        let mut page = Page::default();
        let press = |page: &mut Page, k: &[u8]| page.keys(k, &rows, &sessions, 7);
        assert!(
            matches!(press(&mut page, b"B")[..], [Do::Say(_)]),
            "running: asks first"
        );
        assert_eq!(press(&mut page, b"B"), [Do::Return(1)]);
        // Another key in between disarms it.
        press(&mut page, b"B");
        press(&mut page, b"+");
        assert!(matches!(press(&mut page, b"B")[..], [Do::Say(_)]));
        // Not started: said, never sent.
        let at = rows.iter().position(|(_, i)| i.id == 2).unwrap();
        page.cursor = at;
        assert!(matches!(press(&mut page, b"B")[..], [Do::Say(_)]));
    }

    #[test]
    fn delete_takes_two_presses_and_typing_adds_and_notes() {
        let rows = rows(&[item(1, 1)], &[], 7);
        let mut page = Page::default();
        let press = |page: &mut Page, k: &[u8]| page.keys(k, &rows, &[], 7);
        assert!(matches!(press(&mut page, b"x")[..], [Do::Say(_)]));
        assert!(press(&mut page, b"k").is_empty(), "another key disarms it");
        assert!(matches!(press(&mut page, b"x")[..], [Do::Say(_)]));
        assert_eq!(press(&mut page, b"x"), [Do::Delete(1)]);

        // n, then a title typed in two reads with a non-ASCII letter; Tab makes it a feature.
        assert!(press(&mut page, b"nCaf\xc3").is_empty());
        assert!(press(&mut page, b"\xa9 \x7f\tsoon").is_empty());
        assert_eq!(
            press(&mut page, b"\r"),
            [Do::Add {
                kind: ItemKind::Feature,
                text: "Cafésoon".into()
            }]
        );
        assert!(
            press(&mut page, b"Nlooks good\x1b").is_empty(),
            "esc drops the note"
        );
        assert_eq!(
            press(&mut page, b"Nship it\r"),
            [Do::Note {
                id: 1,
                text: "ship it".into()
            }]
        );
        assert_eq!(page.typing, None);

        // The cursor moves: a word typed back at the start, and Delete there.
        assert!(press(&mut page, b"nbug\x1b[Hold \x1b[3~").is_empty());
        assert_eq!(
            press(&mut page, b"\r"),
            [Do::Add {
                kind: ItemKind::Bug,
                text: "old ug".into()
            }]
        );
    }

    #[test]
    fn e_edits_the_title_and_kind_in_place() {
        let rows = rows(&[item(1, 1)], &[], 7);
        let mut page = Page::default();
        let press = |page: &mut Page, k: &[u8]| page.keys(k, &rows, &[], 7);
        let title = rows[0].1.title.clone();
        assert!(press(&mut page, b"e").is_empty());
        assert!(
            matches!(&page.typing, Some(Typing::Edit { id: 1, text, .. }) if text.text() == title),
            "starts from the title, cursor at its end"
        );
        assert!(
            press(&mut page, b"\x1b").is_empty(),
            "esc keeps it as it was"
        );
        assert_eq!(page.typing, None);
        assert_eq!(
            press(&mut page, b"e\x1b[H\x1b[3~X\t\r"),
            [Do::Edit {
                priority: None,
                cwd: None,
                id: 1,
                kind: Some(ItemKind::Feature),
                title: Some(format!("X{}", title.chars().skip(1).collect::<String>())),
            }]
        );
        // Only what changed here goes, so a title changed elsewhere meanwhile stays.
        assert_eq!(
            press(&mut page, b"e\t\r"),
            [Do::Edit {
                priority: None,
                cwd: None,
                id: 1,
                kind: Some(ItemKind::Feature),
                title: None,
            }]
        );
        // Shift-Tab steps through the repositories to move it to; the page's own
        // stays put unless it's picked, and picking it again sends nothing.
        page.roots = vec!["/r/app".into(), "/r/web".into()];
        assert_eq!(
            press(
                &mut page,
                b"e\x1b[Z
"
            ),
            [Do::Edit {
                priority: None,
                cwd: Some("/r/web".into()),
                id: 1,
                kind: None,
                title: None,
            }],
            "moved to the next repository"
        );
        assert!(
            press(
                &mut page,
                b"e\x1b[Z\x1b[Z
"
            )
            .is_empty(),
            "round to where it was: nothing changed"
        );
        assert!(
            press(&mut page, b"e\r").is_empty(),
            "nothing changed, nothing sent"
        );
        assert!(
            press(&mut page, b"e\x15\r").is_empty(),
            "a blank title isn't saved"
        );
    }

    #[test]
    fn capital_s_asks_then_ships_what_it_showed_but_not_while_typing() {
        let rows = rows(&[item(1, 1)], &[], 7);
        let mut page = Page::default();
        let press = |page: &mut Page, k: &[u8]| page.keys(k, &rows, &[], 7);
        assert_eq!(press(&mut page, b"S"), [Do::CheckMerged]);
        assert_eq!(
            page.keys(b"S", &[], &[], 7),
            [Do::CheckMerged],
            "with nothing selected too"
        );
        // What the check found arms it: the second S ships exactly those.
        page.ship_armed = Some(vec![1, 4]);
        assert_eq!(press(&mut page, b"S"), [Do::ShipMerged(vec![1, 4])]);
        assert_eq!(press(&mut page, b"S"), [Do::CheckMerged], "once");
        page.ship_armed = Some(vec![1]);
        assert!(press(&mut page, b"j").is_empty());
        assert_eq!(
            press(&mut page, b"S"),
            [Do::CheckMerged],
            "another key disarms"
        );
        page.ship_armed = Some(vec![1]);
        assert!(press(&mut page, b"NSent").is_empty(), "typed into the note");
        assert_eq!(page.ship_armed, None);
        assert_eq!(
            press(&mut page, b"\r"),
            [Do::Note {
                id: 1,
                text: "Sent".into()
            }]
        );
    }

    #[test]
    fn the_ship_merged_summary_counts_what_stayed() {
        let shipped = |id| ShippedItem {
            id,
            title: String::new(),
            branch: format!("fix/{id}"),
            branch_taken: id == 4,
            filed: id == 6,
            against: "origin/main".into(),
            note: None,
        };
        let kept = |id, why: &str| KeptItem {
            id,
            title: String::new(),
            why: why.into(),
        };
        assert_eq!(
            shipped_summary(
                &[shipped(1), shipped(4)],
                &[
                    kept(2, "not merged"),
                    kept(3, "not merged"),
                    kept(9, "no commits of its own")
                ],
                0,
                Said::Shipped
            ),
            "shipped #1 #4; 2 not merged; #9 no commits of its own"
        );
        let found = [shipped(1), shipped(4)];
        assert_eq!(
            shipped_summary(&found, &[kept(2, "not merged")], 0, Said::Confirm),
            "ship #1 fix/1, #4 fix/4 (take)? S again; 1 not merged"
        );
        let filed = ShippedItem {
            branch_taken: true,
            ..shipped(6)
        };
        assert_eq!(
            shipped_summary(&[filed], &[], 0, Said::Confirm),
            "ship #6 fix/6 (take, agent-filed)? S again"
        );
        assert_eq!(
            shipped_summary(&found, &[], 0, Said::Found),
            "merged: #1 fix/1, #4 fix/4 (take) (b, then S S to ship)"
        );
        assert_eq!(
            shipped_summary(&[], &[], 1, Said::Confirm),
            "nothing merged to ship; 1 not fetched, judged as last fetched"
        );
        assert_eq!(
            shipped_summary(&[], &[], 0, Said::Shipped),
            "nothing shipped"
        );
    }

    #[test]
    fn an_answer_arms_only_an_open_page_and_says_shipped_only_when_it_did() {
        let found = [ShippedItem {
            id: 1,
            title: String::new(),
            branch: "fix/a".into(),
            branch_taken: false,
            filed: false,
            against: "origin/main".into(),
            note: None,
        }];
        assert_eq!(
            answer(true, true, &found, &[], 0),
            (Some(vec![1]), "ship #1 fix/a? S again".into())
        );
        assert_eq!(
            answer(true, false, &found, &[], 0),
            (None, "merged: #1 fix/a (b, then S S to ship)".into())
        );
        assert_eq!(
            answer(false, true, &found, &[], 0),
            (None, "shipped #1".into())
        );
        assert_eq!(
            answer(false, false, &[], &[], 0),
            (None, "nothing shipped".into())
        );
        assert_eq!(
            answer(true, true, &[], &[], 0),
            (None, "nothing merged to ship".into())
        );
    }

    #[test]
    fn the_detail_shows_the_pull_request() {
        let t = crate::theme::DRACULA;
        let pr = "https://github.com/o/r/pull/7";
        let taken = Item {
            pr: Some(pr.into()),
            ..item(1, 1)
        };
        let text: Vec<String> = detail_lines(&taken, Lane::Review, None, None, 0, &t)
            .iter()
            .map(|l| l.to_string())
            .collect();
        assert!(text.contains(&format!(" pr: {pr}")), "{text:?}");
    }

    #[test]
    fn the_new_item_label_takes_its_kinds_color() {
        let t = crate::theme::DRACULA;
        let label = |kind| {
            let typing = Typing::Add {
                kind,
                text: LineEdit::default(),
            };
            typing_line(&typing, 40, &t).spans[0].style.bg
        };
        assert_eq!(label(ItemKind::Bug), Some(kind_color(ItemKind::Bug, &t)));
        assert_eq!(
            label(ItemKind::Feature),
            Some(kind_color(ItemKind::Feature, &t))
        );
        assert_ne!(label(ItemKind::Bug), label(ItemKind::Feature));
    }

    #[test]
    fn r_cycles_every_repository_then_each_by_name_then_no_folder() {
        let at = |id, project: Option<&str>| Item {
            project: project.map(PathBuf::from),
            ..item(id, 1)
        };
        let items = [
            at(1, Some("/w/zeta")),
            at(2, Some("/home/alpha")),
            at(3, None),
            at(4, Some("/w/zeta")),
        ];
        let mut repo = Repo::All;
        let mut seen = Vec::new();
        for _ in 0..4 {
            repo = repo.next(&items);
            seen.push(repo.clone());
        }
        assert_eq!(
            seen,
            [
                Repo::In("/home/alpha".into()),
                Repo::In("/w/zeta".into()),
                Repo::NoFolder,
                Repo::All,
            ]
        );
        let held = |r: Repo| {
            items
                .iter()
                .filter(|i| r.holds(i))
                .map(|i| i.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(held(Repo::In("/w/zeta".into())), [1, 4]);
        assert_eq!(held(Repo::NoFolder), [3]);
        assert_eq!(held(Repo::All), [1, 2, 3, 4]);
        // One whose items are gone starts over.
        assert_eq!(Repo::In("/gone".into()).next(&items), Repo::All);
        // And `r` asks for it, with or without items shown.
        let mut page = Page::default();
        assert_eq!(page.keys(b"r", &[], &[], 7), [Do::NextRepo]);
    }

    #[test]
    fn each_row_shows_its_state_and_still_fits_the_width() {
        let t = crate::theme::DRACULA;
        for lane in Lane::ORDER {
            for width in [40u16, 80] {
                let line = item_line(&item(1, 1), lane, true, width, &t);
                let text = line.to_string();
                assert!(text.contains(lane.title()), "{lane:?} at {width}: {text:?}");
                assert_eq!(
                    line.width(),
                    width as usize,
                    "{lane:?} at {width}: {text:?}"
                );
            }
        }
    }

    #[test]
    fn a_stray_csi_before_esc_does_not_hold_the_prompt() {
        let mut page = Page {
            typing: Some(Typing::add()),
            ..Page::default()
        };
        page.keys(b"\x1b[\x1b", &[], &[], 7);
        assert!(page.typing.is_none(), "Esc cancels at once");
        assert!(page.held.is_empty());
    }

    #[test]
    fn a_cut_sequence_is_held_only_briefly() {
        let mut page = Page {
            typing: Some(Typing::add()),
            ..Page::default()
        };
        page.keys(b"\x1b[1;", &[], &[], 7);
        assert_eq!(page.held, b"\x1b[1;");
        let mut page = Page {
            typing: Some(Typing::add()),
            ..Page::default()
        };
        page.keys(&[b"\x1b[".as_slice(), &[b'1'; 40]].concat(), &[], &[], 7);
        assert!(page.held.is_empty(), "past the cap it isn't held");
    }

    #[test]
    fn a_lane_is_in_the_order_to_work_it_and_plus_minus_move_priority() {
        let mut urgent = item(1, 1);
        urgent.priority = Priority::P0;
        let mut waits = item(2, 9);
        waits.priority = Priority::P0;
        waits.blocked_by = vec![3, 4];
        let later = item(3, 5);
        let mut gone = item(4, 1);
        gone.status = ItemStatus::Shipped;
        let all = [urgent, waits, later, gone];
        let rows = rows(&all, &[], 7);
        let order: Vec<(Lane, u32)> = rows.iter().map(|(l, i)| (*l, i.id)).collect();
        assert_eq!(
            order,
            [
                (Lane::Backlog, 1),
                (Lane::Backlog, 3),
                (Lane::Backlog, 2),
                (Lane::Done, 4)
            ]
        );
        // The row keeps only what's still open of what it waits on.
        assert_eq!(rows[2].1.blocked_by, [3]);
        let t = crate::theme::DRACULA;
        let line: String = item_line(&rows[2].1, Lane::Backlog, false, 90, &t)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(line.contains("p0") && line.contains("after #3"), "{line}");

        let mut page = Page::default();
        let edit = |priority| Do::Edit {
            id: 3,
            kind: None,
            title: None,
            priority: Some(priority),
            cwd: None,
        };
        page.cursor = 1;
        assert_eq!(page.keys(b"+", &rows, &[], 7), [edit(Priority::P1)]);
        assert_eq!(page.keys(b"-", &rows, &[], 7), [edit(Priority::P3)]);
        page.cursor = 0;
        assert!(
            matches!(&page.keys(b"+", &rows, &[], 7)[..], [Do::Say(s)] if s.contains("already p0"))
        );
    }
}

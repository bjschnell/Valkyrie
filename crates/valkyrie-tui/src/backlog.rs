//! The backlog page (DESIGN §8.12): `b` on the home screen, or in tab mode over a
//! session. The same lanes as the web board, as one list, since four columns don't
//! fit a terminal; the selected item's detail beside or below it.

use crate::edit::LineEdit;
use crate::search::{Search, Searched};
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
            Lane::Proposed => t.accent_dim(),
            Lane::Needs => t.needs,
            Lane::Running => t.working,
            Lane::Review => t.done,
            Lane::Backlog => t.muted,
            Lane::Done => t.muted,
        }
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
#[cfg(test)]
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

/// The backlog's filter row (spec: Backlog), cycled with `[ ]` with Pull
/// requests as its last stop: every lane, then the ones that need nothing
/// first, the ones agents are on, and the ones in review.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Filter {
    #[default]
    All,
    Ready,
    Running,
    Review,
}

impl Filter {
    pub(crate) const ALL: [Filter; 4] =
        [Filter::All, Filter::Ready, Filter::Running, Filter::Review];

    /// The stop after (`forward`) or before `at` on the row, where `None` is
    /// Pull requests, the last stop, and round again.
    pub(crate) fn step(at: Option<Filter>, forward: bool) -> Option<Filter> {
        let stops = Filter::ALL;
        let n = stops.len() + 1;
        let at = at
            .and_then(|f| stops.iter().position(|&s| s == f))
            .unwrap_or(stops.len());
        let next = if forward {
            (at + 1) % n
        } else {
            (at + n - 1) % n
        };
        stops.get(next).copied()
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Ready => "Ready",
            Filter::Running => "Running",
            Filter::Review => "Review",
        }
    }

    /// Whether a row in `lane` (its `blocked_by` worked out) shows. Ready is
    /// what `valk todo next` would pick from: in the backlog, waiting on
    /// nothing unfinished. Proposed and done items show under All only.
    pub(crate) fn shows(self, lane: Lane, item: &Item) -> bool {
        match self {
            Filter::All => true,
            Filter::Ready => lane == Lane::Backlog && item.blocked_by.is_empty(),
            Filter::Running => matches!(lane, Lane::Running | Lane::Needs),
            Filter::Review => lane == Lane::Review,
        }
    }

    /// What it says when it shows nothing.
    fn none(self) -> &'static str {
        match self {
            Filter::All => "nothing written down: n adds a bug or an idea",
            Filter::Ready => "nothing ready: every open item waits on another, or is under way",
            Filter::Running => "no agent is on an item",
            Filter::Review => "nothing in review",
        }
    }
}

/// What `/` matches an item by: its title, `#number`, kind and repository.
fn said(item: &Item) -> String {
    let repo = item
        .project
        .as_ref()
        .and_then(|p| p.file_name())
        .map_or_else(|| "no folder".into(), |n| n.to_string_lossy().into_owned());
    format!(
        "{} #{} {} {}",
        item.title,
        item.number,
        item.kind.as_str(),
        repo
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Typing {
    /// A new item: Tab cycles its kind.
    Add { kind: ItemKind, text: LineEdit },
    /// A handoff note on item `id`.
    Note {
        id: u32,
        number: u32,
        text: LineEdit,
    },
    /// Item `id`'s title, its kind with Tab and its repository with Shift-Tab; the
    /// body stays as it is. `was` holds all three as the prompt opened, so only
    /// what changed here is sent.
    Edit {
        id: u32,
        number: u32,
        kind: ItemKind,
        project: Option<PathBuf>,
        text: LineEdit,
        was: (String, ItemKind, Option<PathBuf>),
    },
    /// `L`'s window: what item `id` launches with.
    Launch(LaunchForm),
    /// A repository to add by path (`^R` in an edit, backlog #240): Enter adds it
    /// and goes back to `back` with it picked, Esc goes back as it was.
    Repo { text: LineEdit, back: Box<Typing> },
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
    /// `id`'s number in its repository, for the title.
    pub(crate) number: u32,
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
pub(crate) fn last_agent(item: &Item) -> &'static str {
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
            number: item.number,
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
    /// The filter row's stop (`[ ]`, the app's).
    pub(crate) filter: Filter,
    /// `/`: items whose title, number, kind or repository has every word.
    pub(crate) search: Search,
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
    /// The repositories an edit can move an item to: the daemon's list and every
    /// item's, by name.
    pub(crate) roots: Vec<PathBuf>,
}

impl Page {
    /// On the list itself, nothing typed or open over it: where section keys
    /// (`g b`, `[ ]`) are the app's, not the page's.
    pub(crate) fn at_list(&self) -> bool {
        self.typing.is_none() && self.order.is_none() && !self.search.typing
    }
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
pub(crate) fn decode(partial: &mut Vec<u8>, bytes: &[u8]) -> String {
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
    /// Add this repository to the list, then type `back` again with it picked.
    AddRepo {
        path: String,
        back: Box<Typing>,
    },
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
    /// Whether `/` leaves `item` in: every word typed is in its title,
    /// number, kind or repository.
    pub(crate) fn holds(&self, item: &Item) -> bool {
        self.search.matches(&said(item))
    }

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
                let editing = matches!(typing, Typing::Edit { .. });
                let adding_repo = matches!(typing, Typing::Repo { .. });
                let text = match typing {
                    Typing::Add { text, .. }
                    | Typing::Note { text, .. }
                    | Typing::Edit { text, .. }
                    | Typing::Repo { text, .. } => text,
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
                            Typing::Note { id, text, .. } if !text.text().trim().is_empty() => {
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
                                ..
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
                            Typing::Repo { text, back } if !text.text().trim().is_empty() => {
                                out.push(Do::AddRepo {
                                    path: text.text().trim().into(),
                                    back,
                                });
                            }
                            // Nothing typed: back to the edit.
                            Typing::Repo { back, .. } => self.typing = Some(*back),
                            _ => {}
                        }
                    }
                    [0x12] if editing => {
                        let back = Box::new(self.typing.take().unwrap());
                        self.typing = Some(Typing::Repo {
                            text: LineEdit::new(String::new()),
                            back,
                        });
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
                    b"\x1b" | [0x03] if adding_repo && (whole || key == [0x03]) => {
                        if let Some(Typing::Repo { back, .. }) = self.typing.take() {
                            self.typing = Some(*back);
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
            let searching = self.search.typing;
            match self.search.key(key, whole) {
                Searched::No => {}
                // The search ended: the rest of the read would act on rows
                // listed before it narrowed them.
                _ if searching && !self.search.typing => {
                    self.cursor = 0;
                    break;
                }
                Searched::Kept => continue,
                Searched::Changed => {
                    self.cursor = 0;
                    self.armed = None;
                    continue;
                }
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
                b"n" => {
                    self.typing = Some(Typing::Add {
                        kind: ItemKind::Bug,
                        text: LineEdit::default(),
                    })
                }
                b"P" => out.push(match self.scope(rows) {
                    Some(project) => Do::Plan(project),
                    None => Do::Say("select an item in a repository first".into()),
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
                Typing::Add { text, .. }
                | Typing::Note { text, .. }
                | Typing::Edit { text, .. }
                | Typing::Repo { text, .. },
            ) => text.insert(&decode(&mut self.partial, &typed)),
            Some(Typing::Launch(form)) => form.model.insert(&decode(&mut self.partial, &typed)),
            None => {}
        }
        out
    }

    /// The repository `P` and `O` are about: the selected item's.
    fn scope(&self, rows: &[(Lane, Item)]) -> Option<PathBuf> {
        rows.get(self.cursor).and_then(|(_, i)| i.project.clone())
    }

    /// Opens `order` over the page, as `O` would (the supervisor page's ↩).
    pub(crate) fn show_order(&mut self, order: &OrderProposal) {
        self.order = Some(OrderView {
            id: order.id,
            project: order.project.clone(),
            cursor: 0,
            skip: Vec::new(),
        });
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
        let last = steps.len().saturating_sub(1);
        match key {
            b"k" | b"\x1b[A" | b"\x1bOA" => view.cursor = view.cursor.saturating_sub(1),
            b"j" | b"\x1b[B" | b"\x1bOB" => view.cursor = (view.cursor + 1).min(last),
            // A big backlog's order is long: a page, or an end, at a time.
            b"\x1b[5~" => view.cursor = view.cursor.saturating_sub(ORDER_PAGE),
            b"\x1b[6~" => view.cursor = (view.cursor + ORDER_PAGE).min(last),
            b"g" | b"\x1b[H" | b"\x1b[1~" => view.cursor = 0,
            b"G" | b"\x1b[F" | b"\x1b[4~" => view.cursor = last,
            // Every change ticked unless all already are; then none.
            b"t" => {
                let changing = steps.iter().filter(|s| s.change().is_some());
                view.skip = if view.skip.is_empty() {
                    changing.map(|s| s.item).collect()
                } else {
                    Vec::new()
                };
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
        let num = item.number;
        let live = live(item, sessions, boot).map(|s| s.id);
        let hand_over = taken_only(item, sessions, boot);
        let launchable = item.status == ItemStatus::Open && (live.is_none() || hand_over);
        let act = match key {
            b"\r" | b"\n" => match live {
                Some(s) if !hand_over => Do::Attach(s),
                _ if item.status == ItemStatus::Proposed => Do::Say("accept it first: a".into()),
                _ if !launchable => {
                    Do::Say(format!("#{num} is {}: o reopens it", item.status.as_str()))
                }
                _ if item.project.is_none() => {
                    Do::Say(format!("#{num} has no folder yet: pick one in the web app"))
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
                Do::Say(format!("an agent is on #{num}: ↩ opens it"))
            }
            b"L" if !launchable => {
                Do::Say(format!("#{num} is {}: o reopens it", item.status.as_str()))
            }
            b"L" => Do::Say(format!("#{num} has no folder yet: pick one in the web app")),
            b"a" if item.status == ItemStatus::Proposed => Do::Status(id, ItemStatus::Open),
            b"s" if item.status == ItemStatus::Open || item.agent_filed() => {
                Do::Status(id, ItemStatus::Shipped)
            }
            b"d" if item.status != ItemStatus::Dropped => Do::Status(id, ItemStatus::Dropped),
            b"B" if item.sessions.is_empty() => Do::Say(format!("#{num} isn't started")),
            b"B" if item.pr.is_some() => {
                Do::Say(format!("#{num} has a pull request: ship or drop it"))
            }
            b"B" if live.is_some() && self.returning != Some(id) => {
                self.returning = Some(id);
                Do::Say(format!("B again stops its agent and returns #{num}"))
            }
            b"B" if matches!(lane, Lane::Done | Lane::Proposed) => {
                Do::Say(format!("#{num} is {}", lane.title()))
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
                    None => Do::Say(format!("#{num} is already {}", item.priority.as_str())),
                }
            }
            b"o" if lane == Lane::Done => Do::Status(id, ItemStatus::Open),
            b"o" if live.is_some() => Do::Attach(live.unwrap_or_default()),
            b"N" => {
                self.typing = Some(Typing::Note {
                    id,
                    number: item.number,
                    text: LineEdit::default(),
                });
                return Vec::new();
            }
            b"e" => {
                self.typing = Some(Typing::Edit {
                    id,
                    number: item.number,
                    kind: item.kind,
                    project: item.project.clone(),
                    text: LineEdit::new(item.title.clone()),
                    was: (item.title.clone(), item.kind, item.project.clone()),
                });
                return Vec::new();
            }
            b"x" if live.is_some() => Do::Say(format!("an agent is still running on #{num}")),
            b"x" if self.armed == Some(id) => {
                self.armed = None;
                Do::Delete(id)
            }
            b"x" => {
                self.armed = Some(id);
                Do::Say(format!("x again deletes #{num} for good"))
            }
            _ => return Vec::new(),
        };
        vec![act]
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &self,
        frame: &mut ratatui::Frame,
        rows: &[(Lane, Item)],
        all: &[Item],
        sessions: &[SessionInfo],
        boot: u64,
        now: u64,
        t: &Theme,
    ) {
        let area = frame.area();
        // Nearly the whole screen, a margin around it so it reads as over what's under.
        let rect = crate::section_rect(area);
        frame.render_widget(Clear, rect);
        let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
        let here = rows.get(self.cursor);
        let hints: Vec<_> = match (&self.typing, here) {
            (None, _) if self.search.typing => {
                [hint(" ↩ ", "keep "), hint(" esc ", "clear ")].concat()
            }
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
                hint(" ^r ", "add repo "),
                hint(" esc ", "cancel "),
            ]
            .concat(),
            (Some(Typing::Repo { .. }), _) => {
                [hint(" ↩ ", "add "), hint(" esc ", "back ")].concat()
            }
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
            // Five at most, the primary first (spec: One key grammar); the
            // rest are on `?`.
            (None, Some((Lane::Proposed, _))) => [
                hint(" a ", "accept "),
                hint(" e ", "edit "),
                hint(" d ", "drop "),
                hint(" x ", "delete "),
                hint(" +/- ", "priority "),
            ]
            .concat(),
            (None, Some((Lane::Done, _))) => [
                hint(" o ", "reopen "),
                hint(" e ", "edit "),
                hint(" x ", "delete "),
                hint(" S ", "ship merged "),
                hint(" n ", "new "),
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
                    hint(" e ", "edit "),
                    hint(" s ", "ship "),
                    hint(" N ", "note "),
                    hint(" d ", "drop "),
                ];
                // The primary and its second (`L with…`, `o`), then the rest:
                // five in all.
                [open, rest.concat()]
                    .concat()
                    .into_iter()
                    .take(10)
                    .collect()
            }
            (None, None) => [hint(" n ", "new "), hint(" / ", "search ")].concat(),
        };
        let around = if self.typing.is_some() || self.search.typing {
            ""
        } else {
            "[ ] filter · j/k move"
        };
        let footer = crate::page_footer(hints, around, rect.width.saturating_sub(2), t);
        let open = rows.iter().filter(|(l, _)| *l != Lane::Done).count();
        let block = crate::section_block(t)
            .title(Line::from(
                [
                    vec![" Backlog ".fg(t.fg).bold()],
                    // The filter row's stop: on the strip from 100 columns,
                    // which a narrow screen abbreviates, so here too.
                    match self.filter {
                        Filter::All => vec![],
                        f => vec![format!("{} ", f.label()).fg(t.accent).bold()],
                    },
                    vec![format!("{open} open ").fg(t.muted)],
                    if self.waiting_order(rows) {
                        vec![" build order waits: O ".fg(t.accent).bold()]
                    } else {
                        vec![]
                    },
                    match self.waiting_launch(rows) {
                        Some(o) => vec![
                            format!(" #{} ready: R starts it, X leaves it ", o.number)
                                .fg(t.accent)
                                .bold(),
                        ],
                        None => vec![],
                    },
                ]
                .concat(),
            ))
            .title_bottom(footer.left_aligned());
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
        if self.search.shown() {
            lines.push(self.search.line(t));
        }
        let mut cursor_line = 0;
        let mut last = None;
        for (i, (lane, item)) in rows.iter().enumerate() {
            if last != Some(*lane) {
                if last.is_some() {
                    lines.push(Line::default());
                }
                let n = rows.iter().filter(|(l, _)| l == lane).count();
                lines.push(Line::from(vec![
                    format!(" {} ", lane.title()).fg(t.dim()).bold(),
                    format!("{n}").fg(t.muted),
                ]));
                last = Some(*lane);
            }
            if i == self.cursor {
                cursor_line = lines.len();
            }
            let group = group_tag(item, all);
            let after = tags(item, &item.blocked_by, all);
            lines.push(item_line(
                item,
                *lane,
                &group,
                &after,
                i == self.cursor,
                list.width,
                t,
            ));
        }
        if rows.is_empty() {
            lines.push(Line::default());
            let none = if self.search.query.is_empty() {
                format!("  {}", self.filter.none())
            } else {
                format!("  nothing matches “{}”: esc clears it", self.search.query)
            };
            lines.push(Line::from(fit(&none, list.width as usize).fg(t.muted)));
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
            let blocked = tags(item, &item.blocked_by, all);
            let mut text = detail_lines(
                item,
                *lane,
                live(item, sessions, boot),
                facts,
                &blocked,
                now,
                t,
            );
            // Its group, under the title and state lines.
            let group = group_lines(item, all, t);
            let at = text.len().min(2);
            text.splice(at..at, group);
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

    /// The launch offer to show: the selected item's repository's, else the
    /// first.
    fn waiting_launch(&self, rows: &[(Lane, Item)]) -> Option<&LaunchOffer> {
        let scope = self.scope(rows);
        self.launches
            .iter()
            .find(|o| Some(&o.project) == scope.as_ref())
            .or(self.launches.first())
    }

    /// Whether a build order waits.
    fn waiting_order(&self, _rows: &[(Lane, Item)]) -> bool {
        !self.orders.is_empty()
    }
}

/// Steps PgUp and PgDn move in a build order.
const ORDER_PAGE: usize = 10;

/// A build order over the page: each step with its tick, what accepting it
/// changes, and why. As tall as it needs up to the page, and scrolled so the
/// whole step under the cursor shows, its reason too.
fn order_window(
    frame: &mut ratatui::Frame,
    over: Rect,
    view: &OrderView,
    order: &OrderProposal,
    rows: &[(Lane, Item)],
    t: &Theme,
) {
    // A dialog: the page behind it steps back.
    crate::dim(frame, over, t);
    let width = over.width.saturating_sub(6).min(100);
    let inner_width = width.saturating_sub(2) as usize;
    let (lines, at) = order_lines(view, order, rows, inner_width, t);
    let height = (lines.len() as u16 + 2)
        .min(over.height.saturating_sub(4))
        .max(3);
    let rect =
        Rect::new(over.x + (over.width - width) / 2, over.y + 2, width, height).intersection(over);
    frame.render_widget(Clear, rect);
    let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
    let hints = [
        hint(" ␣ ", "tick "),
        hint(" ↩ ", "accept ticked "),
        hint(" d ", "decline "),
        hint(" esc ", "later "),
        hint(" t ", "tick all/none "),
        hint(" g/G ", "ends "),
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
            format!("{}/{} ", view.cursor + 1, order.steps.len()).fg(t.accent),
            format!("({}) ", order.trigger).fg(t.muted),
        ]))
        .title_bottom(Line::from(fit_hints(hints, width.saturating_sub(4) as usize)).centered())
        .style(style::Style::new().bg(t.panel).fg(t.fg));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    let top = order_scroll(at, inner.height as usize);
    frame.render_widget(Paragraph::new(lines).scroll((top as u16, 0)), inner);
}

/// The build order's lines at `width` columns, wrapped here so each is one row,
/// and the rows the cursor's step takes.
fn order_lines(
    view: &OrderView,
    order: &OrderProposal,
    rows: &[(Lane, Item)],
    width: usize,
    t: &Theme,
) -> (Vec<Line<'static>>, std::ops::Range<usize>) {
    let mut lines = Vec::new();
    let mut at = 0..0;
    for (n, step) in order.steps.iter().enumerate() {
        let start = lines.len();
        let item = rows.iter().map(|(_, i)| i).find(|i| i.id == step.item);
        let changes = step_changes(step, item, rows);
        let tick = match (step.change().is_some(), view.skip.contains(&step.item)) {
            (false, _) => "   ",
            (true, true) => "[ ]",
            (true, false) => "[x]",
        };
        let title = item.map_or("(not shown)", |i| i.title.as_str());
        let number = item.map_or(step.item, |i| i.number);
        let head = format!(" {tick} {:>2}. #{number} ", n + 1);
        let room = width.saturating_sub(head.chars().count() + changes.chars().count() + 2);
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
            const INDENT: &str = "          ";
            for row in
                crate::comments::wrap(&step.reason, width.saturating_sub(INDENT.len()).max(1))
            {
                lines.push(Line::from(format!("{INDENT}{row}").fg(t.muted)));
            }
        }
        if n == view.cursor {
            at = start..lines.len();
        }
    }
    (lines, at)
}

/// The first row to show in `room` rows: no further down than the top, and far
/// enough that the cursor's step (`at`) shows whole, its head first when it's
/// taller than the window.
fn order_scroll(at: std::ops::Range<usize>, room: usize) -> usize {
    at.end.saturating_sub(room.max(1)).min(at.start)
}

/// What accepting `step` changes on `item`: `p2→p1, after #37`; empty for none.
pub(crate) fn step_changes(
    step: &valkyrie_proto::OrderStep,
    item: Option<&Item>,
    rows: &[(Lane, Item)],
) -> String {
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
            .map(|b| {
                let number = rows
                    .iter()
                    .find(|(_, i)| i.id == *b)
                    .map_or(*b, |(_, i)| i.number);
                format!("#{number}")
            })
            .collect();
        if !added.is_empty() {
            parts.push(format!("after {}", added.join(", ")));
        }
    }
    parts.join(", ")
}

/// The hints, key and word in pairs, that fit in `room` columns: the first ones,
/// which matter most, since a centered title too wide loses its start.
pub(crate) fn fit_hints(hints: Vec<Span<'static>>, room: usize) -> Vec<Span<'static>> {
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
    crate::dim(frame, over, t);
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
        .title(format!(" Launch #{} ", form.number).fg(t.fg).bold())
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
    let ids: Vec<String> = shipped.iter().map(|s| format!("#{}", s.number)).collect();
    let branches: Vec<String> = shipped
        .iter()
        .map(|s| {
            let marks: Vec<&str> = [(s.branch_taken, "take"), (s.filed, "agent-filed")]
                .into_iter()
                .filter_map(|(on, mark)| on.then_some(mark))
                .collect();
            match marks.is_empty() {
                true => format!("#{} {}", s.number, s.branch),
                false => format!("#{} {} ({})", s.number, s.branch, marks.join(", ")),
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
        parts.push(format!("#{} {}", k.number, k.why));
    }
    if unfetched > 0 {
        parts.push(format!("{unfetched} not fetched, judged as last fetched"));
    }
    parts.join("; ")
}

/// ` ↳#3` for a piece of #3, ` 2/5` for a group's parent with two of its five
/// pieces shipped or dropped, else nothing.
fn group_tag(item: &Item, all: &[Item]) -> String {
    if let Some(p) = item.parent {
        let number = all.iter().find(|i| i.id == p).map_or(p, |i| i.number);
        return format!(" ↳#{number}");
    }
    match item.progress(all) {
        (_, 0) => String::new(),
        (done, of) => format!(" {done}/{of}"),
    }
}

/// The detail pane's say on `item`'s group: the item it's a piece of, or its
/// own pieces with their states.
fn group_lines(item: &Item, all: &[Item], t: &Theme) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    if let Some(p) = item.parent.and_then(|p| all.iter().find(|i| i.id == p)) {
        out.push(Line::from(
            format!(
                " piece of #{} {} ({})",
                p.number,
                p.title,
                p.status.as_str()
            )
            .fg(t.accent),
        ));
    }
    let pieces = item.children(all);
    if !pieces.is_empty() {
        let (done, of) = item.progress(all);
        out.push(Line::from(
            format!(" pieces {done}/{of} done").fg(t.accent).bold(),
        ));
        for c in pieces {
            let fg = if c.closed() { t.muted } else { t.fg };
            out.push(Line::from(
                format!("   #{} {} ({})", c.number, c.title, c.status.as_str()).fg(fg),
            ));
        }
    }
    out
}

/// What people call the items `ids` name, seen from `item` (decision #73): `#3`
/// for one in its repository, `repo#3` for one in another, `#id` for one gone.
fn tags(item: &Item, ids: &[u32], all: &[Item]) -> Vec<String> {
    ids.iter()
        .map(|id| match all.iter().find(|i| i.id == *id) {
            Some(i) if i.project == item.project => format!("#{}", i.number),
            Some(i) => {
                let repo = i
                    .project
                    .as_deref()
                    .and_then(|p| p.file_name())
                    .map_or("no-folder".into(), |n| n.to_string_lossy().into_owned());
                format!("{repo}#{}", i.number)
            }
            None => format!("#{id}"),
        })
        .collect()
}

/// `after`: what it still waits on, as `tags` names them.
fn item_line(
    item: &Item,
    lane: Lane,
    group: &str,
    after: &[String],
    selected: bool,
    width: u16,
    t: &Theme,
) -> Line<'static> {
    let folder = item
        .project
        .as_ref()
        .and_then(|p| p.file_name())
        .map_or("no folder".into(), |n| n.to_string_lossy().into_owned());
    let head = format!(" #{:<4}", item.number);
    let prio = format!("{:<3}", item.priority.as_str());
    let kind = format!("{:<8}", item.kind.as_str());
    // The lane as a column too, so a row still says its state when the heading scrolls away.
    let state = format!("{:<12}", lane.title());
    let folder = format!("  {}", fit(&folder, 16));
    // What it still waits on (`rows_of` cut `blocked_by` to that), before the folder.
    let after = match after {
        [] => String::new(),
        [one] => format!(" after {one}"),
        [one, rest @ ..] => format!(" after {one} +{}", rest.len()),
    };
    let room = (width as usize).saturating_sub(
        head.len()
            + prio.len()
            + kind.len()
            + state.len()
            + group.chars().count()
            + after.len()
            + folder.chars().count()
            + 1,
    );
    let title_fg = if lane == Lane::Done { t.muted } else { t.fg };
    let mut line = Line::from(vec![
        head.fg(t.muted),
        prio.fg(t.muted),
        kind.fg(t.muted),
        state.fg(lane.color(t)),
        // Padded, so the folders line up at the right.
        format!("{:<room$}", fit(&item.title, room)).fg(title_fg),
        group.to_owned().fg(t.accent),
        after.fg(t.muted),
        folder.fg(t.muted),
    ]);
    if selected {
        let pad = (width as usize).saturating_sub(line.width());
        line.spans.push(" ".repeat(pad).into());
        line = t
            .lift_faint(line)
            .style(style::Style::new().bg(t.selection));
    }
    line
}

fn typing_line(typing: &Typing, width: usize, t: &Theme) -> Line<'static> {
    // Kinds are words, not colors: the label is the accent, as any prompt's.
    let (label, color, text) = match typing {
        Typing::Add { kind, text } => (format!(" new {} ", kind.as_str()), t.accent, text),
        Typing::Note { number, text, .. } => (format!(" note on #{number} "), t.accent, text),
        Typing::Edit {
            number,
            kind,
            project,
            text,
            was,
            ..
        } => (
            match project.as_ref().filter(|p| Some(*p) != was.2.as_ref()) {
                Some(p) => format!(
                    " #{number} {} → {} ",
                    kind.as_str(),
                    p.file_name().map_or_else(
                        || p.display().to_string(),
                        |n| n.to_string_lossy().into_owned()
                    )
                ),
                None => format!(" #{number} {} ", kind.as_str()),
            },
            t.accent,
            text,
        ),
        Typing::Launch(form) => (format!(" #{} model ", form.number), t.accent, &form.model),
        Typing::Repo { text, .. } => (" add repo, path ".to_owned(), t.accent, text),
    };
    let room = width.saturating_sub(label.chars().count() + 1);
    let mut spans = vec![label.fg(color).bold(), " ".into()];
    spans.extend(text.spans(room, t));
    Line::from(spans)
}

/// The selected item in full: its state, body, facts and notes.
fn detail_lines(
    item: &Item,
    lane: Lane,
    live: Option<&SessionInfo>,
    facts: Option<&ItemFacts>,
    blocked: &[String],
    now: u64,
    t: &Theme,
) -> Vec<Line<'static>> {
    let mut out = vec![
        Line::from(vec![
            format!(" #{} ", item.number).fg(t.muted),
            item.title.clone().fg(t.fg).bold(),
        ]),
        Line::from(vec![
            format!(" {} ", lane.title()).fg(t.dim()).bold(),
            match (lane == Lane::Review && item.pr.is_some(), item.pr_closed) {
                (true, true) => "· PR closed ".fg(t.muted),
                (true, false) => "· PR open ".fg(t.muted),
                (false, _) => "".into(),
            },
            format!("· {} ", item.priority.as_str()).fg(t.muted),
            match live {
                Some(s) => format!(
                    "· tab {} {} ",
                    s.id,
                    crate::theme::state_word(s.status.state)
                )
                .fg(t.muted),
                None => "".into(),
            },
            item.branch
                .clone()
                .map_or("".into(), |b| format!("· {b}").fg(t.muted)),
        ]),
    ];
    if lane == Lane::Proposed {
        out.push(Line::from(
            format!(" by {}, waiting on you", item.by).fg(t.accent),
        ));
    } else if item.agent_filed() {
        out.push(Line::from(
            format!(" agent-filed by {}, not accepted yet: a", item.by).fg(t.accent),
        ));
    }
    if !item.blocked_by.is_empty() {
        out.push(Line::from(
            format!(" after {}, not finished yet", blocked.join(", ")).fg(t.muted),
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
        let origin = n.origin().map(|o| format!(" · {o}")).unwrap_or_default();
        out.push(Line::from(
            format!(" {by} · {ago}{origin}").fg(t.muted).bold(),
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
            parent: None,
            pr_merged: false,
            priority: Default::default(),
            blocked_by: Vec::new(),
            id,
            number: id,
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
            number: 2,
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
        // None offered: R says so.
        page.launches.clear();
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
    fn a_long_order_pages_ends_and_ticks_all_at_once() {
        let items: Vec<Item> = (1..=30).map(|n| item(n, 1)).collect();
        let rows = rows(&items, &[], 7);
        let mut page = Page::default();
        let press = |page: &mut Page, k: &[u8]| page.keys(k, &rows, &[], 7);
        page.orders = vec![proposal(
            (1..=30)
                .map(|n| step(n, (n % 2 == 0).then_some(Priority::P1), None))
                .collect(),
        )];
        press(&mut page, b"O");
        let cursor = |page: &Page| page.order.as_ref().unwrap().cursor;
        press(&mut page, b"\x1b[6~");
        assert_eq!(cursor(&page), ORDER_PAGE);
        press(&mut page, b"G");
        assert_eq!(cursor(&page), 29);
        press(&mut page, b"\x1b[6~");
        assert_eq!(cursor(&page), 29);
        press(&mut page, b"g");
        assert_eq!(cursor(&page), 0);
        // t unticks every change (the place-only steps have no tick), then ticks them back.
        press(&mut page, b"t");
        assert_eq!(page.order.as_ref().unwrap().skip.len(), 15);
        assert!(matches!(&press(&mut page, b"\r")[..], [Do::Say(s)] if s.contains("declines")));
        press(&mut page, b"O");
        press(&mut page, b"j t");
        assert!(page.order.as_ref().unwrap().skip.is_empty());
    }

    #[test]
    fn the_order_scrolls_to_show_the_cursors_step_whole() {
        // Fits: from the top.
        assert_eq!(order_scroll(3..5, 20), 0);
        // Below the window: its last row at the bottom.
        assert_eq!(order_scroll(30..34, 10), 24);
        // Taller than the window: its head first.
        assert_eq!(order_scroll(30..45, 10), 30);
    }

    #[test]
    fn a_long_reason_wraps_into_rows_the_scroll_can_count() {
        let rows = rows(&[item(1, 1), item(2, 1)], &[], 7);
        let mut order = proposal(vec![step(1, None, None), step(2, Some(Priority::P1), None)]);
        order.steps[0].reason = "word ".repeat(40);
        let view = OrderView {
            id: order.id,
            project: order.project.clone(),
            cursor: 1,
            skip: Vec::new(),
        };
        let (lines, at) = order_lines(&view, &order, &rows, 40, &crate::theme::VALKYRIE);
        assert!(lines.iter().all(|l| l.width() <= 40));
        // The first step's head and its wrapped reason come before the cursor's.
        assert!(at.start > 2);
        assert_eq!(at.end, lines.len());
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
            step_changes(&step(Some(Priority::P1), Some(vec![2, 5])), Some(&i), &[]),
            "p2→p1, after #2"
        );
        assert_eq!(step_changes(&step(None, None), Some(&i), &[]), "");
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
            report: None,
            supervisor: false,
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
        let text: Vec<String> = detail_lines(&filed, Lane::Running, None, None, &[], 0, &t)
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
        let text: Vec<String> = detail_lines(&pr(1), Lane::Review, None, None, &[], 0, &t)
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
    fn ctrl_r_in_an_edit_adds_a_repository_by_path() {
        let rows = rows(&[item(1, 1)], &[], 7);
        let mut page = Page::default();
        let press = |page: &mut Page, k: &[u8]| page.keys(k, &rows, &[], 7);
        assert!(press(&mut page, b"e\x12").is_empty());
        assert!(matches!(&page.typing, Some(Typing::Repo { .. })));
        // Esc goes back to the edit, not out of it.
        assert!(press(&mut page, b"\x1b").is_empty());
        assert!(matches!(&page.typing, Some(Typing::Edit { id: 1, .. })));
        // A pasted path, then Enter: added, with the edit to go back to.
        let got = press(&mut page, b"\x12\x1b[200~~/far/away/app\x1b[201~\r");
        let [Do::AddRepo { path, back }] = got.as_slice() else {
            panic!("expected an add, got {got:?}");
        };
        assert_eq!(path, "~/far/away/app");
        assert!(matches!(**back, Typing::Edit { id: 1, .. }));
        assert_eq!(
            page.typing, None,
            "the app puts the edit back once it's added"
        );
        // Nothing typed: straight back to the edit.
        assert!(press(&mut page, b"e\x12\r").is_empty());
        assert!(matches!(&page.typing, Some(Typing::Edit { id: 1, .. })));
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
            number: id,
            title: String::new(),
            branch: format!("fix/{id}"),
            branch_taken: id == 4,
            filed: id == 6,
            against: "origin/main".into(),
            note: None,
        };
        let kept = |id, why: &str| KeptItem {
            number: id,
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
            number: 1,
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
        let text: Vec<String> = detail_lines(&taken, Lane::Review, None, None, &[], 0, &t)
            .iter()
            .map(|l| l.to_string())
            .collect();
        assert!(text.contains(&format!(" pr: {pr}")), "{text:?}");
    }

    #[test]
    fn the_new_item_label_is_the_accent_whatever_its_kind() {
        let t = crate::theme::DRACULA;
        let label = |kind| {
            let typing = Typing::Add {
                kind,
                text: LineEdit::default(),
            };
            typing_line(&typing, 40, &t).spans[0].style.fg
        };
        assert_eq!(label(ItemKind::Bug), Some(t.accent));
        assert_eq!(label(ItemKind::Feature), Some(t.accent));
    }

    #[test]
    fn slash_narrows_by_title_number_kind_and_repository_and_r_t_are_gone() {
        let of = |id, kind, repo: Option<&str>, title: &str| Item {
            kind,
            title: title.into(),
            project: repo.map(PathBuf::from),
            ..item(id, 1)
        };
        let items = [
            of(1, ItemKind::Bug, Some("/w/api"), "Login drops the token"),
            of(2, ItemKind::Feature, Some("/w/api"), "Dark mode"),
            of(3, ItemKind::Bug, Some("/w/web"), "Toasts overlap"),
            of(4, ItemKind::Chore, None, "Tidy the scripts"),
        ];
        let mut page = Page {
            cursor: 2,
            ..Page::default()
        };
        let shown = |page: &Page| {
            items
                .iter()
                .filter(|i| page.holds(i))
                .map(|i| i.id)
                .collect::<Vec<_>>()
        };
        // r and t aren't filters any more: rename and nothing.
        assert!(page.keys(b"rt", &[], &[], 7).is_empty());
        assert_eq!(shown(&page), [1, 2, 3, 4]);
        assert!(page.keys(b"/api bug", &[], &[], 7).is_empty());
        assert!(
            page.search.typing && !page.at_list(),
            "[ ] and g type while typing"
        );
        assert_eq!(shown(&page), [1]);
        assert_eq!(page.cursor, 0, "a new search starts at the top");
        // ↩ keeps it; Esc on the list clears it before it closes the page.
        assert!(page.keys(b"\r", &[], &[], 7).is_empty());
        assert!(page.at_list());
        assert!(page.keys(b"\x1b", &[], &[], 7).is_empty());
        assert_eq!(shown(&page), [1, 2, 3, 4]);
        assert_eq!(page.keys(b"\x1b", &[], &[], 7), [Do::Close]);
        // ↩ then a key in the same read: nothing runs on the stale rows.
        let rows = rows(&items, &[], 7);
        assert!(page.keys(b"/login\rd", &rows, &[], 7).is_empty());
        page.keys(b"\x1b", &rows, &[], 7);
        // By number, and the folder-less by "no folder".
        page.keys(b"/#3\r", &[], &[], 7);
        assert_eq!(shown(&page), [3]);
        page.keys(b"\x1b", &[], &[], 7);
        page.keys(b"/no folder\r", &[], &[], 7);
        assert_eq!(shown(&page), [4]);
        // A new item starts as a bug, whatever is shown.
        page.keys(b"n", &[], &[], 7);
        assert!(matches!(
            page.typing,
            Some(Typing::Add {
                kind: ItemKind::Bug,
                ..
            })
        ));
    }

    #[test]
    fn brackets_step_through_the_filter_row_to_pull_requests_and_round() {
        let mut at = Some(Filter::All);
        let mut seen = Vec::new();
        for _ in 0..5 {
            at = Filter::step(at, true);
            seen.push(at);
        }
        assert_eq!(
            seen,
            [
                Some(Filter::Ready),
                Some(Filter::Running),
                Some(Filter::Review),
                None,
                Some(Filter::All)
            ]
        );
        assert_eq!(Filter::step(Some(Filter::All), false), None, "[ from All");
        assert_eq!(Filter::step(None, false), Some(Filter::Review));
    }

    #[test]
    fn the_filter_row_maps_onto_lanes() {
        let mut blocked = item(5, 1);
        blocked.blocked_by = vec![4];
        let free = item(6, 1);
        let shows = |f: Filter, lane| {
            [&free, &blocked]
                .iter()
                .filter(|i| f.shows(lane, i))
                .map(|i| i.id)
                .collect::<Vec<_>>()
        };
        for lane in Lane::ORDER {
            assert_eq!(shows(Filter::All, lane), [6, 5], "All shows {lane:?}");
        }
        assert_eq!(
            shows(Filter::Ready, Lane::Backlog),
            [6],
            "not one that waits"
        );
        assert!(shows(Filter::Ready, Lane::Proposed).is_empty());
        assert_eq!(shows(Filter::Running, Lane::Running).len(), 2);
        assert_eq!(shows(Filter::Running, Lane::Needs).len(), 2);
        assert!(shows(Filter::Running, Lane::Review).is_empty());
        assert_eq!(shows(Filter::Review, Lane::Review).len(), 2);
        for f in [Filter::Ready, Filter::Running, Filter::Review] {
            assert!(shows(f, Lane::Done).is_empty(), "{f:?} has no done");
            assert!(shows(f, Lane::Proposed).is_empty(), "{f:?} has no proposed");
        }
    }

    /// Backlog #235: a piece points at its group, the group counts its pieces.
    #[test]
    fn a_group_shows_its_progress_and_its_pieces_point_at_it() {
        let t = crate::theme::DRACULA;
        let numbered = |id, number| Item {
            number,
            ..item(id, 1)
        };
        let mut parent = numbered(10, 1);
        let mut one = numbered(11, 2);
        let mut two = numbered(12, 3);
        one.parent = Some(parent.id);
        two.parent = Some(parent.id);
        two.status = ItemStatus::Shipped;
        parent.title = "the plan".into();
        let all = vec![parent.clone(), one.clone(), two.clone()];
        assert_eq!(group_tag(&parent, &all), " 1/2");
        assert_eq!(
            group_tag(&one, &all),
            " ↳#1",
            "the parent's number, not its id"
        );
        assert_eq!(group_tag(&numbered(13, 4), &all), "");
        let text = |i: &Item| -> Vec<String> {
            group_lines(i, &all, &t)
                .iter()
                .map(|l| l.to_string())
                .collect()
        };
        assert_eq!(text(&one), [" piece of #1 the plan (open)"]);
        let lines = text(&parent);
        assert_eq!(lines[0], " pieces 1/2 done");
        assert!(
            lines[2].contains("#3") && lines[2].contains("shipped"),
            "{lines:?}"
        );
    }

    #[test]
    fn each_row_shows_its_state_and_still_fits_the_width() {
        let t = crate::theme::DRACULA;
        for lane in Lane::ORDER {
            for width in [40u16, 80] {
                let line = item_line(&item(1, 1), lane, " ↳#2", &[], true, width, &t);
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
        let after = tags(&rows[2].1, &rows[2].1.blocked_by, &all);
        let line: String = item_line(&rows[2].1, Lane::Backlog, "", &after, false, 90, &t)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(line.contains("p0") && line.contains("after #3"), "{line}");
        // Backlog #239: named by its number in the repository, not the global id;
        // one in another repository by that repository too.
        let mut here = item(20, 1);
        here.number = 2;
        let mut there = item(21, 1);
        there.number = 5;
        there.project = Some("/r/other".into());
        let mut waits = item(22, 1);
        waits.blocked_by = vec![20, 21, 99];
        let all = [here, there, waits.clone()];
        assert_eq!(
            tags(&waits, &waits.blocked_by, &all),
            ["#2", "other#5", "#99"]
        );

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

//! The backlog page (DESIGN §8.12): `b` on the home screen, or in tab mode over a
//! session. The same lanes as the web board, as one list, since four columns don't
//! fit a terminal; the selected item's detail beside or below it.

use crate::edit::LineEdit;
use crate::theme::Theme;
use crate::{fit, next_key};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{self, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use valkyrie_proto::{AgentState, Item, ItemFacts, ItemKind, ItemStatus, SessionId, SessionInfo};

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

/// Where an item stands, from its newest live session: as the web board works it out.
pub(crate) fn lane(item: &Item, sessions: &[SessionInfo], boot: u64) -> Lane {
    match item.status {
        ItemStatus::Proposed => return Lane::Proposed,
        ItemStatus::Shipped | ItemStatus::Dropped => return Lane::Done,
        ItemStatus::Open => {}
    }
    if let Some(s) = live(item, sessions, boot) {
        return match s.status.state {
            AgentState::NeedsInput | AgentState::Blocked => Lane::Needs,
            AgentState::Working | AgentState::Stale => Lane::Running,
            _ => Lane::Review,
        };
    }
    if item.checkout.is_some() || !item.sessions.is_empty() {
        Lane::Review
    } else {
        Lane::Backlog
    }
}

/// Items in page order, each with its lane: by lane, then newest first. Only the
/// newest few done ones.
pub(crate) fn rows(items: &[Item], sessions: &[SessionInfo], boot: u64) -> Vec<(Lane, Item)> {
    let mut out: Vec<(Lane, Item)> = items
        .iter()
        .map(|i| (lane(i, sessions, boot), i.clone()))
        .collect();
    let rank = |l: Lane| Lane::ORDER.iter().position(|&o| o == l).unwrap_or(0);
    out.sort_by(|(la, a), (lb, b)| {
        rank(*la)
            .cmp(&rank(*lb))
            .then(b.updated.cmp(&a.updated))
            .then(b.id.cmp(&a.id))
    });
    // Sorted newest first: the newest few done ones stay.
    let mut done = 0;
    out.retain(|(l, _)| {
        done += usize::from(*l == Lane::Done);
        *l != Lane::Done || done <= DONE_SHOWN
    });
    out
}

/// Done items shown at the bottom of the page.
const DONE_SHOWN: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Typing {
    /// A new item: Tab cycles its kind.
    Add { kind: ItemKind, text: LineEdit },
    /// A handoff note on item `id`.
    Note { id: u32, text: LineEdit },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Page {
    pub(crate) cursor: usize,
    pub(crate) typing: Option<Typing>,
    /// `x` was pressed once on this item: deleting has no undo, so it takes two.
    pub(crate) armed: Option<u32>,
    /// The selected item's facts, for `(id, updated)`, read when the cursor lands.
    pub(crate) facts: Option<(u32, u64, Option<ItemFacts>)>,
    /// The start of a character a read cut off, while typing.
    partial: Vec<u8>,
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
    Launch { id: u32, agent: &'static str },
    Attach(SessionId),
    Status(u32, ItemStatus),
    Delete(u32),
    Add { kind: ItemKind, text: String },
    Note { id: u32, text: String },
    Say(String),
}

impl Page {
    /// Handles `keys` against `rows` (from `rows`), returning what to do, in order.
    pub(crate) fn keys(
        &mut self,
        mut keys: &[u8],
        rows: &[(Lane, Item)],
        sessions: &[SessionInfo],
        boot: u64,
    ) -> Vec<Do> {
        let mut out = Vec::new();
        let mut typed = Vec::new();
        while !keys.is_empty() {
            let key = next_key(keys);
            let whole = key.len() == keys.len();
            keys = &keys[key.len()..];
            if let Some(typing) = &mut self.typing {
                let text = match typing {
                    Typing::Add { text, .. } | Typing::Note { text, .. } => text,
                };
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
                            _ => {}
                        }
                    }
                    b"\t" => {
                        if let Typing::Add { kind, .. } = typing {
                            let at = ItemKind::ALL.iter().position(|k| k == kind).unwrap_or(0);
                            *kind = ItemKind::ALL[(at + 1) % ItemKind::ALL.len()];
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
                }
                continue;
            }
            let here = rows.get(self.cursor).map(|(l, i)| (*l, i));
            if key != b"x" {
                self.armed = None;
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
        if let Some(Typing::Add { text, .. } | Typing::Note { text, .. }) = &mut self.typing {
            text.insert(&decode(&mut self.partial, &typed));
        }
        out
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
        let launchable = item.status == ItemStatus::Open && live.is_none();
        let act = match key {
            b"\r" | b"\n" => match live {
                Some(s) => Do::Attach(s),
                None if lane == Lane::Proposed => Do::Say("accept it first: a".into()),
                None if !launchable => {
                    Do::Say(format!("#{id} is {}: o reopens it", item.status.as_str()))
                }
                None if item.project.is_none() => {
                    Do::Say(format!("#{id} has no folder yet: pick one in the web app"))
                }
                None => Do::Launch {
                    id,
                    agent: "claude",
                },
            },
            b"c" if launchable && item.project.is_some() => Do::Launch { id, agent: "codex" },
            b"a" if lane == Lane::Proposed => Do::Status(id, ItemStatus::Open),
            b"s" if item.status == ItemStatus::Open => Do::Status(id, ItemStatus::Shipped),
            b"d" if item.status != ItemStatus::Dropped => Do::Status(id, ItemStatus::Dropped),
            b"o" if lane == Lane::Done => Do::Status(id, ItemStatus::Open),
            b"N" => {
                self.typing = Some(Typing::Note {
                    id,
                    text: LineEdit::default(),
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
            (Some(Typing::Note { .. }), _) => {
                [hint(" ↩ ", "note "), hint(" esc ", "cancel ")].concat()
            }
            (None, Some((Lane::Proposed, _))) => [
                hint(" a ", "accept "),
                hint(" x ", "delete "),
                hint(" n ", "new "),
                hint(" esc ", "close "),
            ]
            .concat(),
            (None, Some((Lane::Done, _))) => [
                hint(" o ", "reopen "),
                hint(" x ", "delete "),
                hint(" n ", "new "),
                hint(" esc ", "close "),
            ]
            .concat(),
            (None, Some((lane, item))) => {
                let open = if live(item, sessions, boot).is_some() {
                    hint(" ↩ ", "open ")
                } else if *lane == Lane::Review {
                    hint(" ↩ ", "relaunch ")
                } else {
                    hint(" ↩ ", "launch ")
                };
                [
                    open,
                    hint(" s ", "ship "),
                    hint(" N ", "note "),
                    hint(" d ", "drop "),
                    hint(" n ", "new "),
                    hint(" esc ", "close "),
                ]
                .concat()
            }
            (None, None) => [hint(" n ", "new "), hint(" esc ", "close ")].concat(),
        };
        let open = rows.iter().filter(|(l, _)| *l != Lane::Done).count();
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style::Style::new().fg(t.accent))
            .title(Line::from(vec![
                " Backlog ".fg(t.fg).bold(),
                format!("{open} open ").fg(t.muted),
            ]))
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
        if let Some(typing) = &self.typing {
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
    }
}

fn kind_color(kind: ItemKind, t: &Theme) -> style::Color {
    match kind {
        ItemKind::Bug => t.blocked,
        ItemKind::Feature => t.accent,
        ItemKind::Chore => t.muted,
    }
}

fn item_line(item: &Item, lane: Lane, selected: bool, width: u16, t: &Theme) -> Line<'static> {
    let folder = item
        .project
        .as_ref()
        .and_then(|p| p.file_name())
        .map_or("no folder".into(), |n| n.to_string_lossy().into_owned());
    let head = format!(" #{:<4}", item.id);
    let kind = format!("{:<8}", item.kind.as_str());
    let folder = format!("  {}", fit(&folder, 16));
    let room =
        (width as usize).saturating_sub(head.len() + kind.len() + folder.chars().count() + 1);
    let title_fg = if lane == Lane::Done { t.muted } else { t.fg };
    let mut line = Line::from(vec![
        head.fg(t.muted),
        kind.fg(kind_color(item.kind, t)),
        // Padded, so the folders line up at the right.
        format!("{:<room$}", fit(&item.title, room)).fg(title_fg),
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
        out.push(Line::from(format!(" {by} · {ago}").fg(t.muted).bold()));
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
            branch: None,
            checkout: None,
            notes: Vec::new(),
            created: 0,
            updated,
        }
    }

    fn session(id: SessionId, state: AgentState) -> SessionInfo {
        SessionInfo {
            id,
            name: String::new(),
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
                agent: "claude"
            }]
        );
        assert_eq!(
            press(&mut page, b"c"),
            [Do::Launch {
                id: 1,
                agent: "codex"
            }]
        );
        assert_eq!(press(&mut page, b"s"), [Do::Status(1, ItemStatus::Shipped)]);
        assert_eq!(press(&mut page, b"\x1b"), [Do::Close]);
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
}

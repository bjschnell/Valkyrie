//! Memory's own tabs (TUI redesign T4b, backlog #269): Decisions, Skills and
//! Notes, beside Docs and Wiki (`g m`, then `[ ]`). Decisions has what's up for
//! review on top, `a`/`d` answering it, then every accepted one grouped by kind;
//! Skills the accepted procedures; Notes what agents noted. A selected row's
//! text shows beside the list on a wide screen; `↩` reads it in full.

use crate::backlog::fit_hints;
use crate::markdown;
use crate::next_key;
use crate::theme::Theme;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{self, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use std::path::Path;
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus, Note, NoteStatus};

/// The user scope's root: habits that hold in every project.
const USER_SCOPE: &str = "~user";
/// Columns from which the selected row's text shows beside the list.
const DETAIL_MIN_WIDTH: u16 = 120;

/// Memory's tabs that this page draws; Docs and Wiki are pages of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Decisions,
    Skills,
    Notes,
}

/// What a key on the page asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Do {
    Close,
    Load,
    /// Accept (or reject) the proposal at this index of the pushed proposals.
    Review {
        index: usize,
        accept: bool,
    },
}

/// A row of a tab: a group's label, or one thing in it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Head(String, usize),
    /// A proposal, by its index among the pushed proposals.
    Review(usize),
    Decision(usize),
    Note(usize),
}

#[derive(Debug)]
pub(crate) struct Page {
    pub(crate) tab: Tab,
    decisions: Option<Vec<Decision>>,
    notes: Option<Vec<Note>>,
    failed: Option<String>,
    /// Index among the selectable rows (not the labels).
    cursor: usize,
    /// Reading the selected row in full, scrolled this far.
    reading: Option<usize>,
}

impl Page {
    pub(crate) fn new(tab: Tab) -> Self {
        Page {
            tab,
            decisions: None,
            notes: None,
            failed: None,
            cursor: 0,
            reading: None,
        }
    }

    /// On the list, nothing read in full: where section keys are the app's.
    pub(crate) fn at_list(&self) -> bool {
        self.reading.is_none()
    }

    /// What the daemon said: every project's decisions and notes, or why not.
    pub(crate) fn loaded(&mut self, got: Result<(Vec<Decision>, Vec<Note>), String>) {
        match got {
            Ok((decisions, notes)) => {
                self.decisions = Some(decisions);
                self.notes = Some(notes);
                self.failed = None;
            }
            Err(e) => self.failed = Some(e),
        }
    }

    /// How many each tab holds, for the strip: `Decisions 24 · Skills 4`.
    pub(crate) fn count(&self, tab: Tab, proposals: &[Decision]) -> Option<usize> {
        self.decisions.as_ref()?;
        Some(
            self.rows(tab, proposals)
                .iter()
                .filter(|r| !matches!(r, Row::Head(..)))
                .count(),
        )
    }

    fn accepted(&self) -> impl Iterator<Item = (usize, &Decision)> {
        self.decisions
            .iter()
            .flatten()
            .enumerate()
            .filter(|(_, d)| d.status == DecisionStatus::Active)
    }

    /// A tab's rows in the order drawn. Decisions: To review, then the accepted
    /// by kind, your habits last. Skills and Notes: by repository.
    fn rows(&self, tab: Tab, proposals: &[Decision]) -> Vec<Row> {
        let mut rows = Vec::new();
        match tab {
            Tab::Decisions => {
                if !proposals.is_empty() {
                    rows.push(Row::Head("To review".into(), proposals.len()));
                    rows.extend((0..proposals.len()).map(Row::Review));
                }
                // Each kind's group, your habits (the user scope, any kind) last.
                let groups = [
                    ("Constraints", Some(DecisionKind::Constraint)),
                    ("Gotchas", Some(DecisionKind::Gotcha)),
                    ("Decisions", Some(DecisionKind::Decision)),
                    ("Patterns", Some(DecisionKind::Pattern)),
                    ("Fixes", Some(DecisionKind::Fix)),
                    ("Policies", Some(DecisionKind::Policy)),
                    ("Your habits", None),
                ];
                for (label, kind) in groups {
                    let these: Vec<usize> = self
                        .accepted()
                        .filter(|(_, d)| {
                            let habit = d.project.as_os_str() == USER_SCOPE;
                            d.kind != DecisionKind::Skill
                                && match kind {
                                    Some(kind) => !habit && d.kind == kind,
                                    None => habit,
                                }
                        })
                        .map(|(i, _)| i)
                        .collect();
                    if these.is_empty() {
                        continue;
                    }
                    rows.push(Row::Head(label.into(), these.len()));
                    rows.extend(these.into_iter().map(Row::Decision));
                }
            }
            Tab::Skills => {
                let skills: Vec<(usize, &Path)> = self
                    .accepted()
                    .filter(|(_, d)| d.kind == DecisionKind::Skill)
                    .map(|(i, d)| (i, d.project.as_path()))
                    .collect();
                by_repo(&mut rows, &skills, Row::Decision);
            }
            Tab::Notes => {
                let notes: Vec<(usize, &Path)> = self
                    .notes
                    .iter()
                    .flatten()
                    .enumerate()
                    .filter(|(_, n)| n.status == NoteStatus::Live)
                    .map(|(i, n)| (i, n.project.as_path()))
                    .collect();
                by_repo(&mut rows, &notes, Row::Note);
            }
        }
        rows
    }

    fn picks(&self, proposals: &[Decision]) -> Vec<Row> {
        self.rows(self.tab, proposals)
            .into_iter()
            .filter(|r| !matches!(r, Row::Head(..)))
            .collect()
    }

    /// Keys while the page is open. List: j/k pick, ↩ reads in full, `a`/`d`
    /// accept or reject a proposal, `r` reloads, Esc or `q` closes. Reading:
    /// j/k and PgUp/PgDn scroll, Esc back to the list.
    pub(crate) fn keys(&mut self, mut bytes: &[u8], proposals: &[Decision]) -> Vec<Do> {
        let mut out = Vec::new();
        let picks = self.picks(proposals);
        let n = picks.len();
        self.cursor = self.cursor.min(n.saturating_sub(1));
        while !bytes.is_empty() {
            let key = next_key(bytes);
            let whole = key.len() == bytes.len();
            bytes = &bytes[key.len()..];
            if let Some(scroll) = &mut self.reading {
                match key {
                    b"\x1b" if whole => self.reading = None,
                    b"q" => self.reading = None,
                    [0x03] => out.push(Do::Close),
                    b"k" | b"\x1b[A" | b"\x1bOA" => *scroll = scroll.saturating_sub(1),
                    b"j" | b"\x1b[B" | b"\x1bOB" => *scroll += 1,
                    b"\x1b[5~" => *scroll = scroll.saturating_sub(20),
                    b"\x1b[6~" | b" " => *scroll += 20,
                    b"g" | b"\x1b[H" | b"\x1b[1~" => *scroll = 0,
                    _ => {}
                }
                continue;
            }
            match key {
                b"\x1b" if whole => out.push(Do::Close),
                b"q" | [0x03] => out.push(Do::Close),
                b"k" | b"\x1b[A" | b"\x1bOA" => self.cursor = self.cursor.saturating_sub(1),
                b"j" | b"\x1b[B" | b"\x1bOB" => {
                    self.cursor = (self.cursor + 1).min(n.saturating_sub(1))
                }
                b"g" | b"\x1b[H" | b"\x1b[1~" => self.cursor = 0,
                b"G" | b"\x1b[F" | b"\x1b[4~" => self.cursor = n.saturating_sub(1),
                b"\r" | b"\n" if n > 0 => self.reading = Some(0),
                b"a" | b"d" => {
                    if let Some(Row::Review(index)) = picks.get(self.cursor) {
                        out.push(Do::Review {
                            index: *index,
                            accept: key == b"a",
                        });
                    }
                }
                b"r" => out.push(Do::Load),
                _ => {}
            }
        }
        out
    }

    /// The selected row's text: title, what it is and where it came from, then
    /// the body as markdown.
    fn detail(
        &self,
        row: &Row,
        proposals: &[Decision],
        width: u16,
        t: &Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let decision = match *row {
            Row::Review(i) => proposals.get(i),
            Row::Decision(i) => self.decisions.as_ref().and_then(|d| d.get(i)),
            _ => None,
        };
        if let Some(d) = decision {
            lines.push(Line::from(d.title.clone().fg(t.fg).bold()));
            let mut about = format!("{} · {}", d.kind.as_str(), repo_name(&d.project));
            if !d.scope.is_empty() {
                about.push_str(&format!(" · applies to {}", d.scope.join(", ")));
            }
            lines.push(Line::from(about.fg(t.muted)));
            let from = match &d.provenance.session {
                Some(session) => format!("came up in “{session}”, by {}", d.provenance.by),
                None => format!("by {}", d.provenance.by),
            };
            lines.push(Line::from(from.fg(t.muted)));
            if matches!(row, Row::Review(_)) {
                lines.push(Line::from(vec![
                    "a".fg(t.accent).bold(),
                    " accept · ".fg(t.dim()),
                    "d".fg(t.accent).bold(),
                    " reject".fg(t.dim()),
                ]));
            }
            lines.push(Line::default());
            lines.extend(markdown::render(&d.body, width, t).lines);
        } else if let Row::Note(i) = *row
            && let Some(note) = self.notes.as_ref().and_then(|n| n.get(i))
        {
            // `none` when it wasn't written in a Valkyrie session.
            let session = match note.session.as_str() {
                "none" | "" => String::new(),
                s => format!(" in {s}"),
            };
            lines.push(Line::from(
                format!("{} · by {}{session}", repo_name(&note.project), note.by).fg(t.muted),
            ));
            lines.push(Line::default());
            lines.extend(markdown::render(&note.text, width, t).lines);
        }
        lines
    }

    /// A row as listed: a marker, its title, then its kind and repository as faint
    /// words.
    fn line(&self, row: &Row, proposals: &[Decision], width: usize, t: &Theme) -> Line<'static> {
        let fit = crate::fit;
        let (mark, title, words) = match *row {
            Row::Head(ref label, n) => {
                return Line::from(vec![
                    format!(" {label} ").fg(t.dim()).bold(),
                    n.to_string().fg(t.dim()),
                ]);
            }
            Row::Review(i) => {
                let d = &proposals[i];
                (
                    "■".fg(t.accent),
                    d.title.clone(),
                    format!("{} · {}", d.kind.as_str(), repo_name(&d.project)),
                )
            }
            Row::Decision(i) => {
                let d = &self.decisions.as_ref().expect("listed")[i];
                let words = match self.tab {
                    Tab::Decisions => format!("{} · {}", d.kind.as_str(), repo_name(&d.project)),
                    _ => repo_name(&d.project),
                };
                (" ".into(), d.title.clone(), words)
            }
            Row::Note(i) => {
                let n = &self.notes.as_ref().expect("listed")[i];
                let first = n.text.lines().next().unwrap_or_default().to_string();
                (" ".into(), first, repo_name(&n.project))
            }
        };
        let room = width.saturating_sub(words.chars().count() + 6);
        let title = fit(&title, room);
        let pad = room.saturating_sub(unicode_width::UnicodeWidthStr::width(title.as_str()));
        Line::from(vec![
            " ".into(),
            mark,
            " ".into(),
            title.fg(t.fg),
            " ".repeat(pad + 2).into(),
            words.fg(t.muted),
        ])
    }

    pub(crate) fn draw(&self, frame: &mut ratatui::Frame, proposals: &[Decision], t: &Theme) {
        let area = frame.area();
        let rect = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        )
        .intersection(area);
        frame.render_widget(Clear, rect);
        let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
        let picks = self.picks(proposals);
        let cursor = self.cursor.min(picks.len().saturating_sub(1));
        let here = picks.get(cursor).cloned();
        let on_review = matches!(here, Some(Row::Review(_)));
        let hints: Vec<Span> = if self.reading.is_some() {
            [hint(" ↑↓ ", "scroll "), hint(" esc ", "back ")].concat()
        } else if on_review {
            [
                hint(" a ", "accept "),
                hint(" d ", "reject "),
                hint(" ↩ ", "read "),
                hint(" esc ", "close "),
            ]
            .concat()
        } else {
            [
                hint(" ↩ ", "read "),
                hint(" r ", "reload "),
                hint(" esc ", "close "),
            ]
            .concat()
        };
        let title = match self.tab {
            Tab::Decisions => " Decisions ",
            Tab::Skills => " Skills ",
            Tab::Notes => " Notes ",
        };
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style::Style::new().fg(t.accent))
            .title(title.fg(t.fg).bold())
            .title_bottom(
                Line::from(fit_hints(hints, rect.width.saturating_sub(4) as usize)).centered(),
            )
            .style(style::Style::new().bg(t.panel).fg(t.fg));
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        if let (Some(scroll), Some(row)) = (self.reading, &here) {
            let lines = self.detail(row, proposals, inner.width.saturating_sub(2), t);
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .scroll((scroll as u16, 0))
                    .block(Block::new().padding(ratatui::widgets::Padding::horizontal(1))),
                inner,
            );
            return;
        }

        let (list_area, detail_area) = if inner.width >= DETAIL_MIN_WIDTH {
            let [list, rule, detail] = Layout::horizontal([
                Constraint::Percentage(55),
                Constraint::Length(1),
                Constraint::Fill(1),
            ])
            .areas(inner);
            let bar = vec![Line::from("│".fg(t.border)); rule.height as usize];
            frame.render_widget(Paragraph::new(bar), rule);
            (list, Some(detail))
        } else {
            (inner, None)
        };

        let mut lines: Vec<Line> = vec![Line::default()];
        match (&self.decisions, &self.failed) {
            (_, Some(e)) => lines.push(
                format!(" Couldn't load memory: {e} · r tries again")
                    .fg(t.blocked)
                    .into(),
            ),
            (None, None) => lines.push(" Loading…".fg(t.muted).into()),
            _ if picks.is_empty() => lines.push(
                match self.tab {
                    Tab::Decisions => " Nothing decided yet: agents propose, you review.",
                    Tab::Skills => " No skills yet: agents propose procedures worth repeating.",
                    Tab::Notes => " No notes yet: agents leave them as they work.",
                }
                .fg(t.muted)
                .into(),
            ),
            _ => {}
        }
        let width = list_area.width as usize;
        let mut at = 0;
        let mut seen = 0;
        for row in self.rows(self.tab, proposals) {
            if matches!(row, Row::Head(..)) {
                if lines.len() > 1 {
                    lines.push(Line::default());
                }
                lines.push(self.line(&row, proposals, width, t));
                continue;
            }
            let mut line = self.line(&row, proposals, width, t);
            if seen == cursor {
                at = lines.len();
                line.spans[0] = "▌".fg(t.accent);
                let pad = width.saturating_sub(line.width());
                line.spans.push(" ".repeat(pad).into());
                line = t
                    .lift_faint(line)
                    .style(style::Style::new().bg(t.selection));
            }
            seen += 1;
            lines.push(line);
        }
        let room = list_area.height as usize;
        let skip = (at + 1).saturating_sub(room);
        frame.render_widget(Paragraph::new(lines).scroll((skip as u16, 0)), list_area);

        if let (Some(area), Some(row)) = (detail_area, &here) {
            let lines = self.detail(row, proposals, area.width.saturating_sub(2), t);
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .block(Block::new().padding(ratatui::widgets::Padding::new(1, 1, 1, 0))),
                area,
            );
        }
    }
}

/// `rows` grouped under each repository's name, in the order they first come.
fn by_repo(rows: &mut Vec<Row>, items: &[(usize, &Path)], row: fn(usize) -> Row) {
    let mut repos: Vec<&Path> = Vec::new();
    for (_, p) in items {
        if !repos.contains(p) {
            repos.push(p);
        }
    }
    for repo in repos {
        let these: Vec<usize> = items
            .iter()
            .filter(|(_, p)| *p == repo)
            .map(|(i, _)| *i)
            .collect();
        rows.push(Row::Head(repo_name(repo), these.len()));
        rows.extend(these.into_iter().map(row));
    }
}

/// `api` for `~/repos/api`, "your habits" for the user scope.
fn repo_name(project: &Path) -> String {
    if project.as_os_str() == USER_SCOPE {
        return "your habits".into();
    }
    project.file_name().map_or_else(
        || project.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(id: u32, project: &str, kind: DecisionKind, status: DecisionStatus) -> Decision {
        serde_json::from_value(serde_json::json!({
            "id": id, "project": project, "title": format!("d{id}"), "body": format!("body {id}"),
            "kind": kind.as_str(), "status": match status {
                DecisionStatus::Active => "active",
                DecisionStatus::Proposed => "proposed",
                _ => "rejected",
            },
            "created": 0, "updated": 0,
            "provenance": {"by": "claude", "session": "fix login"},
        }))
        .unwrap()
    }

    #[test]
    fn decisions_put_review_on_top_then_group_accepted_by_kind() {
        let mut page = Page::new(Tab::Decisions);
        page.loaded(Ok((
            vec![
                decision(1, "/r/api", DecisionKind::Gotcha, DecisionStatus::Active),
                decision(
                    2,
                    "/r/api",
                    DecisionKind::Constraint,
                    DecisionStatus::Active,
                ),
                decision(3, "/r/web", DecisionKind::Skill, DecisionStatus::Active),
                decision(4, "~user", DecisionKind::Decision, DecisionStatus::Active),
                decision(5, "/r/api", DecisionKind::Gotcha, DecisionStatus::Rejected),
            ],
            Vec::new(),
        )));
        let proposals = vec![decision(
            9,
            "/r/api",
            DecisionKind::Fix,
            DecisionStatus::Proposed,
        )];
        assert_eq!(
            page.rows(Tab::Decisions, &proposals),
            [
                Row::Head("To review".into(), 1),
                Row::Review(0),
                Row::Head("Constraints".into(), 1),
                Row::Decision(1),
                Row::Head("Gotchas".into(), 1),
                Row::Decision(0),
                Row::Head("Your habits".into(), 1),
                Row::Decision(3),
            ]
        );
        assert_eq!(page.rows(Tab::Skills, &proposals)[1], Row::Decision(2));
        assert_eq!(page.count(Tab::Decisions, &proposals), Some(4));
        assert_eq!(page.count(Tab::Skills, &proposals), Some(1));

        // a/d answer the proposal under the cursor, and only that.
        assert_eq!(
            page.keys(b"a", &proposals),
            [Do::Review {
                index: 0,
                accept: true
            }]
        );
        assert!(page.keys(b"jd", &proposals).is_empty());
        // ↩ reads in full; Esc comes back, then closes.
        page.keys(b"\r", &proposals);
        assert!(!page.at_list());
        page.keys(b"\x1b", &proposals);
        assert!(page.at_list());
        assert_eq!(page.keys(b"\x1b", &proposals), [Do::Close]);
    }

    #[test]
    fn the_page_draws_its_groups_without_boxes_inside() {
        let mut page = Page::new(Tab::Decisions);
        page.loaded(Ok((
            vec![decision(
                1,
                "/r/api",
                DecisionKind::Gotcha,
                DecisionStatus::Active,
            )],
            Vec::new(),
        )));
        let proposals = vec![decision(
            9,
            "/r/api",
            DecisionKind::Fix,
            DecisionStatus::Proposed,
        )];
        for width in [80, 140] {
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 20)).unwrap();
            term.draw(|f| page.draw(f, &proposals, &crate::theme::DRACULA))
                .unwrap();
            let screen: String = term
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            for want in ["To review 1", "■ d9", "Gotchas 1", "d1", "a accept"] {
                assert!(screen.contains(want), "{width}: lacks {want:?}");
            }
            // Wide, the selected one's text beside the list.
            assert_eq!(screen.contains("body 9"), width >= 140, "{width}");
        }
    }
}

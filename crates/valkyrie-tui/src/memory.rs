//! Memory's own tabs (TUI redesign T4b, backlog #269): Decisions, Skills and
//! Notes, beside Docs and Wiki (`g m`, then `[ ]`). Decisions has what's up for
//! review on top, `a`/`d` answering it (`e` rewrites it in your editor, then
//! accepts; `z` takes the last answer back), then every accepted one grouped by kind;
//! Skills the accepted procedures; Notes what agents noted. A selected row's
//! text shows beside the list on a wide screen; `↩` reads it in full.

use crate::markdown;
use crate::next_key;
use crate::search::{Search, Searched};
use crate::theme::Theme;
use ratatui::style::{self, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use std::path::{Path, PathBuf};
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus, Note, NoteStatus};

/// The user scope's root: habits that hold in every project.
const USER_SCOPE: &str = "~user";

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
    /// Accept (or reject) the proposal at this index of the pushed proposals.
    Review {
        index: usize,
        accept: bool,
    },
    /// Open the proposal at this index in your editor, to accept what you write.
    Edit {
        index: usize,
    },
    /// Take back the last answer given here (`Page::undo`).
    Undo,
}

/// What `z` takes back: the last answer given on this page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Undo {
    pub(crate) project: PathBuf,
    pub(crate) id: u32,
    pub(crate) title: String,
    /// Its `updated` once answered: the undo is refused if it changed since.
    pub(crate) seen: u64,
    pub(crate) how: Reverse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reverse {
    /// An accept or a reject: proposed again, with this wording when you
    /// rewrote it on the way in (`e`). A retire: in use again.
    Reopen(Option<Wording>),
}

/// A decision's words, as `e` edits them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Wording {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) kind: DecisionKind,
    /// `None` for a habit, which has no scope.
    pub(crate) scope: Option<Vec<String>>,
}

impl Wording {
    pub(crate) fn of(d: &Decision) -> Self {
        Self {
            title: d.title.clone(),
            body: d.body.clone(),
            kind: d.kind,
            scope: (!is_habit(d)).then(|| d.scope.clone()),
        }
    }

    /// The file your editor opens: its kind and where it applies over a `---`,
    /// then the title as a heading and the body.
    pub(crate) fn text(&self) -> String {
        let mut out = format!("kind: {}\n", self.kind.as_str());
        if let Some(scope) = &self.scope {
            out.push_str(&format!("scope: {}\n", scope.join(" ")));
        }
        out.push_str(&format!("---\n# {}\n\n{}", self.title, self.body));
        if !self.body.ends_with('\n') {
            out.push('\n');
        }
        out
    }

    /// Reads back what `text` wrote, as edited. A habit's scope stays `None`
    /// whatever the file says; dropping the `scope:` line leaves it as it was.
    pub(crate) fn parse(text: &str, habit: bool) -> Result<Self, String> {
        let kinds = || {
            DecisionKind::ALL
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        // An editor that saves CRLF lines.
        let text = &text.replace("\r\n", "\n");
        let (head, rest) = text
            .split_once("\n---\n")
            .or_else(|| text.strip_prefix("---\n").map(|r| ("", r)))
            .ok_or("no `---` line between the kind and the title")?;
        let mut kind = None;
        let mut scope = None;
        for line in head.lines().map(str::trim).filter(|l| !l.is_empty()) {
            match line.split_once(':') {
                Some(("kind", v)) => {
                    let v = v.trim();
                    kind = Some(
                        DecisionKind::parse(v)
                            .ok_or_else(|| format!("no kind `{v}`: it's one of {}", kinds()))?,
                    );
                }
                Some(("scope", v)) => {
                    scope = Some(
                        v.split([' ', ','])
                            .filter(|g| !g.is_empty())
                            .map(String::from)
                            .collect(),
                    );
                }
                _ => return Err(format!("`{line}` isn't `kind:` or `scope:`")),
            }
        }
        let kind = kind.ok_or_else(|| format!("no `kind:` line: it's one of {}", kinds()))?;
        let rest = rest.trim_start_matches(['\n', '\r']);
        let (first, body) = rest.split_once('\n').unwrap_or((rest, ""));
        let title = first
            .strip_prefix('#')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or("no title: the line after `---` is `# ` and the title")?;
        Ok(Self {
            title: title.to_string(),
            body: body.trim().to_string(),
            kind,
            scope: if habit { None } else { scope },
        })
    }
}

/// A habit: it holds in every project, so it has no scope.
pub(crate) fn is_habit(d: &Decision) -> bool {
    d.project.as_os_str() == USER_SCOPE
}

/// What the cursor is on, by identity, so it stays on the same thing when a
/// push reorders the list (else `a` could accept another proposal).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Anchor {
    Review(std::path::PathBuf, u32),
    Decision(std::path::PathBuf, u32),
    Note(std::path::PathBuf, u32),
}

/// How far reading may scroll; drawing keeps it to the text's length.
const SCROLL_MAX: usize = 10_000;

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
    /// Index among the selectable rows (not the labels); `anchor` wins when
    /// the thing it names is still listed.
    cursor: usize,
    anchor: Option<Anchor>,
    /// Repositories whose notes couldn't be read.
    pub(crate) notes_failed: usize,
    /// Reading the selected row in full, scrolled this far.
    reading: Option<usize>,
    /// The last answer given here, which `z` takes back.
    pub(crate) undo: Option<Undo>,
    /// `/`: rows whose words have every word typed.
    pub(crate) search: Search,
}

impl Page {
    pub(crate) fn new(tab: Tab) -> Self {
        Page {
            tab,
            decisions: None,
            notes: None,
            failed: None,
            cursor: 0,
            anchor: None,
            notes_failed: 0,
            reading: None,
            undo: None,
            search: Search::default(),
        }
    }

    /// On the list, nothing read in full: where section keys are the app's.
    pub(crate) fn at_list(&self) -> bool {
        self.reading.is_none() && !self.search.typing
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

    /// A tab's rows in the order drawn, as far as `/` leaves them: each group's
    /// label counts what's left in it, and one with nothing left goes.
    fn rows(&self, tab: Tab, proposals: &[Decision]) -> Vec<Row> {
        let rows = self.all_rows(tab, proposals);
        if self.search.query.trim().is_empty() {
            return rows;
        }
        let mut out = Vec::new();
        let mut head: Option<(String, usize)> = None;
        for row in rows {
            match row {
                Row::Head(label, _) => {
                    Self::close_group(&mut out, head.take());
                    head = Some((label, out.len()));
                }
                row if self.search.matches(&self.said(&row, proposals)) => out.push(row),
                _ => {}
            }
        }
        Self::close_group(&mut out, head);
        out
    }

    /// Puts a group's label in front of what it kept, counted, or nothing.
    fn close_group(out: &mut Vec<Row>, head: Option<(String, usize)>) {
        if let Some((label, from)) = head {
            let n = out.len() - from;
            if n > 0 {
                out.insert(from, Row::Head(label, n));
            }
        }
    }

    /// What `/` matches a row by: its title (a note's text), kind and repository.
    fn said(&self, row: &Row, proposals: &[Decision]) -> String {
        let decision = match *row {
            Row::Review(i) => proposals.get(i),
            Row::Decision(i) => self.decisions.as_ref().and_then(|d| d.get(i)),
            _ => None,
        };
        if let Some(d) = decision {
            return format!("{} {} {}", d.title, d.kind.as_str(), repo_name(&d.project));
        }
        match (*row).clone() {
            Row::Note(i) => self
                .notes
                .as_ref()
                .and_then(|n| n.get(i))
                .map(|n| format!("{} {}", n.text, repo_name(&n.project)))
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// A tab's rows in the order drawn. Decisions: To review, then the accepted
    /// by kind, your habits last. Skills and Notes: by repository.
    fn all_rows(&self, tab: Tab, proposals: &[Decision]) -> Vec<Row> {
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

    /// Every project's decisions again, after a review: the notes stay.
    pub(crate) fn decisions_loaded(&mut self, got: Result<Vec<Decision>, String>) {
        match got {
            Ok(decisions) => {
                self.decisions = Some(decisions);
                self.failed = None;
            }
            Err(e) => self.failed = Some(e),
        }
    }

    fn anchor_of(&self, row: &Row, proposals: &[Decision]) -> Option<Anchor> {
        Some(match *row {
            Row::Review(i) => {
                let d = proposals.get(i)?;
                Anchor::Review(d.project.clone(), d.id)
            }
            Row::Decision(i) => {
                let d = self.decisions.as_ref()?.get(i)?;
                Anchor::Decision(d.project.clone(), d.id)
            }
            Row::Note(i) => {
                let n = self.notes.as_ref()?.get(i)?;
                Anchor::Note(n.project.clone(), n.id)
            }
            Row::Head(..) => return None,
        })
    }

    /// The cursor's place among `picks`: where its anchor is now, else where it
    /// was (the next row, once a reviewed proposal leaves).
    fn place(&self, picks: &[Row], proposals: &[Decision]) -> usize {
        self.anchor
            .as_ref()
            .and_then(|a| {
                picks
                    .iter()
                    .position(|r| self.anchor_of(r, proposals).as_ref() == Some(a))
            })
            .unwrap_or(self.cursor)
            .min(picks.len().saturating_sub(1))
    }

    fn picks(&self, proposals: &[Decision]) -> Vec<Row> {
        self.rows(self.tab, proposals)
            .into_iter()
            .filter(|r| !matches!(r, Row::Head(..)))
            .collect()
    }

    /// Keys while the page is open. List: j/k pick, ↩ reads in full, `a`/`d`
    /// accept or reject a proposal, `e` rewrites it then accepts, `z` takes the
    /// last answer back, `/` searches, Esc or `q` closes. Reading:
    /// j/k and PgUp/PgDn scroll, Esc back to the list.
    pub(crate) fn keys(&mut self, mut bytes: &[u8], proposals: &[Decision]) -> Vec<Do> {
        let mut out = Vec::new();
        let picks = self.picks(proposals);
        let n = picks.len();
        self.cursor = self.place(&picks, proposals);
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
                    b"j" | b"\x1b[B" | b"\x1bOB" => *scroll = (*scroll + 1).min(SCROLL_MAX),
                    b"\x1b[5~" => *scroll = scroll.saturating_sub(20),
                    b"\x1b[6~" | b" " => *scroll = (*scroll + 20).min(SCROLL_MAX),
                    b"g" | b"\x1b[H" | b"\x1b[1~" => *scroll = 0,
                    _ => {}
                }
                continue;
            }
            let searching = self.search.typing;
            match self.search.key(key, whole) {
                Searched::No => {}
                // The search ended: the rest of the read would act on rows
                // picked before it narrowed them.
                _ if searching && !self.search.typing => break,
                Searched::Kept => continue,
                Searched::Changed => {
                    self.cursor = 0;
                    self.anchor = None;
                    continue;
                }
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
                        // One answer a read: the list changes under the next key.
                        break;
                    }
                }
                b"e" => {
                    if let Some(Row::Review(index)) = picks.get(self.cursor) {
                        out.push(Do::Edit { index: *index });
                        break;
                    }
                }
                b"z" => {
                    out.push(Do::Undo);
                    break;
                }
                _ => {}
            }
        }
        self.anchor = picks
            .get(self.cursor)
            .and_then(|r| self.anchor_of(r, proposals));
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
                let mut keys = vec!["a".fg(t.accent).bold(), " accept · ".fg(t.dim())];
                // A flagged decision or a proposed scope is in use already.
                if d.status == DecisionStatus::Proposed {
                    keys.extend(["e".fg(t.accent).bold(), " edit, then accept · ".fg(t.dim())]);
                }
                keys.extend(["d".fg(t.accent).bold(), " reject".fg(t.dim())]);
                if self.undo.is_some() {
                    keys.extend([
                        " · ".fg(t.dim()),
                        "z".fg(t.accent).bold(),
                        " undo".fg(t.dim()),
                    ]);
                }
                lines.push(Line::from(keys));
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
        let rect = crate::section_rect(area);
        frame.render_widget(Clear, rect);
        let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
        let picks = self.picks(proposals);
        let cursor = self.place(&picks, proposals);
        let here = picks.get(cursor).cloned();
        let on_review = matches!(here, Some(Row::Review(_)));
        let hints: Vec<Span> = if self.reading.is_some() {
            [hint(" ↑↓ ", "scroll "), hint(" esc ", "back ")].concat()
        } else if self.search.typing {
            [hint(" ↩ ", "keep "), hint(" esc ", "clear ")].concat()
        } else if on_review {
            let proposed = matches!(here, Some(Row::Review(i))
                if proposals.get(i).is_some_and(|d| d.status == DecisionStatus::Proposed));
            let mut hints = vec![hint(" a ", "accept ")];
            if proposed {
                hints.push(hint(" e ", "edit "));
            }
            hints.push(hint(" d ", "reject "));
            if self.undo.is_some() {
                hints.push(hint(" z ", "undo "));
            }
            hints.push(hint(" ↩ ", "read "));
            hints.concat()
        } else {
            let mut hints = vec![hint(" ↩ ", "read ")];
            if self.undo.is_some() {
                hints.push(hint(" z ", "undo "));
            }
            hints.push(hint(" / ", "search "));
            hints.concat()
        };
        let around = if self.reading.is_some() || self.search.typing {
            ""
        } else {
            "[ ] tab · j/k move"
        };
        let title = match self.tab {
            Tab::Decisions => " Decisions ",
            Tab::Skills => " Skills ",
            Tab::Notes => " Notes ",
        };
        let block = crate::section_block(t)
            .title(title.fg(t.fg).bold())
            .title_bottom(
                crate::page_footer(hints, around, rect.width.saturating_sub(2), t).left_aligned(),
            );
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        if let (Some(scroll), Some(row)) = (self.reading, &here) {
            let lines = self.detail(row, proposals, inner.width.saturating_sub(2), t);
            let text = Paragraph::new(lines).wrap(Wrap { trim: false });
            let total = text.line_count(inner.width.saturating_sub(2).max(1));
            let scroll = scroll.min(total.saturating_sub(inner.height as usize));
            frame.render_widget(
                text.scroll((scroll as u16, 0))
                    .block(Block::new().padding(ratatui::widgets::Padding::horizontal(1))),
                inner,
            );
            return;
        }

        let (list_area, detail_area) = crate::list_and_detail(frame, inner, t);

        let mut lines: Vec<Line> = vec![match self.search.shown() {
            true => self.search.line(t),
            false => Line::default(),
        }];
        match (&self.decisions, &self.failed) {
            (_, Some(e)) => lines.push(
                format!(" Couldn't load memory: {e} · trying again")
                    .fg(t.blocked)
                    .into(),
            ),
            (None, None) => lines.push(" Loading…".fg(t.muted).into()),
            _ if self.tab == Tab::Notes && self.notes_failed > 0 => lines.push(
                format!(
                    " Couldn't read the notes of {} repositor{} · trying again",
                    self.notes_failed,
                    if self.notes_failed == 1 { "y" } else { "ies" }
                )
                .fg(t.blocked)
                .into(),
            ),
            _ if picks.is_empty() && !self.search.query.is_empty() => lines.push(
                format!(" Nothing matches “{}”: esc clears it", self.search.query)
                    .fg(t.muted)
                    .into(),
            ),
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
        page.keys(b"k", &proposals);
        // Two answers in one read: only the first goes.
        assert_eq!(page.keys(b"aa", &proposals).len(), 1);
        // A proposal pushed ahead of it: the cursor stays on the same one.
        let more = vec![
            decision(8, "/r/api", DecisionKind::Gotcha, DecisionStatus::Proposed),
            proposals[0].clone(),
        ];
        assert_eq!(
            page.keys(b"d", &more),
            [Do::Review {
                index: 1,
                accept: false
            }]
        );
        // ↩ reads in full; Esc comes back, then closes.
        page.keys(b"\r", &proposals);
        assert!(!page.at_list());
        page.keys(b"\x1b", &proposals);
        assert!(page.at_list());
        assert_eq!(page.keys(b"\x1b", &proposals), [Do::Close]);
    }

    #[test]
    fn e_edits_only_a_proposal_and_z_always_asks_to_undo() {
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
        assert_eq!(page.keys(b"e", &proposals), [Do::Edit { index: 0 }]);
        // On an accepted one: nothing to edit before accepting.
        assert!(page.keys(b"je", &proposals).is_empty());
        // The app says when there's nothing to take back.
        assert_eq!(page.keys(b"z", &proposals), [Do::Undo]);
    }

    #[test]
    fn a_proposal_goes_through_the_editor_and_back() {
        let mut d = decision(9, "/r/api", DecisionKind::Gotcha, DecisionStatus::Proposed);
        d.scope = vec!["crates/a/**".into(), "web/*.ts".into()];
        let w = Wording::of(&d);
        let text = w.text();
        assert_eq!(
            text,
            "kind: gotcha\nscope: crates/a/** web/*.ts\n---\n# d9\n\nbody 9\n"
        );
        assert_eq!(Wording::parse(&text, false).unwrap(), w);
        // Rewritten: a new kind, one glob, a longer body.
        let edited = "kind: constraint\nscope: crates/a/**\n---\n# Sharper\n\nOne.\n\nTwo.\n";
        assert_eq!(
            Wording::parse(edited, false).unwrap(),
            Wording {
                title: "Sharper".into(),
                body: "One.\n\nTwo.".into(),
                kind: DecisionKind::Constraint,
                scope: Some(vec!["crates/a/**".into()]),
            }
        );
        // An empty scope applies everywhere; no scope line leaves it be.
        let everywhere = Wording::parse("kind: fix\nscope:\n---\n# T\n", false).unwrap();
        assert_eq!(everywhere.scope, Some(Vec::new()));
        let kept = Wording::parse("kind: fix\n---\n# T\n", false).unwrap();
        assert_eq!(kept.scope, None);
        // A habit has no scope line, and one typed in is ignored.
        let habit = decision(
            3,
            USER_SCOPE,
            DecisionKind::Decision,
            DecisionStatus::Proposed,
        );
        assert!(!Wording::of(&habit).text().contains("scope"));
        assert_eq!(
            Wording::parse("kind: fix\nscope: x\n---\n# T\n", true)
                .unwrap()
                .scope,
            None
        );
        // An editor that saves CRLF lines.
        assert_eq!(
            Wording::parse(&text.replace('\n', "\r\n"), false).unwrap(),
            w
        );
        // What it can't read, it says why.
        for bad in [
            "# no header\n",
            "kind: nope\n---\n# T\n",
            "---\n# T\n",
            "kind: fix\n---\nno heading\n",
            "kind: fix\n---\n#\n",
            "kind: fix\nwhat: ever\n---\n# T\n",
        ] {
            assert!(Wording::parse(bad, false).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn slash_narrows_every_group_and_counts_what_is_left() {
        let mut page = Page::new(Tab::Decisions);
        let mut a = decision(1, "/r/api", DecisionKind::Gotcha, DecisionStatus::Active);
        a.title = "Pin the socket path".into();
        let mut b = decision(2, "/r/web", DecisionKind::Gotcha, DecisionStatus::Active);
        b.title = "Toasts need their own row".into();
        let mut c = decision(
            3,
            "/r/api",
            DecisionKind::Constraint,
            DecisionStatus::Active,
        );
        c.title = "No secrets in logs".into();
        page.loaded(Ok((vec![a, b, c], Vec::new())));
        let proposals = vec![decision(
            9,
            "/r/api",
            DecisionKind::Fix,
            DecisionStatus::Proposed,
        )];
        assert!(page.keys(b"/web", &proposals).is_empty());
        assert!(!page.at_list(), "[ ] type while typing");
        assert_eq!(
            page.rows(Tab::Decisions, &proposals),
            [Row::Head("Gotchas".into(), 1), Row::Decision(1)]
        );
        // By repository and kind too; ↩ keeps it, Esc clears it, then closes.
        page.keys(b"\x7f\x7f\x7fapi gotcha\r", &proposals);
        assert_eq!(
            page.rows(Tab::Decisions, &proposals),
            [Row::Head("Gotchas".into(), 1), Row::Decision(0)]
        );
        assert!(page.keys(b"\x1b", &proposals).is_empty());
        assert_eq!(page.rows(Tab::Decisions, &proposals).len(), 7);
        assert_eq!(page.keys(b"\x1b", &proposals), [Do::Close]);
        // ↩ and a key after it in the same read (an unbracketed paste): the
        // rest isn't run against rows picked before the search narrowed them.
        assert!(page.keys(b"/web\ra", &proposals).is_empty());
        assert!(!page.search.typing);
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
            for want in [
                "To review 1",
                "■ d9",
                "Gotchas 1",
                "d1",
                "a accept",
                "e edit",
            ] {
                assert!(screen.contains(want), "{width}: lacks {want:?}");
            }
            // Wide, the selected one's text beside the list.
            assert_eq!(screen.contains("body 9"), width >= 140, "{width}");
        }
    }
}

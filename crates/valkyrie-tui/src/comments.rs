//! Comments on a doc (ADR-0021 §6, backlog #164), over the Docs page's reader:
//! `c` opens them. A comment is on the whole doc or on one block (a heading,
//! paragraph, list item or code block), and the session that wrote the doc
//! hears it at its next prompt. Blocks are quoted as the web app quotes them
//! (`blocks.rs`), so a comment from either finds its block in the other.

use crate::backlog::{decode, fit_hints};
use crate::blocks::{self, Block};
use crate::edit::LineEdit;
use crate::theme::Theme;
use jiff::tz::TimeZone;
use ratatui::layout::Rect;
use ratatui::style::{self, Color, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{BorderType, Clear, Paragraph};
use valkyrie_proto::{CommentState, DocComment};

/// Longest comment typed here, in characters.
const MAX_TEXT: usize = 4000;

/// What a key in the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Do {
    /// Leave a comment on the block `quote` reads, or the whole doc.
    Send {
        quote: Option<String>,
        text: String,
    },
    Resolve(u32),
}

#[derive(Debug)]
enum Mode {
    /// The comments, the cursor on one.
    List { cursor: usize },
    /// Which block to comment on: 0 is the whole doc, then each block.
    Pick { cursor: usize, blocks: Vec<Block> },
    /// Writing one, on the block `quote` reads or the whole doc.
    /// `sending` while the daemon has it: keys wait, so a held Enter can't send
    /// it twice.
    Write {
        quote: Option<String>,
        text: LineEdit,
        sending: bool,
    },
}

/// A doc's comments, and the panel when it's open.
#[derive(Debug, Default)]
pub(crate) struct Comments {
    list: Vec<DocComment>,
    failed: Option<String>,
    mode: Option<Mode>,
    /// A character cut off by the end of a read.
    partial: Vec<u8>,
    /// Inside a bracketed paste; `true` right after a line break.
    paste: Option<bool>,
}

impl Comments {
    pub(crate) fn loaded(&mut self, got: Result<Vec<DocComment>, String>) {
        match got {
            Ok(list) => {
                self.list = list;
                self.failed = None;
            }
            Err(e) => self.failed = Some(e),
        }
        if let Some(Mode::List { cursor }) = &mut self.mode {
            *cursor = (*cursor).min(self.list.len().saturating_sub(1));
        }
    }

    /// The comment wasn't taken: its box takes keys again, to send it again.
    pub(crate) fn unsent(&mut self) {
        if let Some(Mode::Write { sending, .. }) = &mut self.mode {
            *sending = false;
        }
    }

    /// The comment was taken: back to the list, which now has it.
    pub(crate) fn sent(&mut self, list: Vec<DocComment>) {
        self.loaded(Ok(list));
        self.mode = Some(Mode::List { cursor: 0 });
    }

    pub(crate) fn is_open(&self) -> bool {
        self.mode.is_some()
    }

    pub(crate) fn open(&mut self) {
        self.mode = Some(Mode::List { cursor: 0 });
    }

    /// How many aren't closed.
    pub(crate) fn open_count(&self) -> usize {
        self.list
            .iter()
            .filter(|c| c.state != CommentState::Resolved)
            .count()
    }

    /// The comments as listed: open ones first, newest first.
    fn shown(&self) -> Vec<&DocComment> {
        let mut shown: Vec<&DocComment> = self.list.iter().collect();
        shown.sort_by_key(|c| (c.state == CommentState::Resolved, std::cmp::Reverse(c.at)));
        shown
    }

    /// A key while the panel is open, with the doc's `body` for picking a block.
    /// A key while the panel is open; `body` gives the doc's body, for picking a
    /// block (made only then: a doc can be large).
    pub(crate) fn key(&mut self, key: &[u8], whole: bool, body: &dyn Fn() -> String) -> Option<Do> {
        // What the list shows, by id and state, before the mode is borrowed.
        let shown: Vec<(u32, CommentState)> =
            self.shown().iter().map(|c| (c.id, c.state)).collect();
        let mode = self.mode.as_mut()?;
        match mode {
            Mode::List { cursor } => {
                let n = self.list.len();
                match key {
                    b"k" | b"\x1b[A" | b"\x1bOA" => *cursor = cursor.saturating_sub(1),
                    b"j" | b"\x1b[B" | b"\x1bOB" => {
                        *cursor = (*cursor + 1).min(n.saturating_sub(1))
                    }
                    b"n" | b"c" => {
                        self.mode = Some(Mode::Pick {
                            cursor: 0,
                            blocks: blocks::blocks(&body()),
                        })
                    }
                    b"x" => {
                        let &(id, state) = shown.get(*cursor)?;
                        if state != CommentState::Resolved {
                            return Some(Do::Resolve(id));
                        }
                    }
                    b"\x1b" if whole => self.mode = None,
                    b"q" | [0x03] => self.mode = None,
                    _ => {}
                }
            }
            Mode::Pick { cursor, blocks } => match key {
                b"k" | b"\x1b[A" | b"\x1bOA" => *cursor = cursor.saturating_sub(1),
                b"j" | b"\x1b[B" | b"\x1bOB" => *cursor = (*cursor + 1).min(blocks.len()),
                b"g" | b"\x1b[H" | b"\x1b[1~" => *cursor = 0,
                b"G" | b"\x1b[F" | b"\x1b[4~" => *cursor = blocks.len(),
                b"\r" | b"\n" => {
                    let quote = cursor
                        .checked_sub(1)
                        .and_then(|i| blocks.get(i))
                        .map(|b| b.quote.clone());
                    self.mode = Some(Mode::Write {
                        quote,
                        text: LineEdit::default(),
                        sending: false,
                    });
                }
                b"\x1b" if whole => self.mode = Some(Mode::List { cursor: 0 }),
                b"q" | [0x03] => self.mode = Some(Mode::List { cursor: 0 }),
                _ => {}
            },
            Mode::Write { sending: true, .. } => {}
            Mode::Write {
                quote,
                text,
                sending,
            } => {
                // Esc or ^C ends a paste whose end never came, then does what it does.
                if (whole && key == b"\x1b") || key == [0x03] {
                    self.paste = None;
                }
                if let Some(after_break) = &mut self.paste {
                    // A paste is text, its line breaks spaces: none of it sends.
                    match key {
                        b"\x1b[201~" => self.paste = None,
                        b"\r" | b"\n" | b"\t" => {
                            if !*after_break {
                                text.insert(" ");
                            }
                            *after_break = true;
                        }
                        b" " if *after_break => {}
                        [b] if *b >= 0x20 && *b != 0x7f => {
                            text.insert(&decode(&mut self.partial, &[*b]));
                            *after_break = false;
                        }
                        _ => {}
                    }
                    text.truncate(MAX_TEXT);
                    return None;
                }
                match key {
                    b"\x1b[200~" => self.paste = Some(false),
                    b"\r" | b"\n" => {
                        let words = text.text().trim().to_string();
                        if !words.is_empty() {
                            *sending = true;
                            return Some(Do::Send {
                                quote: quote.clone(),
                                text: words,
                            });
                        }
                    }
                    b"\x1b" if whole => self.mode = Some(Mode::List { cursor: 0 }),
                    [0x03] => self.mode = Some(Mode::List { cursor: 0 }),
                    [b] if *b >= 0x20 && *b != 0x7f => {
                        text.insert(&decode(&mut self.partial, &[*b]));
                        text.truncate(MAX_TEXT);
                    }
                    _ => {
                        text.key(key);
                    }
                }
            }
        }
        None
    }

    /// The panel, over `area` (the reader's).
    pub(crate) fn draw(&self, frame: &mut ratatui::Frame, area: Rect, t: &Theme) {
        let Some(mode) = &self.mode else { return };
        let width = area.width.saturating_sub(6).min(96);
        let room = width.saturating_sub(4) as usize;
        let tz = TimeZone::system();
        let mut lines: Vec<Line> = vec![Line::default()];
        let (title, hints, cursor_rows) = match mode {
            Mode::List { cursor } => {
                if let Some(e) = &self.failed {
                    lines.push(
                        fit(&format!("Couldn't load them: {e}"), room)
                            .fg(t.needs)
                            .into(),
                    );
                }
                let shown = self.shown();
                if shown.is_empty() {
                    lines.push(
                        "No comments yet. The agent that wrote it hears one at"
                            .fg(t.muted)
                            .into(),
                    );
                    lines.push("its next prompt.".fg(t.muted).into());
                }
                let mut at = (0, 0);
                for (i, c) in shown.iter().enumerate() {
                    let from = lines.len();
                    let mut head = vec![
                        format!(" {} ", state_label(c.state))
                            .fg(t.bg)
                            .bg(state_color(c.state, t))
                            .bold(),
                        format!(" {}", when(c.at, &tz)).fg(t.muted),
                    ];
                    if let Some(to) = &c.sent_to {
                        head.push(format!(" · to {to}").fg(t.muted));
                    }
                    lines.push(Line::from(head));
                    let quote = match &c.quote {
                        Some(q) => format!("“{}”", fit(q, room.saturating_sub(2))),
                        None => "On the whole doc".into(),
                    };
                    lines.push(quote.fg(t.muted).italic().into());
                    for l in wrap(&c.text, room).into_iter().take(4) {
                        lines.push(l.fg(t.fg).into());
                    }
                    if i == *cursor {
                        at = (from, lines.len());
                        for l in &mut lines[from..] {
                            *l = std::mem::take(l).style(style::Style::new().bg(t.selection));
                        }
                    }
                    lines.push(Line::default());
                }
                let title = match self.open_count() {
                    0 => " Comments ".to_string(),
                    n => format!(" Comments · {n} open "),
                };
                let mut hints = hint(" n ", "new ", t);
                if shown
                    .get(*cursor)
                    .is_some_and(|c| c.state != CommentState::Resolved)
                {
                    hints.extend(hint(" x ", "resolve ", t));
                }
                hints.extend(hint(" esc ", "back ", t));
                (title, hints, at)
            }
            Mode::Pick { cursor, blocks } => {
                lines.push("Which part is it about?".fg(t.muted).into());
                lines.push(Line::default());
                let counts = |quote: &str| {
                    self.list
                        .iter()
                        .filter(|c| {
                            c.state != CommentState::Resolved && c.quote.as_deref() == Some(quote)
                        })
                        .count()
                };
                let mut at = (0, 0);
                let whole = std::iter::once(("doc", "The whole doc".to_string(), 0));
                let each = blocks
                    .iter()
                    .map(|b| (b.kind.label(), b.quote.clone(), counts(&b.quote)));
                for (i, (kind, text, n)) in whole.chain(each).enumerate() {
                    let here = i == *cursor;
                    let count = if n > 0 {
                        format!(" · {n}")
                    } else {
                        String::new()
                    };
                    let mut row = Line::from(vec![
                        if here { "▌".fg(t.accent) } else { " ".into() },
                        format!("{kind:<8}").fg(t.muted),
                        fit(&text, room.saturating_sub(9 + count.chars().count())).fg(t.fg),
                        count.fg(t.accent2),
                    ]);
                    if here {
                        row = row.style(style::Style::new().bg(t.selection));
                        at = (lines.len(), lines.len() + 1);
                    }
                    lines.push(row);
                }
                let hints = [hint(" ↩ ", "comment on it ", t), hint(" esc ", "back ", t)].concat();
                (" Comment on… ".to_string(), hints, at)
            }
            Mode::Write {
                quote,
                text,
                sending,
            } => {
                match quote {
                    Some(q) => {
                        for l in wrap(&format!("“{q}”"), room).into_iter().take(3) {
                            lines.push(l.fg(t.muted).italic().into());
                        }
                    }
                    None => lines.push("On the whole doc".fg(t.muted).italic().into()),
                }
                lines.push(Line::default());
                let mut input = vec![Span::from("› ").fg(t.accent)];
                input.extend(text.spans(room.saturating_sub(2), t));
                lines.push(Line::from(input));
                let hints = if *sending {
                    vec![" sending… ".fg(t.muted)]
                } else {
                    [hint(" ↩ ", "send ", t), hint(" esc ", "cancel ", t)].concat()
                };
                (" Comment ".to_string(), hints, (0, 0))
            }
        };
        let height = u16::try_from(lines.len())
            .unwrap_or(u16::MAX)
            .saturating_add(3)
            .min(area.height.saturating_sub(2))
            .max(5);
        let rect = Rect::new(
            area.x + (area.width.saturating_sub(width)) / 2,
            area.y + (area.height.saturating_sub(height)) / 2,
            width,
            height,
        )
        .intersection(area);
        let shown_rows = rect.height.saturating_sub(2) as usize;
        let skip = cursor_rows.1.saturating_sub(shown_rows).min(cursor_rows.0);
        let lines: Vec<Line> = lines.into_iter().skip(skip).collect();
        let block = ratatui::widgets::Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style::Style::new().fg(t.accent))
            .title(title.fg(t.fg).bold())
            .title_bottom(Line::from(fit_hints(hints, room)).centered())
            .padding(ratatui::widgets::Padding::horizontal(1))
            .style(style::Style::new().bg(t.panel).fg(t.fg));
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

fn state_label(s: CommentState) -> &'static str {
    match s {
        CommentState::Open => "open",
        CommentState::Sent => "sent",
        CommentState::Answered => "answered",
        CommentState::Resolved => "resolved",
    }
}

fn state_color(s: CommentState, t: &Theme) -> Color {
    match s {
        CommentState::Open => t.needs,
        CommentState::Sent => t.accent2,
        CommentState::Answered => t.done,
        CommentState::Resolved => t.muted,
    }
}

/// When, from seconds, in local time.
fn when(secs: u64, tz: &TimeZone) -> String {
    jiff::Timestamp::from_second(secs as i64)
        .map(|ts| ts.to_zoned(tz.clone()).strftime("%b %-d %H:%M").to_string())
        .unwrap_or_default()
}

/// `text` in rows of at most `width` columns, at spaces where it can.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines() {
        let mut row = String::new();
        for word in para.split_whitespace() {
            if !row.is_empty() && row.chars().count() + 1 + word.chars().count() > width {
                out.push(std::mem::take(&mut row));
            }
            if !row.is_empty() {
                row.push(' ');
            }
            row.push_str(word);
        }
        out.push(row);
    }
    out.into_iter().map(|r| fit(&r, width)).collect()
}

fn hint(k: &'static str, w: &'static str, t: &Theme) -> Vec<Span<'static>> {
    vec![k.fg(t.accent).bold(), w.fg(t.muted)]
}

fn fit(text: &str, max: usize) -> String {
    crate::fit(text, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(id: u32, at: u64, quote: Option<&str>, state: CommentState) -> DocComment {
        DocComment {
            id,
            at,
            quote: quote.map(Into::into),
            text: format!("text {id}"),
            state,
            sent_to: None,
            changed: at,
        }
    }

    fn press(c: &mut Comments, keys: &[&[u8]], body: &str) -> Vec<Do> {
        let body = body.to_string();
        keys.iter()
            .filter_map(|k| c.key(k, true, &|| body.clone()))
            .collect()
    }

    #[test]
    fn writes_a_comment_on_a_block() {
        let mut c = Comments::default();
        c.open();
        let body = "# Plan\n\nFirst step.\n";
        // n, down to the heading, then the paragraph, ↩, type, ↩.
        let got = press(&mut c, &[b"n", b"j", b"j", b"\r", b"o", b"k", b"\r"], body);
        assert_eq!(
            got,
            vec![Do::Send {
                quote: Some("First step.".into()),
                text: "ok".into()
            }]
        );
        c.sent(vec![comment(
            1,
            10,
            Some("First step."),
            CommentState::Open,
        )]);
        assert!(matches!(c.mode, Some(Mode::List { .. })));
        assert_eq!(c.open_count(), 1);
    }

    #[test]
    fn a_held_enter_sends_once_and_a_failed_send_can_go_again() {
        let mut c = Comments::default();
        c.open();
        let got = press(&mut c, &[b"n", b"\r", b"o", b"k", b"\r", b"\r", b"z"], "x");
        assert_eq!(got.len(), 1, "{got:?}");
        c.unsent();
        let Some(Mode::Write { text, .. }) = &c.mode else {
            panic!()
        };
        assert_eq!(text.text(), "ok", "nothing typed while it was sending");
        assert_eq!(press(&mut c, &[b"\r"], "x").len(), 1);
    }

    #[test]
    fn the_whole_doc_is_first_and_empty_text_does_not_send() {
        let mut c = Comments::default();
        c.open();
        assert_eq!(press(&mut c, &[b"n", b"\r", b"\r"], "x"), vec![]);
        assert_eq!(
            press(&mut c, &[b"h", b"i", b"\r"], "x"),
            vec![Do::Send {
                quote: None,
                text: "hi".into()
            }]
        );
    }

    #[test]
    fn a_paste_is_text_and_esc_goes_back_a_step() {
        let mut c = Comments::default();
        c.open();
        press(&mut c, &[b"n", b"\r"], "x");
        let got = press(
            &mut c,
            &[b"\x1b[200~", b"a", b"\r", b"b", b"\x1b[201~"],
            "x",
        );
        assert!(got.is_empty(), "a pasted line break doesn't send");
        let Some(Mode::Write { text, .. }) = &c.mode else {
            panic!()
        };
        assert_eq!(text.text(), "a b");
        press(&mut c, &[b"\x1b"], "x");
        assert!(matches!(c.mode, Some(Mode::List { .. })));
        press(&mut c, &[b"\x1b"], "x");
        assert!(!c.is_open());
    }

    #[test]
    fn resolves_the_one_under_the_cursor_open_ones_first() {
        let mut c = Comments::default();
        c.loaded(Ok(vec![
            comment(1, 10, None, CommentState::Resolved),
            comment(2, 5, None, CommentState::Sent),
            comment(3, 20, None, CommentState::Open),
        ]));
        c.open();
        assert_eq!(press(&mut c, &[b"x"], ""), vec![Do::Resolve(3)]);
        assert_eq!(press(&mut c, &[b"j", b"x"], ""), vec![Do::Resolve(2)]);
        // A closed one can't be closed again.
        assert_eq!(press(&mut c, &[b"j", b"x"], ""), vec![]);
    }

    #[test]
    fn typed_text_keeps_multibyte_characters() {
        let mut c = Comments::default();
        c.open();
        press(&mut c, &[b"n", b"\r"], "x");
        let bytes = "é✓".as_bytes();
        let keys: Vec<&[u8]> = bytes.chunks(1).collect();
        press(&mut c, &keys, "x");
        let Some(Mode::Write { text, .. }) = &c.mode else {
            panic!()
        };
        assert_eq!(text.text(), "é✓");
    }

    #[test]
    fn draws_each_mode() {
        let mut c = Comments::default();
        c.loaded(Ok(vec![comment(
            1,
            10,
            Some("First step."),
            CommentState::Sent,
        )]));
        c.open();
        let draw = |c: &Comments| {
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
            term.draw(|f| c.draw(f, f.area(), &crate::theme::DRACULA))
                .unwrap();
            term.backend()
                .buffer()
                .content
                .iter()
                .map(|x| x.symbol())
                .collect::<String>()
        };
        let s = draw(&c);
        assert!(
            s.contains("Comments · 1 open") && s.contains("“First step.”") && s.contains("text 1"),
            "{s}"
        );
        press(&mut c, &[b"n"], "# Plan\n\nFirst step.");
        let s = draw(&c);
        assert!(
            s.contains("The whole doc") && s.contains("heading") && s.contains("First step. · 1"),
            "{s}"
        );
        press(&mut c, &[b"j", b"\r", b"h"], "# Plan\n\nFirst step.");
        let s = draw(&c);
        assert!(s.contains("“Plan”") && s.contains("› h"), "{s}");
    }
}

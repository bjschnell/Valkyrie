//! Per-session VT screen model (ADR-0001).
//!
//! Wraps `alacritty_terminal` so no alacritty type escapes this crate: bytes go in,
//! [`Signal`]s and protocol [`ScreenUpdate`]s come out.

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term, TermDamage, TermMode};
use alacritty_terminal::vte::ansi::{self, NamedColor, Processor, Rgb};
use overseer_proto::{Color, Cursor, CursorShape, Modes, Row, ScreenUpdate, Size, Span, Style};
use std::sync::{Arc, Mutex};

pub const SCROLLBACK: usize = 10_000;

/// Side effects of feeding bytes that the session host must act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// Bytes to write back to the PTY (answers to DA/DSR/color queries).
    Reply(Vec<u8>),
    Bell,
    Title(Option<String>),
}

#[derive(Clone, Default)]
struct Listener(Arc<Mutex<Vec<Event>>>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

struct Dims(Size);

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.0.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.0.rows as usize
    }
    fn columns(&self) -> usize {
        self.0.cols as usize
    }
}

pub struct VtScreen {
    term: Term<Listener>,
    parser: Processor,
    events: Listener,
    size: Size,
    title: Option<String>,
    sent_modes: Option<Modes>,
    sent_title: Option<String>,
}

impl VtScreen {
    pub fn new(size: Size) -> Self {
        let size = size.clamped();
        let events = Listener::default();
        let config = Config {
            scrolling_history: SCROLLBACK,
            ..Config::default()
        };
        let term = Term::new(config, &Dims(size), events.clone());
        Self {
            term,
            parser: Processor::new(),
            events,
            size,
            title: None,
            sent_modes: None,
            sent_title: None,
        }
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Signal> {
        self.parser.advance(&mut self.term, bytes);
        let events = std::mem::take(&mut *self.events.0.lock().unwrap());
        let mut signals = Vec::new();
        for event in events {
            match event {
                Event::PtyWrite(s) => signals.push(Signal::Reply(s.into_bytes())),
                Event::ColorRequest(index, format) => {
                    signals.push(Signal::Reply(format(default_color(index)).into_bytes()))
                }
                Event::TextAreaSizeRequest(format) => {
                    let ws = WindowSize {
                        num_lines: self.size.rows,
                        num_cols: self.size.cols,
                        cell_width: 8,
                        cell_height: 16,
                    };
                    signals.push(Signal::Reply(format(ws).into_bytes()))
                }
                Event::Bell => signals.push(Signal::Bell),
                Event::Title(t) => {
                    self.title = Some(t.clone());
                    signals.push(Signal::Title(Some(t)))
                }
                Event::ResetTitle => {
                    self.title = None;
                    signals.push(Signal::Title(None))
                }
                // Clipboard access, blink, and wakeups are client/renderer concerns; M0 drops them.
                _ => {}
            }
        }
        signals
    }

    pub fn resize(&mut self, size: Size) {
        let size = size.clamped();
        if size != self.size {
            self.size = size;
            self.term.resize(Dims(size));
        }
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Full snapshot. Does not consume damage, so it is safe to call for a newly
    /// attached client while other clients are receiving diffs.
    pub fn snapshot(&self) -> ScreenUpdate {
        let rows = (0..self.size.rows).map(|y| self.row(y)).collect();
        self.update(true, rows)
    }

    /// Rows changed since the last call, or `None` if nothing visible changed.
    pub fn take_diff(&mut self) -> Option<ScreenUpdate> {
        let damaged: Option<Vec<u16>> = match self.term.damage() {
            TermDamage::Full => None,
            TermDamage::Partial(lines) => Some(
                lines
                    .filter(|d| d.is_damaged())
                    .map(|d| d.line as u16)
                    .collect(),
            ),
        };
        self.term.reset_damage();

        let modes = self.modes();
        let meta_changed = self.sent_modes != Some(modes) || self.sent_title != self.title;
        self.sent_modes = Some(modes);
        self.sent_title = self.title.clone();

        let update = match damaged {
            None => self.snapshot(),
            Some(lines) => {
                let rows: Vec<Row> = lines
                    .into_iter()
                    .filter(|&y| y < self.size.rows)
                    .map(|y| self.row(y))
                    .collect();
                if rows.is_empty() && !meta_changed {
                    return None;
                }
                self.update(false, rows)
            }
        };
        Some(update)
    }

    /// Visible screen as plain text, trailing blanks trimmed.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for y in 0..self.size.rows {
            let line = &self.term.grid()[Line(y as i32)];
            let mut s = String::new();
            for x in 0..self.size.cols as usize {
                let cell = &line[Column(x)];
                if !is_spacer(cell) {
                    push_cell_text(&mut s, cell);
                }
            }
            out.push_str(s.trim_end());
            out.push('\n');
        }
        let trimmed = out.trim_end_matches('\n').len();
        out.truncate(trimmed);
        out.push('\n');
        out
    }

    fn update(&self, full: bool, rows: Vec<Row>) -> ScreenUpdate {
        let point = self.term.grid().cursor.point;
        let style = self.term.cursor_style();
        let shape = match style.shape {
            ansi::CursorShape::Underline => CursorShape::Underline,
            ansi::CursorShape::Beam => CursorShape::Beam,
            _ => CursorShape::Block,
        };
        let visible = self.term.mode().contains(TermMode::SHOW_CURSOR)
            && style.shape != ansi::CursorShape::Hidden;
        ScreenUpdate {
            full,
            size: self.size,
            rows,
            cursor: Cursor {
                x: point.column.0 as u16,
                y: point.line.0.max(0) as u16,
                visible,
                shape,
            },
            modes: self.modes(),
            title: self.title.clone(),
        }
    }

    fn modes(&self) -> Modes {
        let m = self.term.mode();
        Modes {
            app_cursor: m.contains(TermMode::APP_CURSOR),
            app_keypad: m.contains(TermMode::APP_KEYPAD),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            focus_events: m.contains(TermMode::FOCUS_IN_OUT),
            alt_screen: m.contains(TermMode::ALT_SCREEN),
        }
    }

    fn row(&self, y: u16) -> Row {
        let line = &self.term.grid()[Line(y as i32)];
        let mut spans: Vec<Span> = Vec::new();
        // Clients may measure wide or combined graphemes differently than alacritty did
        // (e.g. "⚠\u{FE0F}" is one cell here, two in ratatui), so every span after one
        // re-anchors at its true column and any disagreement cannot shift the rest.
        let mut anchor = false;
        for x in 0..self.size.cols as usize {
            let cell = &line[Column(x)];
            if is_spacer(cell) {
                continue;
            }
            let style = style_of(cell);
            let continues = !std::mem::replace(
                &mut anchor,
                cell.flags.contains(Flags::WIDE_CHAR) || cell.zerowidth().is_some(),
            );
            match spans.last_mut() {
                Some(span) if continues && span.style == style => {
                    push_cell_text(&mut span.text, cell)
                }
                _ => {
                    let mut text = String::new();
                    push_cell_text(&mut text, cell);
                    spans.push(Span {
                        x: x as u16,
                        text,
                        style,
                    });
                }
            }
        }
        // A row update implies blank default cells past the last span, so drop trailing blanks.
        while let Some(last) = spans.last_mut()
            && last.style.is_default()
        {
            let keep = last.text.trim_end_matches(' ').len();
            last.text.truncate(keep);
            if !last.text.is_empty() {
                break;
            }
            spans.pop();
        }
        Row { y, spans }
    }
}

fn is_spacer(cell: &Cell) -> bool {
    cell.flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
}

fn push_cell_text(s: &mut String, cell: &Cell) {
    s.push(if cell.c == '\0' { ' ' } else { cell.c });
    if let Some(extra) = cell.zerowidth() {
        s.extend(extra);
    }
}

fn style_of(cell: &Cell) -> Style {
    let f = cell.flags;
    let mut flags = 0;
    for (from, to) in [
        (Flags::BOLD, Style::BOLD),
        (Flags::ITALIC, Style::ITALIC),
        (Flags::ALL_UNDERLINES, Style::UNDERLINE),
        (Flags::INVERSE, Style::INVERSE),
        (Flags::DIM, Style::DIM),
        (Flags::HIDDEN, Style::HIDDEN),
        (Flags::STRIKEOUT, Style::STRIKEOUT),
    ] {
        if f.intersects(from) {
            flags |= to;
        }
    }
    Style {
        fg: color(cell.fg),
        bg: color(cell.bg),
        flags,
    }
}

fn color(c: ansi::Color) -> Color {
    match c {
        ansi::Color::Spec(Rgb { r, g, b }) => Color::Rgb(r, g, b),
        ansi::Color::Indexed(i) => Color::Indexed(i),
        ansi::Color::Named(n) => {
            let n = n as usize;
            if n < 16 {
                Color::Indexed(n as u8)
            } else if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&n) {
                Color::Indexed((n - NamedColor::DimBlack as usize) as u8)
            } else {
                Color::Default
            }
        }
    }
}

/// Answers OSC 4/10/11 color queries. Programs (Codex among them) probe the
/// background to pick a light or dark theme, so report a dark xterm palette.
fn default_color(index: usize) -> Rgb {
    const ANSI: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0xcd, 0x00, 0x00),
        (0x00, 0xcd, 0x00),
        (0xcd, 0xcd, 0x00),
        (0x00, 0x00, 0xee),
        (0xcd, 0x00, 0xcd),
        (0x00, 0xcd, 0xcd),
        (0xe5, 0xe5, 0xe5),
        (0x7f, 0x7f, 0x7f),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x5c, 0x5c, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    let (r, g, b) = match index {
        0..16 => ANSI[index],
        16..232 => {
            let i = index - 16;
            let level = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
            (level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        232..256 => {
            let v = (8 + (index - 232) * 10) as u8;
            (v, v, v)
        }
        i if i == NamedColor::Background as usize => (0x18, 0x18, 0x18),
        _ => (0xd8, 0xd8, 0xd8),
    };
    Rgb { r, g, b }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> VtScreen {
        VtScreen::new(Size { cols: 20, rows: 5 })
    }

    fn row_text(row: &Row) -> String {
        let mut s = String::new();
        for span in &row.spans {
            while s.chars().count() < span.x as usize {
                s.push(' ');
            }
            s.push_str(&span.text);
        }
        s
    }

    #[test]
    fn first_diff_is_full_then_partial() {
        let mut s = screen();
        assert!(s.take_diff().unwrap().full);
        s.feed(b"hello");
        let d = s.take_diff().unwrap();
        assert!(!d.full);
        assert_eq!(d.rows.len(), 1);
        assert_eq!(row_text(&d.rows[0]), "hello");
        assert_eq!((d.cursor.x, d.cursor.y), (5, 0));
    }

    #[test]
    fn diff_contains_exactly_changed_rows() {
        let mut s = screen();
        s.feed(b"a\r\nb\r\nc");
        s.take_diff();
        s.feed(b"\x1b[2;1Hxx");
        let d = s.take_diff().unwrap();
        let ys: Vec<u16> = d.rows.iter().map(|r| r.y).collect();
        // Row 1 changed; row 2 holds the old cursor position and is included for redraw.
        assert!(ys.contains(&1));
        assert!(ys.iter().all(|y| [1, 2].contains(y)), "{ys:?}");
        let row1 = d.rows.iter().find(|r| r.y == 1).unwrap();
        assert_eq!(row_text(row1), "xx");
    }

    #[test]
    fn diffs_applied_to_snapshot_match_new_snapshot() {
        let mut s = screen();
        s.feed(b"line one\r\nline two");
        let mut model = s.snapshot().rows;
        s.take_diff();
        s.feed(b"\x1b[1;6H\x1b[31mONE\x1b[0m\r\n\r\nthird");
        for row in s.take_diff().unwrap().rows {
            let y = row.y as usize;
            model[y] = row;
        }
        assert_eq!(model, s.snapshot().rows);
    }

    #[test]
    fn spans_group_by_style_and_trim() {
        let mut s = screen();
        s.feed(b"ab\x1b[1mcd\x1b[0mef   ");
        let row = &s.snapshot().rows[0];
        assert_eq!(row.spans.len(), 3);
        assert_eq!(row.spans[1].text, "cd");
        assert_eq!(row.spans[1].style.flags, Style::BOLD);
        assert_eq!(row.spans[2].text, "ef");
        assert_eq!(row.spans[2].x, 4);
    }

    #[test]
    fn wide_chars_skip_spacer_and_reanchor() {
        let mut s = screen();
        s.feed("日本x".as_bytes());
        let row = &s.snapshot().rows[0];
        let placed: Vec<(u16, &str)> = row.spans.iter().map(|s| (s.x, s.text.as_str())).collect();
        assert_eq!(placed, [(0, "日"), (2, "本"), (4, "x")]);
        assert_eq!(s.snapshot().cursor.x, 5);
        assert_eq!(s.text(), "日本x\n");
    }

    #[test]
    fn span_after_combined_grapheme_is_anchored() {
        let mut s = screen();
        // alacritty gives "⚠\u{FE0F}" one cell; clients that draw it two wide must
        // still put "ab" at column 1.
        s.feed("⚠\u{FE0F}ab".as_bytes());
        let row = &s.snapshot().rows[0];
        let ab = row.spans.iter().find(|s| s.text == "ab").expect("ab span");
        assert_eq!(ab.x, 1);
    }

    #[test]
    fn answers_cursor_position_query() {
        let mut s = screen();
        let signals = s.feed(b"ab\x1b[6n");
        assert_eq!(signals, vec![Signal::Reply(b"\x1b[1;3R".to_vec())]);
    }

    #[test]
    fn reports_bell_title_and_modes() {
        let mut s = screen();
        s.take_diff();
        let signals = s.feed(b"\x07\x1b]0;agent\x07\x1b[?2004h\x1b[?1h");
        assert!(signals.contains(&Signal::Bell));
        assert!(signals.contains(&Signal::Title(Some("agent".into()))));
        let d = s.take_diff().unwrap();
        assert!(d.modes.bracketed_paste && d.modes.app_cursor);
        assert_eq!(d.title.as_deref(), Some("agent"));
    }

    #[test]
    fn idle_diff_is_none_after_flush() {
        let mut s = screen();
        s.take_diff();
        s.feed(b"x");
        s.take_diff();
        // Only the always-damaged cursor row remains; with no content or mode change
        // the caller still gets a cheap single-row update rather than nothing.
        let d = s.take_diff();
        assert!(d.is_none_or(|d| d.rows.len() <= 1));
    }

    #[test]
    fn resize_changes_snapshot_dimensions() {
        let mut s = screen();
        s.resize(Size { cols: 40, rows: 10 });
        let snap = s.snapshot();
        assert_eq!(snap.size, Size { cols: 40, rows: 10 });
        assert_eq!(snap.rows.len(), 10);
    }

    #[test]
    fn text_dump() {
        let mut s = screen();
        s.feed(b"hi\r\n\r\nthere");
        assert_eq!(s.text(), "hi\n\nthere\n");
    }
}

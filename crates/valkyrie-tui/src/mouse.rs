//! The mouse while attached, when the program doesn't want it: the wheel scrolls back,
//! a drag selects, and the selection is copied to the outer terminal's clipboard
//! (OSC 52, so it works over SSH). The TUI asks for SGR reports (1002 + 1006); Shift
//! still gives the terminal's own selection in most terminals.

use base64::Engine;
use std::io::Write;
use unicode_width::UnicodeWidthStr;
use valkyrie_proto::{Modes, Row};

/// Turns on button and drag reports, SGR-encoded.
pub const CAPTURE_ON: &str = "\x1b[?1002h\x1b[?1006h";
/// Every mouse mode off, whoever turned it on.
pub const CAPTURE_OFF: &str = "\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?1006l\x1b[?1005l";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Press,
    Drag,
    Release,
    WheelUp,
    WheelDown,
    /// The right button went down.
    RightPress,
    /// Other buttons, and plain motion.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mouse {
    pub kind: MouseKind,
    /// Cell, 0-based.
    pub x: u16,
    pub y: u16,
}

#[derive(Debug, PartialEq)]
pub enum Input {
    Bytes(Vec<u8>),
    Mouse(Mouse),
}

/// The mouse part of `Modes`, to tell when it changed.
pub fn modes(m: Modes) -> [bool; 5] {
    [
        m.mouse_click,
        m.mouse_drag,
        m.mouse_motion,
        m.mouse_sgr,
        m.mouse_utf8,
    ]
}

/// Where a mouse report that the read cut off starts, if the input ends in one. A
/// trailing `ESC [` may be one too (Alt-[ then waits for the next key); a lone ESC is
/// the Esc key and never held.
pub fn unfinished(bytes: &[u8]) -> Option<usize> {
    if bytes.ends_with(b"\x1b[") {
        return Some(bytes.len() - 2);
    }
    let start = bytes.windows(3).rposition(|w| w == b"\x1b[<")?;
    bytes[start + 3..]
        .iter()
        .all(|b| b.is_ascii_digit() || *b == b';')
        .then_some(start)
}

/// Splits raw input into keyboard bytes and SGR mouse reports (`ESC [ < b ; x ; y M/m`).
pub fn split(bytes: &[u8]) -> Vec<Input> {
    let mut out = Vec::new();
    let mut plain = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if let Some((mouse, len)) = parse_sgr(&bytes[i..]) {
            if !plain.is_empty() {
                out.push(Input::Bytes(std::mem::take(&mut plain)));
            }
            out.push(Input::Mouse(mouse));
            i += len;
        } else {
            plain.push(bytes[i]);
            i += 1;
        }
    }
    if !plain.is_empty() {
        out.push(Input::Bytes(plain));
    }
    out
}

/// Input for a program that wants the mouse, drawn `top` rows below the terminal's
/// top: SGR reports above it are taken out for Valkyrie, the rest move up by `top`.
/// Legacy X10 reports (`ESC [ M` and three bytes) move too; ones above are dropped.
pub fn route(bytes: &[u8], top: u16) -> (Vec<u8>, Vec<Mouse>) {
    let mut rest = Vec::with_capacity(bytes.len());
    let mut taken = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = &bytes[i..];
        if let Some((raw, len)) = parse_sgr_raw(b) {
            if raw.y <= top {
                taken.extend(to_mouse(raw));
            } else {
                let end = if raw.release { 'm' } else { 'M' };
                let (code, x, y) = (raw.code, raw.x, raw.y - top);
                rest.extend_from_slice(format!("\x1b[<{code};{x};{y}{end}").as_bytes());
            }
            i += len;
        } else if b.len() >= 6 && b.starts_with(b"\x1b[M") {
            // The row byte is 32 + the 1-based row.
            let row = b[5].saturating_sub(32) as u16;
            if row > top {
                rest.extend_from_slice(&b[..5]);
                rest.push(b[5] - top as u8);
            }
            i += 6;
        } else {
            rest.push(b[0]);
            i += 1;
        }
    }
    (rest, taken)
}

/// An SGR report as sent: the button code and 1-based cell.
#[derive(Debug, Clone, Copy)]
struct RawSgr {
    code: u16,
    x: u16,
    y: u16,
    release: bool,
}

fn parse_sgr_raw(b: &[u8]) -> Option<(RawSgr, usize)> {
    let rest = b.strip_prefix(b"\x1b[<")?;
    let end = rest.iter().position(|&c| c == b'M' || c == b'm')?;
    let body = std::str::from_utf8(&rest[..end]).ok()?;
    let mut nums = body.split(';').map(|n| n.parse::<u16>().ok());
    let (code, x, y) = (nums.next()??, nums.next()??, nums.next()??);
    if nums.next().is_some() {
        return None;
    }
    let release = rest[end] == b'm';
    Some((
        RawSgr {
            code,
            x,
            y,
            release,
        },
        3 + end + 1,
    ))
}

fn parse_sgr(b: &[u8]) -> Option<(Mouse, usize)> {
    let (raw, len) = parse_sgr_raw(b)?;
    Some((to_mouse(raw)?, len))
}

fn to_mouse(raw: RawSgr) -> Option<Mouse> {
    let RawSgr {
        code,
        x,
        y,
        release,
    } = raw;
    // Low bits: button; 32: motion; 64: wheel. 4/8/16 are Shift/Alt/Ctrl.
    let button = code & 0b11;
    let kind = if code & 128 != 0 {
        // Back, forward and the other extra buttons.
        MouseKind::Other
    } else if code & 64 != 0 {
        // 66 and 67 are the horizontal wheel.
        match button {
            0 => MouseKind::WheelUp,
            1 => MouseKind::WheelDown,
            _ => MouseKind::Other,
        }
    } else if button == 2 && code & 32 == 0 && !release {
        MouseKind::RightPress
    } else if button != 0 {
        MouseKind::Other
    } else if code & 32 != 0 {
        MouseKind::Drag
    } else if release {
        MouseKind::Release
    } else {
        MouseKind::Press
    };
    Some(Mouse {
        kind,
        x: x.saturating_sub(1),
        y: y.saturating_sub(1),
    })
}

/// A drag's two ends, in screen cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: (u16, u16),
    pub head: (u16, u16),
}

impl Selection {
    /// Start and end in reading order.
    pub fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        let key = |(x, y): (u16, u16)| (y, x);
        if key(self.anchor) <= key(self.head) {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn contains(&self, x: u16, y: u16) -> bool {
        let ((x0, y0), (x1, y1)) = self.ordered();
        (y0..=y1).contains(&y) && (y > y0 || x >= x0) && (y < y1 || x <= x1)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// The selected text: trailing blanks trimmed per line, soft-wrapped rows joined.
    /// `cols` is the screen width.
    pub fn text(&self, rows: &[Row], cols: u16) -> String {
        let ((x0, y0), (x1, y1)) = self.ordered();
        let mut out = String::new();
        for y in y0..=y1 {
            let row = rows.iter().find(|r| r.y == y);
            let mut cells = row_cells(row);
            // Rows drop their trailing blanks; a wrapped one keeps them, since they
            // are part of the line (`hello ` + `world`).
            if row.is_some_and(|r| r.wrapped) {
                cells.resize(cells.len().max(cols as usize), " ".into());
            }
            let mut from = if y == y0 { x0 as usize } else { 0 };
            // Starting on the right half of a wide character takes the character.
            while from > 0 && cells.get(from).is_some_and(String::is_empty) {
                from -= 1;
            }
            let to = if y == y1 {
                x1 as usize + 1
            } else {
                cells.len()
            };
            let line: String = cells
                .get(from.min(cells.len())..to.min(cells.len()))
                .unwrap_or_default()
                .concat();
            let wrapped = y < y1 && rows.iter().any(|r| r.y == y && r.wrapped);
            if wrapped {
                out.push_str(&line);
            } else {
                out.push_str(line.trim_end());
                if y < y1 {
                    out.push('\n');
                }
            }
        }
        out
    }
}

/// A row as one string per cell; a wide character's second cell is empty.
fn row_cells(row: Option<&Row>) -> Vec<String> {
    let mut cells: Vec<String> = Vec::new();
    for span in row.map(|r| r.spans.as_slice()).unwrap_or_default() {
        while cells.len() < span.x as usize {
            cells.push(" ".into());
        }
        cells.truncate(span.x as usize);
        for g in graphemes(&span.text) {
            match g.width() {
                0 => match cells.last_mut() {
                    Some(last) => last.push_str(g),
                    None => cells.push(g.to_owned()),
                },
                w => {
                    cells.push(g.to_owned());
                    for _ in 1..w {
                        cells.push(String::new());
                    }
                }
            }
        }
    }
    cells
}

/// Characters, with zero-width ones kept as their own pieces (they attach to the
/// previous cell).
fn graphemes(s: &str) -> impl Iterator<Item = &str> {
    s.char_indices().map(move |(i, c)| &s[i..i + c.len_utf8()])
}

/// Puts `text` on the outer terminal's clipboard.
pub fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    let mut out = std::io::stdout();
    write!(out, "{}", osc52(text))?;
    out.flush()
}

fn osc52(text: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text);
    // Inside tmux the sequence must be wrapped to reach the outer terminal.
    if std::env::var_os("TMUX").is_some() {
        format!("\x1bPtmux;\x1b\x1b]52;c;{b64}\x07\x1b\\")
    } else {
        format!("\x1b]52;c;{b64}\x07")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::Span;

    fn row(y: u16, spans: &[(u16, &str)], wrapped: bool) -> Row {
        Row {
            y,
            spans: spans
                .iter()
                .map(|&(x, text)| Span {
                    x,
                    text: text.into(),
                    style: Default::default(),
                })
                .collect(),
            wrapped,
        }
    }

    #[test]
    fn routes_reports_above_the_session_to_us_and_moves_the_rest_up() {
        // Row 2 (1-based) is in a 2-row strip; row 7 is the session's row 5.
        let (rest, taken) = route(b"a\x1b[<2;10;2M\x1b[<0;5;7mb\x1b[M !(\x1b[M !\"", 2);
        assert_eq!(rest, b"a\x1b[<0;5;5mb\x1b[M !&");
        assert_eq!(
            taken,
            [Mouse {
                kind: MouseKind::RightPress,
                x: 9,
                y: 1
            }]
        );
    }

    #[test]
    fn splits_sgr_mouse_from_keys() {
        let got = split(b"a\x1b[<0;5;2M\x1b[<32;7;2M\x1b[<0;7;2mb\x1b[<64;1;1M\x1b[<65;1;1M\x1b[A");
        let m = |kind, x, y| Input::Mouse(Mouse { kind, x, y });
        assert_eq!(
            got,
            [
                Input::Bytes(b"a".to_vec()),
                m(MouseKind::Press, 4, 1),
                m(MouseKind::Drag, 6, 1),
                m(MouseKind::Release, 6, 1),
                Input::Bytes(b"b".to_vec()),
                m(MouseKind::WheelUp, 0, 0),
                m(MouseKind::WheelDown, 0, 0),
                Input::Bytes(b"\x1b[A".to_vec()),
            ]
        );
        // Horizontal wheel is not a scroll.
        assert_eq!(
            split(b"\x1b[<66;1;1M"),
            [Input::Mouse(Mouse {
                kind: MouseKind::Other,
                x: 0,
                y: 0
            })]
        );
        assert_eq!(unfinished(b"x\x1b[<0;12"), Some(1));
        assert_eq!(unfinished(b"x\x1b[<0;12;3M"), None);
        assert_eq!(unfinished(b"\x1b"), None);
        assert_eq!(unfinished(b"ab\x1b["), Some(2));
        assert_eq!(
            split(b"\x1b[<128;1;1M"),
            [Input::Mouse(Mouse {
                kind: MouseKind::Other,
                x: 0,
                y: 0
            })]
        );
        // Not mouse: left alone.
        assert_eq!(split(b"\x1b[<1;2"), [Input::Bytes(b"\x1b[<1;2".to_vec())]);
    }

    #[test]
    fn selection_text_follows_reading_order_and_wraps() {
        let rows = [
            row(0, &[(0, "hello wor")], true),
            row(1, &[(0, "ld"), (6, "x")], false),
            row(2, &[(2, "日本")], false),
        ];
        // Dragged backwards, from row 1 col 2 up to row 0 col 6.
        let sel = Selection {
            anchor: (6, 1),
            head: (6, 0),
        };
        assert_eq!(sel.text(&rows, 9), "world    x");
        let sel = Selection {
            anchor: (0, 1),
            head: (4, 2),
        };
        assert_eq!(sel.text(&rows, 9), "ld    x\n  日本");
        assert!(sel.contains(9, 1) && sel.contains(0, 2) && !sel.contains(5, 2));
        // A wrap after a space keeps the space; starting mid wide char takes it.
        let rows = [
            row(0, &[(0, "hello")], true),
            row(1, &[(0, "world")], false),
        ];
        let sel = Selection {
            anchor: (0, 0),
            head: (4, 1),
        };
        assert_eq!(sel.text(&rows, 6), "hello world");
        let rows = [row(0, &[(0, "日本")], false)];
        let sel = Selection {
            anchor: (1, 0),
            head: (3, 0),
        };
        assert_eq!(sel.text(&rows, 9), "日本");
    }

    #[test]
    fn osc52_is_base64() {
        if std::env::var_os("TMUX").is_none() {
            assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
        }
    }
}

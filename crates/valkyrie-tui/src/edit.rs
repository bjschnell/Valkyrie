//! A one-line text prompt with a cursor that moves: the rename prompt's, and the
//! backlog's new-item and note lines.

use crate::theme::Theme;
use ratatui::style::Stylize;
use ratatui::text::Span;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct LineEdit {
    text: String,
    /// The cursor, as a byte offset on a character boundary.
    at: usize,
}

impl LineEdit {
    /// `text`, with the cursor after it.
    pub(crate) fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            at: text.len(),
            text,
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Types `s` at the cursor.
    pub(crate) fn insert(&mut self, s: &str) {
        self.text.insert_str(self.at, s);
        self.at += s.len();
    }

    /// Keeps the first `max` characters.
    pub(crate) fn truncate(&mut self, max: usize) {
        if let Some((cut, _)) = self.text.char_indices().nth(max) {
            self.text.truncate(cut);
            self.at = self.at.min(cut);
        }
    }

    /// An editing key, as readline has them: the arrows, Home and End (and ^A ^E ^B
    /// ^F), Ctrl or Alt with an arrow for a word, Backspace, Delete, ^W, ^U and ^K.
    /// Whether it was one.
    pub(crate) fn key(&mut self, key: &[u8]) -> bool {
        match key {
            b"\x1b[D" | b"\x1bOD" | [0x02] => self.at = self.prev(self.at),
            b"\x1b[C" | b"\x1bOC" | [0x06] => self.at = self.next(self.at),
            b"\x1b[H" | b"\x1bOH" | b"\x1b[1~" | b"\x1b[7~" | [0x01] => self.at = 0,
            b"\x1b[F" | b"\x1bOF" | b"\x1b[4~" | b"\x1b[8~" | [0x05] => self.at = self.text.len(),
            b"\x1b[1;5D" | b"\x1b[1;3D" => self.at = self.word_start(),
            b"\x1b[1;5C" | b"\x1b[1;3C" => self.at = self.word_end(),
            [0x7f] | [0x08] => {
                let from = self.prev(self.at);
                self.text.replace_range(from..self.at, "");
                self.at = from;
            }
            b"\x1b[3~" | [0x04] => {
                let to = self.next(self.at);
                self.text.replace_range(self.at..to, "");
            }
            [0x17] => {
                let from = self.word_start();
                self.text.replace_range(from..self.at, "");
                self.at = from;
            }
            [0x15] => {
                self.text.replace_range(..self.at, "");
                self.at = 0;
            }
            [0x0b] => self.text.truncate(self.at),
            _ => return false,
        }
        true
    }

    /// The text in at most `room` columns, the cursor on the character it's before
    /// (`▏` at the end), scrolled to keep it in view.
    pub(crate) fn spans(&self, room: usize, t: &Theme) -> Vec<Span<'static>> {
        let width = |s: &str| s.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>();
        let (before, rest) = self.text.split_at(self.at);
        let mut chars = rest.chars();
        let under = chars.next();
        let after = chars.as_str();
        let cursor = under.map_or(1, |c| c.width().unwrap_or(0).max(1));
        // What's before the cursor, from its end, as fits with the cursor.
        let mut shown = before;
        let mut cut = false;
        while width(shown) + cursor + usize::from(cut) > room && !shown.is_empty() {
            let first = shown.chars().next().map_or(0, char::len_utf8);
            shown = &shown[first..];
            cut = true;
        }
        let mut left = room.saturating_sub(width(shown) + cursor + usize::from(cut));
        let mut tail = String::new();
        for c in after.chars() {
            let w = c.width().unwrap_or(0);
            if w > left {
                break;
            }
            left -= w;
            tail.push(c);
        }
        let mut out = Vec::new();
        if cut {
            out.push("…".fg(t.muted));
        }
        out.push(shown.to_owned().fg(t.fg));
        out.push(match under {
            Some(c) => c.to_string().fg(t.bg).bg(t.accent),
            None => "▏".fg(t.accent),
        });
        out.push(tail.fg(t.fg));
        out
    }

    fn prev(&self, at: usize) -> usize {
        self.text[..at]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map_or(at, |c| at + c.len_utf8())
    }

    /// Where the word before the cursor starts, past any spaces.
    fn word_start(&self) -> usize {
        let before = self.text[..self.at].trim_end();
        before
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map_or(0, |(i, c)| i + c.len_utf8())
    }

    /// Where the word after the cursor ends, past any spaces.
    fn word_end(&self) -> usize {
        let rest = &self.text[self.at..];
        let skip = rest.len() - rest.trim_start().len();
        rest[skip..]
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
            .map_or(self.text.len(), |(i, _)| self.at + skip + i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(e: &mut LineEdit, keys: &[&[u8]]) {
        for k in keys {
            assert!(e.key(k), "{k:?}");
        }
    }

    #[test]
    fn the_cursor_moves_and_typing_goes_there() {
        let mut e = LineEdit::new("hllo");
        keys(&mut e, &[b"\x1b[D", b"\x1b[D", b"\x1b[D"]);
        e.insert("e");
        assert_eq!(e.text(), "hello");
        keys(&mut e, &[b"\x1b[F"]);
        e.insert("!");
        keys(&mut e, &[&[0x01]]);
        e.insert(">");
        assert_eq!(e.text(), ">hello!");
    }

    #[test]
    fn deletes_on_either_side_of_the_cursor() {
        let mut e = LineEdit::new("café au lait");
        keys(&mut e, &[b"\x1b[1;5D", b"\x1b[1;5D"]);
        // Backspace over the é, then Delete the space after "caf".
        keys(&mut e, &[b"\x1b[D", &[0x7f], b"\x1b[3~"]);
        assert_eq!(e.text(), "cafau lait");
        keys(&mut e, &[&[0x0b]]);
        assert_eq!(e.text(), "caf");
        let mut e = LineEdit::new("one two three");
        keys(&mut e, &[&[0x17]]);
        assert_eq!(e.text(), "one two ");
        keys(&mut e, &[b"\x1b[D", &[0x15]]);
        assert_eq!(e.text(), " ");
        assert!(!e.key(b"\r"), "not an editing key");
    }

    #[test]
    fn word_moves_skip_spaces() {
        let mut e = LineEdit::new("fix  the bug");
        keys(&mut e, &[b"\x1b[1;5D"]);
        e.insert("[");
        assert_eq!(e.text(), "fix  the [bug");
        keys(&mut e, &[&[0x01], b"\x1b[1;3C"]);
        e.insert("]");
        assert_eq!(e.text(), "fix]  the [bug");
    }

    #[test]
    fn truncate_keeps_the_cursor_in_the_text() {
        let mut e = LineEdit::new("ééééé");
        e.truncate(3);
        assert_eq!(e.text(), "ééé");
        e.insert("x");
        assert_eq!(e.text(), "éééx");
    }

    #[test]
    fn a_long_text_scrolls_to_the_cursor() {
        let t = crate::theme::DRACULA;
        let text = |spans: Vec<Span>| {
            spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        let mut e = LineEdit::new("abcdefghij");
        assert_eq!(text(e.spans(20, &t)), "abcdefghij▏");
        assert_eq!(text(e.spans(6, &t)), "…ghij▏");
        keys(&mut e, &[&[0x01]]);
        assert_eq!(text(e.spans(6, &t)), "abcdef");
    }
}

//! `/` on every list (TUI redesign T5b, backlog #273): type to narrow the list,
//! ↩ keeps what you typed and goes back to the list, Esc clears it. Each word
//! has to appear somewhere in what a row says, in any case.

use crate::theme::Theme;
use ratatui::style::Stylize;
use ratatui::text::Line;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Search {
    /// Keys go to the query, not the list.
    pub(crate) typing: bool,
    pub(crate) query: String,
    /// The start of a character a key cut off (`next_key` is a byte at a time
    /// outside escape sequences), joined to the next key.
    partial: Vec<u8>,
}

/// What a key did to the search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Searched {
    /// Not the search's: the list has it.
    No,
    /// Taken, nothing to redo.
    Kept,
    /// The query changed: the list is narrower or wider now.
    Changed,
}

impl Search {
    /// The key, if it's the search's: `/` starts typing; while typing, text,
    /// backspace, ↩ (keep) and Esc or ^C (clear); on the list, Esc clears a
    /// query before it closes anything.
    pub(crate) fn key(&mut self, key: &[u8], whole: bool) -> Searched {
        if !self.typing {
            return match key {
                b"/" => {
                    self.typing = true;
                    Searched::Kept
                }
                b"\x1b" if whole && !self.query.is_empty() => {
                    self.query.clear();
                    Searched::Changed
                }
                _ => Searched::No,
            };
        }
        match key {
            b"\r" | b"\n" => {
                self.typing = false;
                Searched::Kept
            }
            [0x03] => self.clear(),
            b"\x1b" if whole => self.clear(),
            [0x7f] | [0x08] => match self.query.pop() {
                Some(_) => Searched::Changed,
                None => Searched::Kept,
            },
            // ^U: the whole line.
            [0x15] => {
                self.query.clear();
                Searched::Changed
            }
            // An arrow or another escape while typing: nothing.
            [0x1b, ..] => Searched::Kept,
            _ => {
                self.partial.extend_from_slice(key);
                match std::str::from_utf8(&self.partial) {
                    Ok(text) => {
                        let text: String = text.chars().filter(|c| !c.is_control()).collect();
                        self.partial.clear();
                        if text.is_empty() {
                            return Searched::Kept;
                        }
                        self.query.push_str(&text);
                        Searched::Changed
                    }
                    // Waiting on the rest of the character.
                    Err(e) if e.error_len().is_none() && self.partial.len() < 4 => Searched::Kept,
                    Err(_) => {
                        self.partial.clear();
                        Searched::Kept
                    }
                }
            }
        }
    }

    fn clear(&mut self) -> Searched {
        self.typing = false;
        if self.query.is_empty() {
            Searched::Kept
        } else {
            self.query.clear();
            Searched::Changed
        }
    }

    /// Whether a row that says `text` stays: every word typed is in it.
    pub(crate) fn matches(&self, text: &str) -> bool {
        let text = text.to_lowercase();
        self.query
            .to_lowercase()
            .split_whitespace()
            .all(|word| text.contains(word))
    }

    /// Something typed, or being typed: the list is narrowed and says so.
    pub(crate) fn shown(&self) -> bool {
        self.typing || !self.query.is_empty()
    }

    /// The row over the list: `/ query`, with a cursor while typing.
    pub(crate) fn line(&self, t: &Theme) -> Line<'static> {
        let mut spans = vec![" / ".fg(t.accent).bold(), self.query.clone().fg(t.fg)];
        if self.typing {
            spans.push("▏".fg(t.accent));
        } else {
            spans.push("  esc clears".fg(t.muted));
        }
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(s: &mut Search, keys: &[&[u8]]) -> Vec<Searched> {
        keys.iter().map(|k| s.key(k, true)).collect()
    }

    #[test]
    fn slash_types_a_query_enter_keeps_it_and_esc_clears_it() {
        let mut s = Search::default();
        assert_eq!(s.key(b"j", true), Searched::No, "the list's, until /");
        assert_eq!(
            press(&mut s, &[b"/", b"a", b"P", b"i", b"\x7f"]),
            [
                Searched::Kept,
                Searched::Changed,
                Searched::Changed,
                Searched::Changed,
                Searched::Changed
            ]
        );
        assert_eq!(s.query, "aP");
        assert_eq!(s.key(b"\r", true), Searched::Kept);
        assert!(!s.typing && s.shown());
        // On the list, j is the list's; Esc clears before anything closes.
        assert_eq!(s.key(b"j", true), Searched::No);
        assert_eq!(s.key(b"\x1b", true), Searched::Changed);
        assert!(!s.shown());
        assert_eq!(s.key(b"\x1b", true), Searched::No, "then Esc is the page's");
        // Esc while typing drops it all; an arrow while typing is nothing.
        press(&mut s, &[b"/", b"x", b"\x1b[A"]);
        assert_eq!(s.query, "x");
        assert_eq!(s.key(b"\x1b", true), Searched::Changed);
        assert!(!s.typing && s.query.is_empty());
    }

    #[test]
    fn every_word_has_to_be_there_in_any_case() {
        let s = Search {
            query: "API  bug".into(),
            ..Search::default()
        };
        assert!(s.matches("fix the login · bug · api"));
        assert!(!s.matches("fix the login · feature · api"));
        assert!(Search::default().matches("anything"));
        // Non-Latin text arrives a byte at a time, and is matched as typed.
        let mut s = Search::default();
        s.key(b"/", true);
        for b in "日本".bytes() {
            s.key(&[b], true);
        }
        assert_eq!(s.query, "日本");
        assert!(s.matches("日本語のドキュメント"));
    }
}

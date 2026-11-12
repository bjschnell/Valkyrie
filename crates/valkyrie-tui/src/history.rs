//! A doc's past versions (ADR-0021 §4, backlog #167), over the Docs page's
//! reader: `h` opens them. Each is the file as it was before an edit replaced
//! it, newest first. `↩` shows one; `R` puts it back, and what it replaces joins
//! the list, so a restore is undone the same way.

use crate::backlog::fit_hints;
use crate::docs::body;
use crate::theme::Theme;
use jiff::tz::TimeZone;
use ratatui::layout::Rect;
use ratatui::style::{self, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{BorderType, Clear, Paragraph, Wrap};
use valkyrie_proto::DocVersion;

/// What a key in the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Do {
    /// Fetch this version's text.
    Peek(String),
    /// Put this version back.
    Restore(String),
}

/// The panel, while it's open.
#[derive(Debug, Default)]
pub(crate) struct History {
    versions: Option<Vec<DocVersion>>,
    failed: Option<String>,
    cursor: usize,
    /// A version shown, and its body.
    shown: Option<(String, String)>,
    /// How far the shown version is scrolled.
    top: u16,
}

impl History {
    pub(crate) fn loaded(&mut self, got: Result<Vec<DocVersion>, String>) {
        match got {
            Ok(v) => {
                self.cursor = self.cursor.min(v.len().saturating_sub(1));
                self.versions = Some(v);
                self.failed = None;
            }
            Err(e) => self.failed = Some(e),
        }
    }

    /// A version's text, as fetched: shown, unless you've moved off it.
    pub(crate) fn peeked(&mut self, version: &str, text: String) {
        if self.selected().is_some_and(|v| v.version == version) {
            self.shown = Some((version.to_string(), body(&text)));
            self.top = 0;
        }
    }

    fn selected(&self) -> Option<&DocVersion> {
        self.versions.as_ref()?.get(self.cursor)
    }

    /// A key while the panel is open; `None` back means it closed.
    pub(crate) fn key(&mut self, key: &[u8], whole: bool) -> (bool, Option<Do>) {
        if self.shown.is_some() {
            match key {
                b"j" | b"\x1b[B" | b"\x1bOB" => self.top = self.top.saturating_add(1),
                b"k" | b"\x1b[A" | b"\x1bOA" => self.top = self.top.saturating_sub(1),
                b" " | b"\x1b[6~" => self.top = self.top.saturating_add(10),
                b"b" | b"\x1b[5~" => self.top = self.top.saturating_sub(10),
                b"R" => {
                    let v = self.shown.as_ref().map(|(v, _)| v.clone());
                    return (true, v.map(Do::Restore));
                }
                b"\x1b" if whole => self.shown = None,
                b"q" | b"\r" | [0x03] => self.shown = None,
                _ => {}
            }
            return (true, None);
        }
        let n = self.versions.as_ref().map_or(0, Vec::len);
        match key {
            b"k" | b"\x1b[A" | b"\x1bOA" => self.cursor = self.cursor.saturating_sub(1),
            b"j" | b"\x1b[B" | b"\x1bOB" => {
                self.cursor = (self.cursor + 1).min(n.saturating_sub(1))
            }
            b"\r" | b"\n" => {
                if let Some(v) = self.selected() {
                    return (true, Some(Do::Peek(v.version.clone())));
                }
            }
            b"R" => {
                if let Some(v) = self.selected() {
                    return (true, Some(Do::Restore(v.version.clone())));
                }
            }
            b"\x1b" if whole => return (false, None),
            b"q" | [0x03] => return (false, None),
            _ => {}
        }
        (true, None)
    }

    pub(crate) fn draw(&self, frame: &mut ratatui::Frame, area: Rect, t: &Theme) {
        let width = area.width.saturating_sub(6).min(96);
        let room = width.saturating_sub(4) as usize;
        let tz = TimeZone::system();
        let mut lines: Vec<Line> = vec![Line::default()];
        let hints: Vec<Span<'static>>;
        let title;
        if let Some((version, text)) = &self.shown {
            let at = self
                .versions
                .iter()
                .flatten()
                .find(|v| &v.version == version)
                .map(|v| when(v.at, &tz))
                .unwrap_or_default();
            title = format!(" As it was {at} ");
            lines.extend(
                text.split('\n')
                    .skip(usize::from(self.top))
                    .map(|l| Line::from(l.to_string().fg(t.fg))),
            );
            hints = [
                hint(" j/k ", "scroll ", t),
                hint(" R ", "restore this ", t),
                hint(" esc ", "back ", t),
            ]
            .concat();
        } else {
            title = match &self.versions {
                Some(v) => format!(" History · {} ", v.len()),
                None => " History ".into(),
            };
            match (&self.versions, &self.failed) {
                (_, Some(e)) => lines.push(
                    crate::fit(&format!("Couldn't load it: {e}"), room)
                        .fg(t.needs)
                        .into(),
                ),
                (None, None) => lines.push("Loading…".fg(t.muted).into()),
                (Some(v), None) if v.is_empty() => {
                    lines.push(
                        "No past versions yet. Each edit keeps the one it"
                            .fg(t.muted)
                            .into(),
                    );
                    lines.push("replaces, for 30 days.".fg(t.muted).into());
                }
                (Some(v), None) => {
                    for (i, v) in v.iter().enumerate() {
                        let here = i == self.cursor;
                        let by = if v.by.is_empty() || v.by == "human" {
                            "you"
                        } else {
                            &v.by
                        };
                        let mut row = Line::from(vec![
                            if here { "▌".fg(t.accent) } else { " ".into() },
                            format!("{:<14}", when(v.at, &tz)).fg(t.fg),
                            format!("{by} · {}", kb(v.size)).fg(t.muted),
                        ]);
                        if here {
                            row = row.style(style::Style::new().bg(t.selection));
                        }
                        lines.push(row);
                    }
                }
            }
            hints = [
                hint(" ↩ ", "show ", t),
                hint(" R ", "restore ", t),
                hint(" esc ", "back ", t),
            ]
            .concat();
        }
        let want = u16::try_from(lines.len())
            .unwrap_or(u16::MAX)
            .saturating_add(3);
        let height = if self.shown.is_some() {
            area.height.saturating_sub(2)
        } else {
            want.min(area.height.saturating_sub(2)).max(5)
        };
        let rect = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        )
        .intersection(area);
        // The list keeps the cursor in view.
        let rows = rect.height.saturating_sub(2) as usize;
        let skip = if self.shown.is_none() {
            (self.cursor + 2).saturating_sub(rows.saturating_sub(1))
        } else {
            0
        };
        let lines: Vec<Line> = lines.into_iter().skip(skip).collect();
        let block = ratatui::widgets::Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style::Style::new().fg(t.accent))
            .title(title.fg(t.fg).bold())
            .title_bottom(Line::from(fit_hints(hints, room)).centered())
            .padding(ratatui::widgets::Padding::horizontal(1))
            .style(style::Style::new().bg(t.panel).fg(t.fg));
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(block),
            rect,
        );
    }
}

/// When, from seconds, in local time.
pub(crate) fn when(secs: u64, tz: &TimeZone) -> String {
    jiff::Timestamp::from_second(secs as i64)
        .map(|ts| ts.to_zoned(tz.clone()).strftime("%b %-d %H:%M").to_string())
        .unwrap_or_default()
}

fn kb(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

fn hint(k: &'static str, w: &'static str, t: &Theme) -> Vec<Span<'static>> {
    vec![k.fg(t.accent).bold(), w.fg(t.muted)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(version: &str, at: u64) -> DocVersion {
        DocVersion {
            version: version.into(),
            at,
            by: "claude".into(),
            size: 2048,
        }
    }

    #[test]
    fn shows_a_version_and_restores_it() {
        let mut h = History::default();
        h.loaded(Ok(vec![v("b", 20), v("a", 10)]));
        assert_eq!(h.key(b"j", true), (true, None));
        assert_eq!(h.key(b"\r", true), (true, Some(Do::Peek("a".into()))));
        // An answer for a version you've moved off isn't shown.
        h.peeked("b", "x".into());
        assert!(h.shown.is_none());
        h.peeked("a", "---\nvalk-id: 1\n---\nold text\n".into());
        assert_eq!(h.shown.as_ref().unwrap().1, "old text\n");
        assert_eq!(h.key(b"R", true), (true, Some(Do::Restore("a".into()))));
        assert_eq!(h.key(b"\x1b", true), (true, None));
        assert!(h.shown.is_none());
        assert_eq!(h.key(b"\x1b", true), (false, None));
    }

    #[test]
    fn draws_the_list_and_a_version() {
        let mut h = History::default();
        h.loaded(Ok(vec![v("a", 10)]));
        let draw = |h: &History| {
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
            term.draw(|f| h.draw(f, f.area(), &crate::theme::DRACULA))
                .unwrap();
            term.backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect::<String>()
        };
        let s = draw(&h);
        assert!(
            s.contains("History · 1") && s.contains("claude · 2.0 KB"),
            "{s}"
        );
        h.peeked("a", "old words".into());
        let s = draw(&h);
        assert!(
            s.contains("As it was") && s.contains("old words") && s.contains("restore this"),
            "{s}"
        );
    }
}

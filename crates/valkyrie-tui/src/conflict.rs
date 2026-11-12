//! When a save from the TUI finds the doc changed since it was opened (ADR-0021
//! §4, backlog #165): nothing was written. Yours and what's on disk show side by
//! side (stacked when narrow), lines one side has and the other doesn't marked,
//! as the web app shows them. `t` takes theirs, `e` edits yours again on top of
//! theirs, `s` saves yours over it.

use std::collections::HashSet;

use crate::docs::body;
use crate::theme::Theme;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{self, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use valkyrie_proto::Doc;

/// What to do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Drop yours; read theirs.
    Theirs,
    /// Edit yours again, saving over theirs this time.
    Edit,
    /// Save yours over theirs as it is.
    Mine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Conflict {
    /// Your text, not saved.
    pub(crate) mine: String,
    /// The doc as it is on disk now; its hash is what a save over it names.
    pub(crate) theirs: Doc,
}

/// The lines of `text`, by index, that `other` has nowhere. Blank lines are never
/// marked (the web app's `onlyIn`).
pub(crate) fn only_in(text: &str, other: &str) -> HashSet<usize> {
    let theirs: HashSet<&str> = other.split('\n').collect();
    text.split('\n')
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !theirs.contains(l))
        .map(|(i, _)| i)
        .collect()
}

impl Conflict {
    pub(crate) fn theirs_body(&self) -> String {
        body(self.theirs.text.as_deref().unwrap_or_default())
    }

    pub(crate) fn key(&self, key: &[u8]) -> Option<Choice> {
        match key {
            b"t" => Some(Choice::Theirs),
            b"e" => Some(Choice::Edit),
            b"s" => Some(Choice::Mine),
            _ => None,
        }
    }

    pub(crate) fn draw(&self, frame: &mut ratatui::Frame, area: Rect, t: &Theme) {
        let theirs = self.theirs_body();
        let [note, sides] =
            Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(area);
        let by = self
            .theirs
            .edited
            .last()
            .cloned()
            .unwrap_or_else(|| self.theirs.by.clone());
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(
                    format!(
                        "Someone saved this doc since you opened it ({by}). Yours wasn't saved."
                    )
                    .fg(t.needs),
                ),
                Line::from("Lines one side has and the other doesn't are marked.".fg(t.muted)),
            ])
            .wrap(Wrap { trim: false }),
            note,
        );
        let [a, b] = if sides.width >= 100 {
            Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).areas(sides)
        } else {
            Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).areas(sides)
        };
        side(frame, a, " Yours ", &self.mine, &theirs, t);
        side(frame, b, " On disk now ", &theirs, &self.mine, t);
    }
}

/// One version, the lines `other` lacks marked.
fn side(frame: &mut ratatui::Frame, area: Rect, title: &str, text: &str, other: &str, t: &Theme) {
    let marked = only_in(text, other);
    let lines: Vec<Line> = text
        .split('\n')
        .enumerate()
        .map(|(i, l)| {
            if marked.contains(&i) {
                Line::from(vec!["▌".fg(t.accent2), l.to_string().fg(t.fg)])
                    .style(style::Style::new().bg(t.selection))
            } else {
                Line::from(vec![" ".into(), l.to_string().fg(t.fg)])
            }
        })
        .collect();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style::Style::new().fg(t.border))
        .title(title.to_string().fg(t.fg).bold());
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_what_only_one_side_has() {
        let mine = "# Plan\n\nmine only\nshared\n";
        let theirs = "# Plan\n\nshared\ntheirs only\n";
        assert_eq!(only_in(mine, theirs), HashSet::from([2]));
        assert_eq!(only_in(theirs, mine), HashSet::from([3]));
    }
}

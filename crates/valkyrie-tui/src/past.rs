//! Past sessions (backlog #150): `h` on the home screen. The agent sessions of the
//! last weeks that have ended, newest first, as `valk sessions` lists them. `↩`
//! resumes one on its own conversation in a new tab, adding its worktree back
//! if that was taken away.

use crate::backlog::fit_hints;
use crate::next_key;
use crate::theme::Theme;
use jiff::tz::TimeZone;
use ratatui::layout::Rect;
use ratatui::style::{self, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use valkyrie_context::digest::when;
use valkyrie_proto::PastSession;

/// What a key on the page asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Do {
    Close,
    Load,
    /// Start this past session again.
    Resume(String),
    /// Tell the user, in the bar.
    Say(String),
}

/// The open page: what the daemon said, and which session is selected.
#[derive(Debug, Default)]
pub(crate) struct Page {
    sessions: Option<Vec<PastSession>>,
    failed: Option<String>,
    cursor: usize,
    /// A resume asked for and not answered yet: keys wait.
    pub(crate) resuming: bool,
}

impl Page {
    pub(crate) fn loaded(&mut self, got: Result<Vec<PastSession>, String>) {
        match got {
            Ok(list) => {
                self.cursor = self.cursor.min(list.len().saturating_sub(1));
                self.sessions = Some(list);
                self.failed = None;
            }
            Err(e) => {
                self.sessions = None;
                self.failed = Some(e);
            }
        }
    }

    fn selected(&self) -> Option<&PastSession> {
        self.sessions.as_ref()?.get(self.cursor)
    }

    /// Keys while the page is open: ↑/↓ (j/k) pick a session, ↩ resumes it, `r`
    /// loads again, Esc or `q` closes.
    pub(crate) fn keys(&mut self, mut bytes: &[u8]) -> Vec<Do> {
        let mut out = Vec::new();
        let n = self.sessions.as_ref().map_or(0, Vec::len);
        while !bytes.is_empty() {
            let key = next_key(bytes);
            let whole = key.len() == bytes.len();
            bytes = &bytes[key.len()..];
            match key {
                b"\x1b" if whole => out.push(Do::Close),
                b"q" | [0x03] => out.push(Do::Close),
                _ if self.resuming => {}
                b"k" | b"\x1b[A" | b"\x1bOA" => self.cursor = self.cursor.saturating_sub(1),
                b"j" | b"\x1b[B" | b"\x1bOB" => {
                    self.cursor = (self.cursor + 1).min(n.saturating_sub(1))
                }
                b"g" | b"\x1b[H" | b"\x1b[1~" => self.cursor = 0,
                b"G" | b"\x1b[F" | b"\x1b[4~" => self.cursor = n.saturating_sub(1),
                b"\r" | b"\n" => match self.selected() {
                    Some(PastSession {
                        cannot: Some(why), ..
                    }) => out.push(Do::Say(format!("can't resume it: {why}"))),
                    Some(p) => {
                        out.push(Do::Resume(p.entry.key.clone()));
                        self.resuming = true;
                    }
                    None => {}
                },
                b"r" => out.push(Do::Load),
                _ => {}
            }
        }
        out
    }

    pub(crate) fn draw(&self, frame: &mut ratatui::Frame, t: &Theme, tz: &TimeZone) {
        let area = frame.area();
        let rect = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        )
        .intersection(area);
        frame.render_widget(Clear, rect);
        let inner = rect.width.saturating_sub(4) as usize;
        let fit = crate::fit;

        let mut top = vec![Line::default()];
        match (&self.sessions, &self.failed) {
            (_, Some(e)) => top.push(
                format!(" Couldn't load past sessions: {}", fit(e, inner))
                    .fg(t.needs)
                    .into(),
            ),
            (None, None) => top.push(" Loading…".fg(t.muted).into()),
            (Some(list), None) if list.is_empty() => {
                top.push(
                    " No agent sessions have ended in the last 31 days."
                        .fg(t.muted)
                        .into(),
                );
            }
            (Some(list), None) => top.push(
                format!(
                    " {} agent session{} that ended, newest first. ↩ picks one up where it left off.",
                    list.len(),
                    if list.len() == 1 { "" } else { "s" }
                )
                .fg(t.fg)
                .into(),
            ),
        }

        let mut list: Vec<Line> = Vec::new();
        let mut at_cursor = (0, 0);
        for (i, p) in self.sessions.iter().flatten().enumerate() {
            let e = &p.entry;
            let here = i == self.cursor;
            let from = list.len();
            list.push(Line::default());
            let label = e.outcome.label();
            let room = inner.saturating_sub(label.len() + 4);
            list.push(Line::from(vec![
                if here { "▌".fg(t.accent) } else { " ".into() },
                format!(" {label} ").fg(t.bg).bg(t.muted).bold(),
                " ".into(),
                fit(&e.what, room).fg(t.fg).bold(),
            ]));
            let pad = "   ";
            let mut sub = vec![
                when(e.ended_ms.unwrap_or(e.last_ms), tz),
                format!("{} in {}", e.agent, e.repo),
            ];
            if let Some(branch) = &e.branch {
                sub.push(branch.clone());
            }
            list.push(
                format!("{pad}{}", fit(&sub.join(" · "), inner.saturating_sub(3)))
                    .fg(t.muted)
                    .into(),
            );
            let (note, color) = match (&p.cannot, &p.regrow) {
                (Some(why), _) => (format!("can't resume: {why}"), t.needs),
                (None, Some(branch)) => (
                    format!("{} · adds its worktree back on {branch}", e.detail),
                    t.fg,
                ),
                (None, None) => (e.detail.clone(), t.fg),
            };
            list.push(
                format!("{pad}{}", fit(&note, inner.saturating_sub(3)))
                    .fg(color)
                    .into(),
            );
            if here {
                for line in &mut list[from + 1..] {
                    *line = std::mem::take(line).style(style::Style::new().bg(t.selection));
                }
                at_cursor = (from, list.len());
            }
        }
        let room = (rect.height as usize).saturating_sub(top.len() + 2);
        let skip = at_cursor.1.saturating_sub(room).min(at_cursor.0);
        let mut lines = top;
        lines.extend(list.into_iter().skip(skip).take(room));

        let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
        let resume = match self.selected() {
            Some(p) if p.cannot.is_none() && !self.resuming => hint(" ↩ ", "resume ").to_vec(),
            _ => Vec::new(),
        };
        let hints = [
            resume,
            hint(" r ", "reload ").to_vec(),
            hint(" esc ", "close ").to_vec(),
        ]
        .concat();
        let hints = fit_hints(hints, rect.width.saturating_sub(4) as usize);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style::Style::new().fg(t.accent))
            .title(" Past sessions ".fg(t.fg).bold())
            .title_bottom(Line::from(hints).centered())
            .style(style::Style::new().bg(t.panel).fg(t.fg));
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::{DigestEntry, Outcome};

    fn past(key: &str, cannot: Option<&str>, regrow: Option<&str>) -> PastSession {
        PastSession {
            entry: DigestEntry {
                key: key.into(),
                session: None,
                repo: "valkyrie".into(),
                cwd: None,
                agent: "claude".into(),
                what: format!("work {key}"),
                item: None,
                branch: Some("feat-x".into()),
                pr: None,
                outcome: Outcome::Abandoned,
                detail: "ended with commits not pushed".into(),
                summary: None,
                prompts: 1,
                first_ms: 1_700_000_000_000,
                last_ms: 1_700_000_600_000,
                ended_ms: None,
            },
            started_ms: 1_700_000_000_000,
            cannot: cannot.map(Into::into),
            regrow: regrow.map(Into::into),
        }
    }

    #[test]
    fn enter_resumes_the_selected_session_once_and_says_why_one_cant() {
        let mut page = Page::default();
        page.loaded(Ok(vec![
            past("1-1", Some("its agent never named its conversation"), None),
            past("1-2", None, None),
        ]));
        assert_eq!(
            page.keys(b"\r"),
            vec![Do::Say(
                "can't resume it: its agent never named its conversation".into()
            )]
        );
        assert_eq!(page.keys(b"j\r"), vec![Do::Resume("1-2".into())]);
        // Waiting on that one: another press does nothing, Esc still closes.
        assert_eq!(page.keys(b"\rk"), vec![]);
        assert_eq!(page.keys(b"\x1b"), vec![Do::Close]);
        page.resuming = false;
        assert_eq!(page.keys(b"r"), vec![Do::Load]);
    }

    #[test]
    fn draws_each_session_with_what_resuming_it_does() {
        let mut page = Page::default();
        page.loaded(Ok(vec![
            past("1-1", None, Some("feat-x")),
            past("1-2", Some("gone"), None),
        ]));
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24)).unwrap();
        term.draw(|f| page.draw(f, &crate::theme::DRACULA, &TimeZone::UTC))
            .unwrap();
        let screen: String = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        for want in [
            "Past sessions",
            "work 1-1",
            "claude in valkyrie",
            "adds its worktree back on feat-x",
            "can't resume: gone",
            "resume",
        ] {
            assert!(screen.contains(want), "{want} missing");
        }
    }
}

//! The away digest (backlog #105, DESIGN §8.19): `w` on the home screen. What
//! every agent did while you were gone, a few lines a session under its
//! repository or day, as `valk digest` and the web app show it. Read-only: `↩`
//! opens a session still running, `y` copies the digest as markdown.

use crate::backlog::fit_hints;
use crate::next_key;
use crate::theme::Theme;
use jiff::tz::TimeZone;
use ratatui::layout::Rect;
use ratatui::style::{self, Color, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use valkyrie_context::digest as core;
use valkyrie_proto::{Digest, DigestEntry, DigestGroup, Outcome, SessionId};

/// How far back the digest looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Window {
    /// Since you were last gone, as the daemon saw your typing.
    #[default]
    Away,
    Hours12,
    Today,
    Days3,
}

impl Window {
    const ALL: [Window; 4] = [Window::Away, Window::Hours12, Window::Today, Window::Days3];

    fn label(self) -> &'static str {
        match self {
            Window::Away => "since you left",
            Window::Hours12 => "12 hours",
            Window::Today => "today",
            Window::Days3 => "3 days",
        }
    }

    /// Where it starts, in ms; `None` leaves it to the daemon (since you left).
    pub(crate) fn since(self, now: u64, tz: &TimeZone) -> Option<u64> {
        let text = match self {
            Window::Away => return None,
            Window::Hours12 => "12h",
            Window::Today => "today",
            Window::Days3 => "3d",
        };
        core::parse_since(text, now, tz).ok()
    }

    fn next(self) -> Window {
        let i = Window::ALL.iter().position(|&w| w == self).unwrap_or(0);
        Window::ALL[(i + 1) % Window::ALL.len()]
    }
}

/// What a key on the page asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Do {
    Close,
    /// Fetch the digest again, for the page's window and grouping.
    Load,
    /// Attach to this live session.
    Open(SessionId),
    /// Put the digest's markdown on the clipboard.
    Copy(String),
    /// Tell the user, in the bar.
    Say(String),
}

/// A line of the list: a heading, or a session.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    Head(String, usize),
    Entry(Box<DigestEntry>, String),
}

/// The open page: its window and grouping, what the daemon said, and which
/// session is selected.
#[derive(Debug, Default)]
pub(crate) struct Page {
    pub(crate) win: Window,
    pub(crate) by: DigestGroup,
    digest: Option<Digest>,
    failed: Option<String>,
    rows: Vec<Row>,
    /// Index among the sessions (not the headings).
    cursor: usize,
}

impl Page {
    /// The daemon's answer for the page's window and grouping.
    pub(crate) fn loaded(&mut self, got: Result<Digest, String>, tz: &TimeZone) {
        self.rows.clear();
        self.cursor = 0;
        match got {
            Ok(digest) => {
                for (head, list) in core::grouped(&digest, tz) {
                    self.rows.push(Row::Head(head, list.len()));
                    for e in list {
                        let at = core::at(e, digest.by, tz);
                        self.rows.push(Row::Entry(Box::new(e.clone()), at));
                    }
                }
                self.digest = Some(digest);
                self.failed = None;
            }
            Err(e) => {
                self.digest = None;
                self.failed = Some(e);
            }
        }
    }

    /// Forgets the old answer while a new one loads, so it can't pass for it.
    pub(crate) fn loading(&mut self) {
        self.digest = None;
        self.failed = None;
        self.rows.clear();
        self.cursor = 0;
    }

    fn entries(&self) -> impl Iterator<Item = &DigestEntry> {
        self.rows.iter().filter_map(|r| match r {
            Row::Entry(e, _) => Some(&**e),
            Row::Head(..) => None,
        })
    }

    fn selected(&self) -> Option<&DigestEntry> {
        self.entries().nth(self.cursor)
    }

    /// Keys while the page is open: ↑/↓ (j/k) pick a session, ↩ opens it if it
    /// still runs; `w` the next window, `g` repository or day; `y` copies; `r`
    /// loads again; Esc or `q` closes.
    pub(crate) fn keys(&mut self, mut bytes: &[u8]) -> Vec<Do> {
        let mut out = Vec::new();
        let n = self.entries().count();
        while !bytes.is_empty() {
            let key = next_key(bytes);
            let whole = key.len() == bytes.len();
            bytes = &bytes[key.len()..];
            match key {
                b"k" | b"\x1b[A" | b"\x1bOA" => self.cursor = self.cursor.saturating_sub(1),
                b"j" | b"\x1b[B" | b"\x1bOB" => {
                    self.cursor = (self.cursor + 1).min(n.saturating_sub(1))
                }
                b"g" | b"\x1b[H" | b"\x1b[1~" => self.cursor = 0,
                b"G" | b"\x1b[F" | b"\x1b[4~" => self.cursor = n.saturating_sub(1),
                b"\r" | b"\n" => match self.selected() {
                    Some(DigestEntry {
                        session: Some(id), ..
                    }) => out.push(Do::Open(*id)),
                    Some(_) => out.push(Do::Say("that session has ended".into())),
                    None => {}
                },
                b"w" => {
                    self.win = self.win.next();
                    out.push(Do::Load);
                }
                b"t" => {
                    self.by = match self.by {
                        DigestGroup::Repo => DigestGroup::Day,
                        DigestGroup::Day => DigestGroup::Repo,
                    };
                    out.push(Do::Load);
                }
                b"r" => out.push(Do::Load),
                b"y" => match &self.digest {
                    Some(d) if !d.entries.is_empty() => out.push(Do::Copy(d.markdown.clone())),
                    _ => out.push(Do::Say("nothing to copy".into())),
                },
                b"\x1b" if whole => out.push(Do::Close),
                b"q" | [0x03] => out.push(Do::Close),
                _ => {}
            }
        }
        out
    }

    pub(crate) fn draw(&self, frame: &mut ratatui::Frame, t: &Theme) {
        let area = frame.area();
        // Nearly the whole screen, like the backlog page.
        let rect = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        )
        .intersection(area);
        frame.render_widget(Clear, rect);
        let inner = rect.width.saturating_sub(4) as usize;

        let mut top = vec![Line::default()];
        let chip = |on: bool, text: String| {
            if on {
                format!(" {text} ").fg(t.bg).bg(t.accent).bold()
            } else {
                format!(" {text} ").fg(t.muted)
            }
        };
        let mut chips = vec![" ".into()];
        for w in Window::ALL {
            chips.push(chip(w == self.win, w.label().into()));
        }
        chips.push("   ".into());
        for g in [DigestGroup::Repo, DigestGroup::Day] {
            let name = if g == DigestGroup::Repo {
                "repo"
            } else {
                "day"
            };
            chips.push(chip(g == self.by, format!("by {name}")));
        }
        top.push(Line::from(chips));
        top.push(Line::default());
        match (&self.digest, &self.failed) {
            (_, Some(e)) => top.push(
                format!(" Couldn't load the digest: {}", fit(e, inner))
                    .fg(t.needs)
                    .into(),
            ),
            (None, None) => top.push(" Loading…".fg(t.muted).into()),
            (Some(d), None) => {
                // The daemon's own words, minus the clock zone: the TUI runs where you are.
                let about = core::about(d, &TimeZone::system());
                let about = if d.entries.is_empty() {
                    about
                } else {
                    format!("{} {}.", about, core::tally(&d.entries))
                };
                top.push(format!(" {}", fit(&about, inner)).fg(t.fg).into());
                if d.entries.is_empty() {
                    top.push(Line::default());
                    top.push(" No agent did anything in that time.".fg(t.muted).into());
                }
            }
        }

        // The sessions, a block each; the selected one kept in view.
        let mut list: Vec<Line> = Vec::new();
        let mut at_cursor = (0, 0);
        let mut i = 0;
        for row in &self.rows {
            match row {
                Row::Head(head, n) => {
                    list.push(Line::default());
                    list.push(Line::from(vec![
                        format!(" {}", fit(head, inner.saturating_sub(6)))
                            .fg(t.accent2)
                            .bold(),
                        format!(" · {n}").fg(t.muted),
                    ]));
                }
                Row::Entry(e, at) => {
                    let here = i == self.cursor;
                    let from = list.len();
                    let label = e.outcome.label();
                    let mut first = vec![
                        if here { "▌".fg(t.accent) } else { " ".into() },
                        format!(" {label} ").fg(t.bg).bg(color(e.outcome, t)).bold(),
                        " ".into(),
                    ];
                    let room = inner.saturating_sub(label.len() + 4);
                    first.push(fit(&e.what, room).fg(t.fg).bold());
                    list.push(Line::from(first));
                    let pad = "   ";
                    list.push(
                        format!("{pad}{}", fit(&e.detail, inner.saturating_sub(3)))
                            .fg(t.fg)
                            .into(),
                    );
                    let mut sub = at.clone();
                    if e.session.is_some() {
                        sub.push_str(" · running");
                    }
                    list.push(
                        format!("{pad}{}", fit(&sub, inner.saturating_sub(3)))
                            .fg(t.muted)
                            .into(),
                    );
                    if let Some(said) = &e.summary {
                        list.push(
                            format!("{pad}“{}”", fit(said, inner.saturating_sub(5)))
                                .fg(t.muted)
                                .italic()
                                .into(),
                        );
                    }
                    if here {
                        for line in &mut list[from..] {
                            *line = std::mem::take(line).style(style::Style::new().bg(t.selection));
                        }
                        at_cursor = (from, list.len());
                    }
                    i += 1;
                }
            }
        }
        let room = (rect.height as usize).saturating_sub(top.len() + 2);
        let skip = at_cursor.1.saturating_sub(room).min(at_cursor.0);
        let mut lines = top;
        lines.extend(list.into_iter().skip(skip).take(room));

        let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
        let open = match self.selected() {
            Some(e) if e.session.is_some() => hint(" ↩ ", "open ").to_vec(),
            _ => Vec::new(),
        };
        let hints = [
            open,
            hint(" w ", "window ").to_vec(),
            hint(" t ", "repo/day ").to_vec(),
            hint(" y ", "copy ").to_vec(),
            hint(" r ", "reload ").to_vec(),
            hint(" esc ", "close ").to_vec(),
        ]
        .concat();
        let hints = fit_hints(hints, rect.width.saturating_sub(4) as usize);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style::Style::new().fg(t.accent))
            .title(" Away digest ".fg(t.fg).bold())
            .title_bottom(Line::from(hints).centered())
            .style(style::Style::new().bg(t.panel).fg(t.fg));
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

fn fit(text: &str, max: usize) -> String {
    crate::fit(text, max)
}

/// Each outcome's color, as the session states it matches use them.
fn color(o: Outcome, t: &Theme) -> Color {
    match o {
        Outcome::Stuck => t.needs,
        Outcome::Abandoned => t.interrupted,
        Outcome::Waiting => t.accent2,
        Outcome::Working => t.working,
        Outcome::Shipped => t.done,
        Outcome::Done => t.idle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, repo: &str, outcome: Outcome, session: Option<SessionId>) -> DigestEntry {
        DigestEntry {
            key: key.into(),
            session,
            repo: repo.into(),
            cwd: None,
            agent: "claude".into(),
            what: format!("work {key}"),
            item: None,
            branch: None,
            pr: None,
            outcome,
            detail: "why".into(),
            summary: Some("said".into()),
            prompts: 1,
            first_ms: 1_700_000_000_000,
            last_ms: 1_700_000_600_000,
            ended_ms: None,
        }
    }

    fn digest(entries: Vec<DigestEntry>) -> Digest {
        Digest {
            since_ms: 1_699_990_000_000,
            until_ms: 1_700_001_000_000,
            away: true,
            by: DigestGroup::Repo,
            entries,
            markdown: "# Away digest\n".into(),
        }
    }

    #[test]
    fn moves_over_sessions_opens_live_ones_and_reloads_on_window_or_grouping() {
        let tz = TimeZone::UTC;
        let mut page = Page::default();
        page.loaded(
            Ok(digest(vec![
                entry("a", "one", Outcome::Stuck, Some(4)),
                entry("b", "two", Outcome::Shipped, None),
            ])),
            &tz,
        );
        assert_eq!(page.keys(b"\r"), vec![Do::Open(4)]);
        // Down past the heading of the second repository, onto the ended one.
        assert_eq!(page.keys(b"jj"), vec![]);
        assert_eq!(page.selected().unwrap().key, "b");
        assert_eq!(
            page.keys(b"\r"),
            vec![Do::Say("that session has ended".into())]
        );
        assert_eq!(page.keys(b"y"), vec![Do::Copy("# Away digest\n".into())]);
        assert_eq!(page.keys(b"w"), vec![Do::Load]);
        assert_eq!(page.win, Window::Hours12);
        assert_eq!(page.keys(b"t"), vec![Do::Load]);
        assert_eq!(page.by, DigestGroup::Day);
        assert_eq!(page.keys(b"\x1b"), vec![Do::Close]);
        page.loading();
        assert_eq!(page.keys(b"y"), vec![Do::Say("nothing to copy".into())]);
    }

    #[test]
    fn windows_start_where_they_say() {
        let tz = TimeZone::UTC;
        let now = 1_700_000_000_000;
        assert_eq!(Window::Away.since(now, &tz), None);
        assert_eq!(Window::Hours12.since(now, &tz), Some(now - 12 * 3_600_000));
        assert_eq!(Window::Days3.since(now, &tz), Some(now - 3 * 86_400_000));
        let today = Window::Today.since(now, &tz).unwrap();
        assert!(today <= now && now - today < 86_400_000 && today.is_multiple_of(86_400_000));
    }

    #[test]
    fn draws_a_session_with_its_outcome_and_what_it_said() {
        let mut page = Page::default();
        page.loaded(
            Ok(digest(vec![entry(
                "a",
                "valkyrie",
                Outcome::Stuck,
                Some(4),
            )])),
            &TimeZone::UTC,
        );
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        term.draw(|f| page.draw(f, &crate::theme::DRACULA)).unwrap();
        let screen: String = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        for want in [
            "Away digest",
            "valkyrie",
            "stuck",
            "work a",
            "running",
            "“said”",
        ] {
            assert!(screen.contains(want), "{want} missing");
        }
    }
}

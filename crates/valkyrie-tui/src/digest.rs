//! History (TUI redesign T4c, backlog #270; the away digest of backlog #105 and
//! the past sessions of #150 before it): one timeline of what every agent did
//! and what the supervisor did, by day, newest first. The window row picks how
//! far back (`[ ]`); `/` searches it. A row is a session or a supervisor step;
//! `↩` opens a session still running, `R` resumes one that ended, `y` copies the
//! summary as a standup. The selected row's detail shows beside the list on a
//! wide screen.

use crate::next_key;
use crate::theme::Theme;
use jiff::tz::TimeZone;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{self, Color, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use valkyrie_context::digest as core;
use valkyrie_proto::{Digest, DigestEntry, Outcome, PastSession, SessionId, SupervisorEntry};

/// How far back History looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Window {
    /// Since you were last gone, as the daemon saw your typing.
    #[default]
    Away,
    Today,
    Days3,
    /// Every session that has ended, weeks back: the ones `R` resumes.
    Ended,
}

impl Window {
    const ALL: [Window; 4] = [Window::Away, Window::Today, Window::Days3, Window::Ended];

    fn label(self) -> &'static str {
        match self {
            Window::Away => "Since you left",
            Window::Today => "Today",
            Window::Days3 => "3 days",
            Window::Ended => "Ended",
        }
    }

    /// Where it starts, in ms; `None` leaves it to the daemon (since you left).
    pub(crate) fn since(self, now: u64, tz: &TimeZone) -> Option<u64> {
        let text = match self {
            Window::Away | Window::Ended => return None,
            Window::Today => "today",
            Window::Days3 => "3d",
        };
        core::parse_since(text, now, tz).ok()
    }

    fn step(self, forward: bool) -> Window {
        let n = Window::ALL.len();
        let i = Window::ALL.iter().position(|&w| w == self).unwrap_or(0);
        Window::ALL[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }]
    }
}

/// What a key on the page asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Do {
    Close,
    /// Fetch the timeline again, for the page's window.
    Load,
    /// Attach to this live session.
    Open(SessionId),
    /// Resume this ended session, by its key.
    Resume(String),
    /// Put the summary's markdown on the clipboard.
    Copy(String),
    /// Tell the user, in the bar.
    Say(String),
}

/// A line of the timeline: a day, a session, or a supervisor step.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    Day(String, usize),
    Entry(Box<DigestEntry>),
    Step(Box<SupervisorEntry>),
}

/// What a row is, by identity, so a reload can find it again.
fn row_id(row: &Row) -> Option<String> {
    match row {
        Row::Day(..) => None,
        Row::Entry(e) => Some(format!("e{}", e.key)),
        Row::Step(st) => Some(format!("s{} {}", st.at, st.said)),
    }
}

impl Row {
    fn at_ms(&self) -> u64 {
        match self {
            Row::Day(..) => 0,
            Row::Entry(e) => e.ended_ms.unwrap_or(e.last_ms),
            Row::Step(s) => s.at * 1000,
        }
    }

    fn text(&self) -> String {
        match self {
            Row::Day(..) => String::new(),
            Row::Entry(e) => format!(
                "{} {} {} {} {}",
                e.what,
                e.repo,
                e.detail,
                e.branch.as_deref().unwrap_or_default(),
                e.summary.as_deref().unwrap_or_default()
            ),
            Row::Step(s) => format!("{} {} supervisor", s.said, repo_of(&s.project)),
        }
    }
}

/// The open page: its window, what the daemon said, the search, and the cursor.
#[derive(Debug, Default)]
pub(crate) struct Page {
    pub(crate) win: Window,
    digest: Option<Digest>,
    steps: Vec<SupervisorEntry>,
    past: Vec<PastSession>,
    failed: Option<String>,
    /// Why the supervisor's log or the ended sessions couldn't be read.
    steps_failed: Option<String>,
    past_failed: Option<String>,
    /// An answer is in (Ended doesn't need the digest's).
    answered: bool,
    /// Inside a bracketed paste while typing the search: its line breaks are
    /// spaces, not the end of typing.
    pasting: bool,
    rows: Vec<Row>,
    /// Index among the sessions and steps (not the days).
    cursor: usize,
    /// What `/` filters by, and whether it's being typed.
    query: String,
    typing: bool,
    /// A resume asked for and not answered yet: keys wait.
    pub(crate) resuming: bool,
}

impl Page {
    /// On the list, not typing a search or waiting on a resume: where section
    /// keys are the app's.
    pub(crate) fn at_list(&self) -> bool {
        !self.typing && !self.resuming
    }

    /// The daemon's answers for the page's window: the digest, the supervisor's
    /// log, and the sessions that ended (which `R` resumes), each on its own:
    /// one failing says so and leaves the others.
    pub(crate) fn loaded(
        &mut self,
        digest: Result<Digest, String>,
        steps: Result<Vec<SupervisorEntry>, String>,
        past: Result<Vec<PastSession>, String>,
        tz: &TimeZone,
    ) {
        self.cursor = 0;
        self.answered = true;
        (self.digest, self.failed) = match digest {
            Ok(d) => (Some(d), None),
            Err(e) => (None, Some(e)),
        };
        (self.steps, self.steps_failed) = match steps {
            Ok(s) => (s, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        (self.past, self.past_failed) = match past {
            Ok(p) => (p, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        self.build(tz);
    }

    /// A live reload's answer (`App::reload_page`): as `loaded`, but the
    /// cursor stays on the row it was on, and a failed reload keeps what was
    /// shown.
    pub(crate) fn refreshed(
        &mut self,
        digest: Result<Digest, String>,
        steps: Result<Vec<SupervisorEntry>, String>,
        past: Result<Vec<PastSession>, String>,
        tz: &TimeZone,
    ) {
        if digest.is_err() && self.digest.is_some() {
            return;
        }
        let was = self.selected().and_then(row_id);
        let cursor = self.cursor;
        self.loaded(digest, steps, past, tz);
        self.cursor = match was {
            Some(id) => self
                .items()
                .position(|r| row_id(r).as_ref() == Some(&id))
                .unwrap_or(cursor),
            None => cursor,
        };
        self.cursor = self.cursor.min(self.items().count().saturating_sub(1));
    }

    /// Forgets the old answer while a new one loads, so it can't pass for it.
    pub(crate) fn loading(&mut self) {
        self.digest = None;
        self.failed = None;
        self.answered = false;
        self.rows.clear();
        self.cursor = 0;
    }

    /// The rows for the window and search: sessions and steps in the window, by
    /// day, newest first.
    fn build(&mut self, tz: &TimeZone) {
        self.rows.clear();
        let mut items: Vec<Row> = if self.win == Window::Ended {
            self.past
                .iter()
                .map(|p| Row::Entry(Box::new(p.entry.clone())))
                .collect()
        } else {
            let Some(d) = &self.digest else { return };
            d.entries
                .iter()
                .map(|e| Row::Entry(Box::new(e.clone())))
                .chain(
                    self.steps
                        .iter()
                        .filter(|s| (d.since_ms..=d.until_ms).contains(&(s.at * 1000)))
                        .map(|s| Row::Step(Box::new(s.clone()))),
                )
                .collect()
        };
        let words: Vec<String> = self
            .query
            .to_lowercase()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        items.retain(|r| {
            let text = r.text().to_lowercase();
            words.iter().all(|w| text.contains(w))
        });
        items.sort_by_key(|r| std::cmp::Reverse(r.at_ms()));
        let mut day: Option<String> = None;
        for item in items {
            let label = day_label(item.at_ms(), tz);
            if day.as_ref() != Some(&label) {
                self.rows.push(Row::Day(label.clone(), 0));
                day = Some(label);
            }
            if let Some(Row::Day(_, n)) = self
                .rows
                .iter_mut()
                .rev()
                .find(|r| matches!(r, Row::Day(..)))
            {
                *n += 1;
            }
            self.rows.push(item);
        }
    }

    fn items(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| !matches!(r, Row::Day(..)))
    }

    fn selected(&self) -> Option<&Row> {
        self.items().nth(self.cursor)
    }

    /// The branch a resume adds the worktree back on, when it's gone.
    fn regrow(&self, key: &str) -> Option<&str> {
        self.past
            .iter()
            .find(|p| p.entry.key == key)?
            .regrow
            .as_deref()
    }

    /// Whether `key` (a past session's) can be resumed, and why not.
    fn resumable(&self, key: &str) -> Option<Result<(), String>> {
        let p = self.past.iter().find(|p| p.entry.key == key)?;
        Some(match &p.cannot {
            None => Ok(()),
            Some(why) => Err(why.clone()),
        })
    }

    /// Keys while the page is open: ↑/↓ (j/k) pick, ↩ opens a running session,
    /// `R` resumes an ended one, `[ ]` the window, `/` searches, `y` copies, `r`
    /// loads again, Esc clears the search or closes.
    pub(crate) fn keys(&mut self, mut bytes: &[u8], tz: &TimeZone) -> Vec<Do> {
        let mut out = Vec::new();
        while !bytes.is_empty() {
            let mut key = next_key(bytes);
            // A character of more than one byte comes a byte at a time: whole.
            if self.typing && key.len() == 1 && key[0] >= 0xC0 {
                let len = match key[0] {
                    0xF0.. => 4,
                    0xE0.. => 3,
                    _ => 2,
                };
                key = &bytes[..len.min(bytes.len())];
            }
            let whole = key.len() == bytes.len();
            bytes = &bytes[key.len()..];
            if self.typing {
                match key {
                    b"\x1b[200~" => self.pasting = true,
                    b"\x1b[201~" => self.pasting = false,
                    b"\r" | b"\n" if self.pasting => {
                        self.query.push(' ');
                        self.build(tz);
                    }
                    b"\r" | b"\n" => self.typing = false,
                    b"\x1b" if whole => {
                        self.typing = false;
                        self.query.clear();
                        self.build(tz);
                    }
                    [0x7f] | [0x08] => {
                        self.query.pop();
                        self.cursor = 0;
                        self.build(tz);
                    }
                    [0x03] => out.push(Do::Close),
                    _ => {
                        if let Ok(text) = std::str::from_utf8(key)
                            && !text.chars().any(char::is_control)
                        {
                            self.query.push_str(text);
                            self.cursor = 0;
                            self.build(tz);
                        }
                    }
                }
                continue;
            }
            let n = self.items().count();
            match key {
                b"q" | [0x03] => out.push(Do::Close),
                b"\x1b" if whole && self.resuming => out.push(Do::Close),
                _ if self.resuming => {}
                b"k" | b"\x1b[A" | b"\x1bOA" => self.cursor = self.cursor.saturating_sub(1),
                b"j" | b"\x1b[B" | b"\x1bOB" => {
                    self.cursor = (self.cursor + 1).min(n.saturating_sub(1))
                }
                b"g" | b"\x1b[H" | b"\x1b[1~" => self.cursor = 0,
                b"G" | b"\x1b[F" | b"\x1b[4~" => self.cursor = n.saturating_sub(1),
                b"\r" | b"\n" => match self.selected() {
                    Some(Row::Entry(e)) => match e.session {
                        Some(id) => out.push(Do::Open(id)),
                        None if self.resumable(&e.key).is_some() => {
                            out.push(Do::Say("that session has ended: R resumes it".into()))
                        }
                        None => out.push(Do::Say("that session has ended".into())),
                    },
                    Some(Row::Step(_)) => {
                        out.push(Do::Say("a supervisor step: nothing to open".into()))
                    }
                    _ => {}
                },
                b"R" => {
                    let picked = match self.selected() {
                        Some(Row::Entry(e)) if e.session.is_some() => {
                            Err("still running: ↩ opens it".to_string())
                        }
                        Some(Row::Entry(e)) => match self.resumable(&e.key) {
                            Some(Ok(())) => Ok(e.key.clone()),
                            Some(Err(why)) => Err(format!("can't resume it: {why}")),
                            None => Err("not one that can be resumed".into()),
                        },
                        _ => continue,
                    };
                    match picked {
                        Ok(key) => {
                            self.resuming = true;
                            out.push(Do::Resume(key));
                        }
                        Err(said) => out.push(Do::Say(said)),
                    }
                }
                b"[" | b"]" | b"w" => {
                    self.win = self.win.step(key != b"[");
                    out.push(Do::Load);
                }
                b"/" => self.typing = true,
                b"y" => match (&self.digest, self.win) {
                    // What's on screen: the ended sessions, as a list.
                    (_, Window::Ended) if !self.past.is_empty() => {
                        out.push(Do::Copy(ended_markdown(&self.past)))
                    }
                    (Some(d), w) if w != Window::Ended && !d.entries.is_empty() => {
                        out.push(Do::Copy(d.markdown.clone()))
                    }
                    _ => out.push(Do::Say("nothing to copy".into())),
                },
                b"\x1b" if whole && !self.query.is_empty() => {
                    self.query.clear();
                    self.cursor = 0;
                    self.build(tz);
                }
                b"\x1b" if whole => out.push(Do::Close),
                _ => {}
            }
        }
        out
    }

    /// One summary line: how long the window is, then each outcome counted in
    /// its state's word and color.
    fn summary(&self, d: &Digest, t: &Theme) -> Line<'static> {
        let mut spans: Vec<Span> = vec![" ".into()];
        // Short, as the spec has it: `9 h away:`, `Today:`, `3 days:`.
        let lead = match self.win {
            Window::Away => format!("{} away:", span(d.until_ms.saturating_sub(d.since_ms))),
            w => format!("{}:", w.label()),
        };
        spans.push(lead.fg(t.fg));
        let mut first = true;
        for o in Outcome::ALL {
            let n = d.entries.iter().filter(|e| e.outcome == o).count();
            if n == 0 {
                continue;
            }
            spans.push(if first { " ".into() } else { ", ".fg(t.muted) });
            first = false;
            spans.push(format!("{n} {}", word(o)).fg(color(o, t)).bold());
        }
        let steps = self.items().filter(|r| matches!(r, Row::Step(_))).count();
        if steps > 0 {
            spans.push(
                format!(
                    " · {steps} supervisor step{}",
                    if steps == 1 { "" } else { "s" }
                )
                .fg(t.muted),
            );
        }
        if self.steps_failed.is_some() {
            spans.push(" · the supervisor's log couldn't be read".fg(t.blocked));
        }
        Line::from(spans)
    }

    /// A row as listed: time, marker, name, repository, what happened.
    fn line(row: &Row, width: usize, tz: &TimeZone, t: &Theme) -> Line<'static> {
        let fit = crate::fit;
        let clock = clock(row.at_ms(), tz);
        let (mark, name, repo, happened) = match row {
            Row::Day(..) => return Line::default(),
            Row::Entry(e) => {
                let mark = match e.outcome {
                    Outcome::Abandoned => "○".fg(t.muted),
                    o => "●".fg(color(o, t)),
                };
                let happened = if e.detail.is_empty() {
                    vec![word(e.outcome).fg(color(e.outcome, t))]
                } else {
                    vec![
                        word(e.outcome).fg(color(e.outcome, t)),
                        format!(" · {}", e.detail).fg(t.secondary()),
                    ]
                };
                (mark, e.what.clone(), e.repo.clone(), happened)
            }
            Row::Step(s) => (
                "▸".fg(t.muted),
                "supervisor".to_string(),
                repo_of(&s.project),
                vec![s.said.clone().fg(t.secondary())],
            ),
        };
        let name = format!("{:<20}", fit(&name, 20));
        let repo = format!("{:<8}", fit(&repo, 8));
        let mut spans = vec![
            " ".into(),
            format!("{clock} ").fg(t.muted),
            mark,
            " ".into(),
            name.fg(t.fg).bold(),
            " ".into(),
            repo.fg(t.muted),
            " ".into(),
        ];
        let mut left = width.saturating_sub(Line::from(spans.clone()).width());
        for span in happened {
            if left == 0 {
                break;
            }
            let cut = fit(&span.content, left);
            left -= unicode_width::UnicodeWidthStr::width(cut.as_str());
            spans.push(Span::styled(cut, span.style));
        }
        Line::from(spans)
    }

    /// The selected row in full: what, where, the outcome, the agent's last
    /// words, and how to open or resume it.
    fn detail(&self, row: &Row, tz: &TimeZone, t: &Theme) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        match row {
            Row::Day(..) => {}
            Row::Entry(e) => {
                lines.push(Line::from(e.what.clone().fg(t.fg).bold()));
                let mut place = vec![e.agent.clone(), e.repo.clone()];
                place.extend(e.branch.clone());
                place.extend(e.pr.clone());
                lines.push(Line::from(place.join(" · ").fg(t.muted)));
                lines.push(Line::from(vec![
                    word(e.outcome).fg(color(e.outcome, t)).bold(),
                    if e.detail.is_empty() {
                        "".into()
                    } else {
                        format!(" · {}", e.detail).fg(t.secondary())
                    },
                ]));
                if let Some(said) = &e.summary {
                    lines.push(Line::default());
                    lines.push(Line::from(format!("“{said}”").fg(t.secondary()).italic()));
                }
                lines.push(Line::default());
                lines.push(Line::from(
                    format!(
                        "{} prompt{} · {} – {}",
                        e.prompts,
                        if e.prompts == 1 { "" } else { "s" },
                        clock(e.first_ms, tz),
                        e.ended_ms.map_or("running".into(), |m| clock(m, tz)),
                    )
                    .fg(t.muted),
                ));
                let how: Option<Line> = match (e.session, self.resumable(&e.key)) {
                    (Some(_), _) => Some(Line::from(vec![
                        "↩".fg(t.accent).bold(),
                        " open it".fg(t.dim()),
                    ])),
                    (None, Some(Ok(()))) => Some(Line::from(vec![
                        "R".fg(t.accent).bold(),
                        match self.regrow(&e.key) {
                            Some(branch) => {
                                format!(" resume it, adding its worktree back on {branch}")
                            }
                            None => " resume it".into(),
                        }
                        .fg(t.dim()),
                    ])),
                    (None, Some(Err(why))) => {
                        Some(Line::from(format!("can't resume: {why}").fg(t.muted)))
                    }
                    (None, None) => None,
                };
                if let Some(how) = how {
                    lines.push(how);
                }
            }
            Row::Step(s) => {
                lines.push(Line::from(s.said.clone().fg(t.fg).bold()));
                let who = match s.policy {
                    Some(n) => format!("taken by policy #{n}"),
                    None => "you answered".into(),
                };
                lines.push(Line::from(
                    format!("supervisor · {} · {who}", repo_of(&s.project)).fg(t.muted),
                ));
                if let Some(pr) = &s.pr {
                    lines.push(Line::from(pr.clone().fg(t.secondary())));
                }
            }
        }
        lines
    }

    pub(crate) fn draw(&self, frame: &mut ratatui::Frame, t: &Theme) {
        let tz = TimeZone::system();
        let area = frame.area();
        // Nearly the whole screen, like the backlog page.
        let rect = crate::section_rect(area);
        frame.render_widget(Clear, rect);

        let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
        let first = match self.selected() {
            Some(Row::Entry(e)) if e.session.is_some() => hint(" ↩ ", "open ").to_vec(),
            Some(Row::Entry(e)) if matches!(self.resumable(&e.key), Some(Ok(()))) => {
                hint(" R ", "resume ").to_vec()
            }
            _ => Vec::new(),
        };
        let (hints, around) = if self.typing {
            ([hint(" ↩ ", "done "), hint(" esc ", "clear ")].concat(), "")
        } else {
            (
                [
                    first,
                    hint(" / ", "search ").to_vec(),
                    hint(" y ", "copy ").to_vec(),
                ]
                .concat(),
                "[ ] window · j/k move",
            )
        };
        let block = crate::section_block(t)
            .title(" History ".fg(t.fg).bold())
            .title_bottom(
                crate::page_footer(hints, around, rect.width.saturating_sub(2), t).left_aligned(),
            );
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        // The window row and the search, then the summary.
        let mut top = vec![Line::default()];
        let mut row = vec![" ".into()];
        for (i, w) in Window::ALL.into_iter().enumerate() {
            if i > 0 {
                row.push(" · ".fg(t.muted));
            }
            row.push(if w == self.win {
                w.label().fg(t.fg).bold().underlined()
            } else {
                w.label().fg(t.dim())
            });
        }
        if self.typing || !self.query.is_empty() {
            row.push("    / ".fg(t.accent).bold());
            row.push(self.query.clone().fg(t.fg));
            if self.typing {
                row.push("▏".fg(t.accent));
            }
        }
        top.push(Line::from(row));
        match (&self.digest, &self.failed) {
            _ if self.win == Window::Ended => match &self.past_failed {
                Some(e) => top.push(
                    format!(" Couldn't read the ended sessions: {e} · trying again")
                        .fg(t.blocked)
                        .into(),
                ),
                None if !self.answered => top.push(" Loading…".fg(t.muted).into()),
                None => {
                    let n = self.past.len();
                    let plural = if n == 1 { "" } else { "s" };
                    top.push(
                        format!(" {n} session{plural} ended, newest first")
                            .fg(t.fg)
                            .into(),
                    );
                }
            },
            (_, Some(e)) => top.push(
                format!(" Couldn't load History: {e} · trying again")
                    .fg(t.blocked)
                    .into(),
            ),
            (None, None) => top.push(" Loading…".fg(t.muted).into()),
            (Some(d), None) => {
                top.push(self.summary(d, t));
                if self.rows.is_empty() {
                    top.push(Line::default());
                    top.push(
                        if self.query.is_empty() {
                            " Nothing happened in that time."
                        } else {
                            " Nothing matches the search."
                        }
                        .fg(t.muted)
                        .into(),
                    );
                }
            }
        }
        let [head, body] =
            Layout::vertical([Constraint::Length(top.len() as u16), Constraint::Fill(1)])
                .areas(inner);
        frame.render_widget(Paragraph::new(top), head);

        let (list_area, detail_area) = crate::list_and_detail(frame, body, t);

        let width = list_area.width as usize;
        let mut lines: Vec<Line> = Vec::new();
        let mut at = 0;
        let mut i = 0;
        for row in &self.rows {
            match row {
                Row::Day(day, n) => {
                    lines.push(Line::default());
                    lines.push(Line::from(vec![
                        format!(" {day} ").fg(t.dim()).bold(),
                        n.to_string().fg(t.dim()),
                    ]));
                }
                _ => {
                    let mut line = Self::line(row, width, &tz, t);
                    if i == self.cursor {
                        at = lines.len();
                        line.spans[0] = "▌".fg(t.accent);
                        let pad = width.saturating_sub(line.width());
                        line.spans.push(" ".repeat(pad).into());
                        line = t
                            .lift_faint(line)
                            .style(style::Style::new().bg(t.selection));
                    }
                    lines.push(line);
                    i += 1;
                }
            }
        }
        let skip = (at + 1).saturating_sub(list_area.height as usize);
        frame.render_widget(
            Paragraph::new(lines).scroll((skip.min(u16::MAX as usize) as u16, 0)),
            list_area,
        );

        if let (Some(area), Some(row)) = (detail_area, self.selected()) {
            frame.render_widget(
                Paragraph::new(self.detail(row, &tz, t))
                    .wrap(Wrap { trim: false })
                    .block(Block::new().padding(ratatui::widgets::Padding::new(1, 1, 1, 0))),
                area,
            );
        }
    }
}

/// The ended sessions as a markdown list, for `y` on Ended.
fn ended_markdown(past: &[PastSession]) -> String {
    let mut out = String::from("Sessions that ended:\n\n");
    for p in past {
        let e = &p.entry;
        out.push_str(&format!("- {} ({}): {}", e.what, e.repo, word(e.outcome)));
        if !e.detail.is_empty() {
            out.push_str(&format!(", {}", e.detail));
        }
        out.push('\n');
    }
    out
}

/// The one word for an outcome: a finished turn is "finished" (spec: one state
/// word per state); the rest say what happened.
fn word(o: Outcome) -> &'static str {
    match o {
        Outcome::Done => "finished",
        o => o.label(),
    }
}

/// Each outcome's color, as the session states it matches use them.
fn color(o: Outcome, t: &Theme) -> Color {
    match o {
        Outcome::Stuck => t.blocked,
        Outcome::Abandoned => t.muted,
        Outcome::Waiting => t.dim(),
        Outcome::Working => t.working,
        Outcome::Shipped | Outcome::Done => t.done,
    }
}

fn repo_of(project: &std::path::Path) -> String {
    project.file_name().map_or_else(
        || project.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn zoned(ms: u64, tz: &TimeZone) -> Option<jiff::Zoned> {
    let ts = jiff::Timestamp::from_millisecond(i64::try_from(ms).ok()?).ok()?;
    Some(ts.to_zoned(tz.clone()))
}

/// `40 min`, `9 h`, `2 days`.
fn span(ms: u64) -> String {
    let min = ms / 60_000;
    match min {
        0..60 => format!("{min} min"),
        60..2880 => format!("{} h", min / 60),
        _ => format!("{} days", min / 1440),
    }
}

/// `14:02`.
fn clock(ms: u64, tz: &TimeZone) -> String {
    zoned(ms, tz).map_or_else(|| "--:--".into(), |z| z.strftime("%H:%M").to_string())
}

/// `Today`, `Yesterday`, else `Thu 19 Nov`.
fn day_label(ms: u64, tz: &TimeZone) -> String {
    let Some(z) = zoned(ms, tz) else {
        return "Unknown day".into();
    };
    let today = jiff::Zoned::now().with_time_zone(tz.clone()).date();
    match today.since(z.date()).map(|s| s.get_days()) {
        Ok(0) => "Today".into(),
        Ok(1) => "Yesterday".into(),
        _ => z.strftime("%a %-d %b").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        key: &str,
        what: &str,
        outcome: &str,
        last_ms: u64,
        session: Option<u32>,
    ) -> DigestEntry {
        serde_json::from_value(serde_json::json!({
            "key": key, "session": session, "repo": "api", "agent": "claude", "what": what,
            "outcome": outcome, "detail": format!("{what} detail"), "prompts": 2,
            "first_ms": last_ms - 60_000, "last_ms": last_ms,
            "ended_ms": if session.is_some() { None } else { Some(last_ms) },
        }))
        .unwrap()
    }

    /// A live reload (`refreshed`) keeps the cursor on the row it was on when
    /// a new session lands above it, and a failed one keeps what was shown.
    #[test]
    fn a_live_reload_keeps_the_cursor_on_its_row() {
        let tz = TimeZone::UTC;
        let now = 100 * 3_600_000;
        let digest = |entries: Vec<DigestEntry>| -> Digest {
            serde_json::from_value(serde_json::json!({
                "since_ms": now - 3 * 3_600_000, "until_ms": now, "away": true, "by": "day",
                "entries": entries, "markdown": "",
            }))
            .unwrap()
        };
        let mut p = Page::default();
        let two = vec![
            entry("k1", "fix login", "stuck", now - 600_000, Some(1)),
            entry("k2", "bump vite", "done", now - 7_200_000, None),
        ];
        p.loaded(Ok(digest(two.clone())), Ok(vec![]), Ok(vec![]), &tz);
        p.cursor = 1;
        let on = |p: &Page| match p.selected() {
            Some(Row::Entry(e)) => e.key.clone(),
            _ => String::new(),
        };
        assert_eq!(on(&p), "k2");
        let mut three = two.clone();
        three.insert(0, entry("k0", "new one", "working", now - 60_000, Some(2)));
        p.refreshed(Ok(digest(three)), Ok(vec![]), Ok(vec![]), &tz);
        assert_eq!(on(&p), "k2", "still on bump vite, a row further down");
        p.refreshed(Err("gone".into()), Ok(vec![]), Ok(vec![]), &tz);
        assert!(
            p.failed.is_none() && on(&p) == "k2",
            "a failed reload keeps the rows"
        );
    }

    fn page(now: u64) -> Page {
        let tz = TimeZone::UTC;
        let digest: Digest = serde_json::from_value(serde_json::json!({
            "since_ms": now - 3 * 3_600_000, "until_ms": now, "away": true, "by": "day",
            "entries": [
                entry("k1", "fix login", "stuck", now - 600_000, Some(1)),
                entry("k2", "bump vite", "done", now - 7_200_000, None),
            ],
            "markdown": "# standup",
        }))
        .unwrap();
        let steps = vec![
            serde_json::from_value(serde_json::json!({
                "at": (now - 3_600_000) / 1000, "project": "/r/api", "said": "you took: fix CI on PR #9",
            }))
            .unwrap(),
            // Before the window: left out.
            serde_json::from_value(serde_json::json!({
                "at": (now - 10 * 3_600_000) / 1000, "project": "/r/api", "said": "old step",
            }))
            .unwrap(),
        ];
        let past: Vec<PastSession> = vec![
            serde_json::from_value(serde_json::json!({
                "key": "k2", "session": null, "repo": "api", "agent": "claude", "what": "bump vite",
                "outcome": "done", "detail": "", "prompts": 1, "first_ms": 0, "last_ms": now - 7_200_000,
                "started_ms": 0,
            }))
            .unwrap(),
        ];
        let mut p = Page::default();
        p.loaded(Ok(digest), Ok(steps), Ok(past), &tz);
        p
    }

    #[test]
    fn the_timeline_merges_sessions_and_steps_newest_first() {
        let now = 1_795_000_000_000;
        let mut p = page(now);
        let shown: Vec<String> = p.items().map(|r| r.text()).collect();
        assert_eq!(shown.len(), 3, "{shown:?}");
        assert!(shown[0].starts_with("fix login"));
        assert!(shown[1].contains("fix CI on PR #9"));
        assert!(shown[2].starts_with("bump vite"));

        let tz = TimeZone::UTC;
        // ↩ opens the running one; R resumes the ended one, once.
        assert_eq!(p.keys(b"\r", &tz), [Do::Open(1)]);
        assert_eq!(p.keys(b"jjR", &tz), [Do::Resume("k2".into())]);
        assert!(p.resuming && !p.at_list());
        p.resuming = false;

        // `/` searches; Esc clears it.
        p.keys(b"/vite", &tz);
        assert_eq!(p.items().count(), 1);
        p.keys(b"\r", &tz);
        assert!(p.at_list());
        p.keys(b"\x1b", &tz);
        assert_eq!(p.items().count(), 3);
        assert_eq!(p.keys(b"\x1b", &tz), [Do::Close]);

        // A character of several bytes, and a pasted line, go in whole.
        p.keys("/é".as_bytes(), &tz);
        p.keys(b"\x1b[200~a\nb\x1b[201~", &tz);
        assert_eq!(p.query, "éa b");
        assert!(p.typing);
        p.keys(b"\x1b", &tz);

        // Waiting on a resume, Esc and q still close.
        p.resuming = true;
        assert_eq!(p.keys(b"q", &tz), [Do::Close]);
        p.resuming = false;

        // `[ ]` move the window and ask again.
        assert_eq!(p.keys(b"]", &tz), [Do::Load]);
        assert_eq!(p.win, Window::Today);
        p.keys(b"[[", &tz);
        assert_eq!(p.win, Window::Ended);
        // Ended stands without the digest, and y copies what it shows.
        let tz = TimeZone::UTC;
        let past = std::mem::take(&mut p.past);
        p.loaded(Err("no digest".into()), Ok(Vec::new()), Ok(past), &tz);
        assert_eq!(p.items().count(), 1);
        match p.keys(b"y", &tz).as_slice() {
            [Do::Copy(md)] => assert!(md.contains("- bump vite (api)"), "{md}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_page_reads_in_state_words_with_the_detail_beside_it_when_wide() {
        let now = 1_795_000_000_000;
        let p = page(now);
        for width in [80u16, 140] {
            let mut term =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 24)).unwrap();
            term.draw(|f| p.draw(f, &crate::theme::DRACULA)).unwrap();
            let screen: String = term
                .backend()
                .buffer()
                .content
                .iter()
                .map(|c| c.symbol())
                .collect();
            for want in [
                "Since you left · Today · 3 days · Ended",
                "3 h away:",
                "1 stuck",
                "1 finished",
                "1 supervisor step",
                "fix login",
                "supervisor",
            ] {
                assert!(screen.contains(want), "{width}: lacks {want:?}");
            }
            assert!(!screen.contains("old step"));
            assert_eq!(screen.contains("2 prompts"), width >= 140, "{width}");
        }
    }
}

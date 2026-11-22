//! Backlog's Pull requests tab (TUI redesign T4c, backlog #270): the open pull
//! requests the supervisor's poll watches, by repository, in the order to merge
//! them, each with its state as the poll last read it. `↩` opens one in the
//! browser. It moved here from the supervisor page: pull requests are the
//! backlog's work, under way.

use crate::backlog::fit_hints;
use crate::next_key;
use crate::theme::Theme;
use ratatui::style::{self, Color, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};
use std::collections::HashMap;
use valkyrie_proto::supervisor::Watch;
use valkyrie_proto::{PrState, WatchedPr};

/// What a key on the page asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Do {
    Close,
    /// Open this pull request in the browser.
    Open(String),
}

#[derive(Debug, Default)]
pub(crate) struct Page {
    watch: Option<Result<Watch, String>>,
    /// Index among the pull requests, in the order drawn.
    cursor: usize,
}

impl Page {
    pub(crate) fn loaded(&mut self, got: Result<Watch, String>) {
        self.watch = Some(got);
        self.cursor = self.cursor.min(self.prs().len().saturating_sub(1));
    }

    /// How many it watches, for the strip.
    pub(crate) fn count(&self) -> Option<usize> {
        match &self.watch {
            Some(Ok(w)) => Some(w.prs.len()),
            _ => None,
        }
    }

    /// The pull requests grouped by repository, each group in the order to merge.
    fn prs(&self) -> Vec<&WatchedPr> {
        let Some(Ok(w)) = &self.watch else {
            return Vec::new();
        };
        let mut repos = Vec::new();
        for pr in &w.prs {
            if !repos.contains(&&pr.project) {
                repos.push(&pr.project);
            }
        }
        let mut out = Vec::new();
        for repo in repos {
            let mut these: Vec<&WatchedPr> = w.prs.iter().filter(|p| &p.project == repo).collect();
            these.sort_by_key(|p| p.order.unwrap_or(u32::MAX));
            out.extend(these);
        }
        out
    }

    /// Keys while it's open: j/k pick, ↩ opens in the browser, `r` reads them
    /// again, Esc or `q` closes.
    pub(crate) fn keys(&mut self, mut bytes: &[u8]) -> Vec<Do> {
        let mut out = Vec::new();
        let prs: Vec<String> = self.prs().iter().map(|p| p.url.clone()).collect();
        let n = prs.len();
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
                b"\r" | b"\n" => {
                    // Only a web address goes to the browser opener, never
                    // something it could read as an option.
                    if let Some(url) = prs
                        .get(self.cursor)
                        .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
                    {
                        out.push(Do::Open(url.clone()));
                    }
                }
                b"\x1b" if whole => out.push(Do::Close),
                b"q" | [0x03] => out.push(Do::Close),
                _ => {}
            }
        }
        out
    }

    /// `numbers`: item id to the number its repository calls it (decision #73).
    pub(crate) fn draw(
        &self,
        frame: &mut ratatui::Frame,
        numbers: &HashMap<u32, u32>,
        now: u64,
        t: &Theme,
    ) {
        let area = frame.area();
        let rect = crate::section_rect(area);
        frame.render_widget(Clear, rect);
        let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
        let hints: Vec<Span> = [
            hint(" ↩ ", "open "),
            hint(" r ", "read again "),
            hint(" esc ", "close "),
        ]
        .concat();
        let block = crate::section_block(t)
            .title(" Pull requests ".fg(t.fg).bold())
            .title_bottom(
                Line::from(fit_hints(hints, rect.width.saturating_sub(4) as usize)).left_aligned(),
            );
        let inner = block.inner(rect);
        frame.render_widget(block, rect);

        let (list_area, detail_area) = crate::list_and_detail(frame, inner, t);

        let ago = |at: u64| crate::age(at * 1000, now);
        let mut lines = vec![Line::default()];
        match &self.watch {
            None => lines.push(" Loading…".fg(t.muted).into()),
            Some(Err(e)) => lines.push(
                format!(" Couldn't read them: {e} · trying again")
                    .fg(t.blocked)
                    .into(),
            ),
            Some(Ok(w)) if !w.on => lines.push(
                " The supervisor's poll is off: nothing is read."
                    .fg(t.muted)
                    .into(),
            ),
            Some(Ok(w)) if w.prs.is_empty() => {
                lines.push(" No open pull request of yours.".fg(t.muted).into())
            }
            Some(Ok(w)) => lines.push(
                match w.polled_at {
                    Some(at) => format!(" {} open, read {} ago", w.prs.len(), ago(at)),
                    None => " The poll hasn't run yet.".into(),
                }
                .fg(t.muted)
                .into(),
            ),
        }
        let prs = self.prs();
        let width = list_area.width as usize;
        let mut repo = None;
        let mut at = 0;
        for (i, pr) in prs.iter().enumerate() {
            if repo != Some(&pr.project) {
                repo = Some(&pr.project);
                let n = prs.iter().filter(|p| p.project == pr.project).count();
                lines.push(Line::default());
                lines.push(Line::from(vec![
                    format!(" {} ", repo_of(&pr.project)).fg(t.dim()).bold(),
                    n.to_string().fg(t.dim()),
                ]));
            }
            let mut line = Self::line(pr, numbers, width, t);
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
        }
        let skip = (at + 1).saturating_sub(list_area.height as usize);
        frame.render_widget(
            Paragraph::new(lines).scroll((skip.min(u16::MAX as usize) as u16, 0)),
            list_area,
        );

        if let (Some(area), Some(pr)) = (detail_area, prs.get(self.cursor)) {
            let mut detail = vec![
                Line::from(number(&pr.url).fg(t.fg).bold()),
                Line::from(pr.url.clone().fg(t.muted)),
                Line::from(vec![
                    pr.state.label().fg(color(pr.state, t)).bold(),
                    format!(" · {}", pr.detail).fg(t.secondary()),
                ]),
            ];
            let mut on = vec![repo_of(&pr.project)];
            on.extend(pr.branch.clone());
            if let Some(item) = pr.item {
                on.push(format!("#{}", numbers.get(&item).copied().unwrap_or(item)));
            }
            detail.push(Line::from(on.join(" · ").fg(t.muted)));
            if let Some(order) = pr.order {
                detail.push(Line::from(
                    format!("merge it {}", ordinal(order)).fg(t.secondary()),
                ));
            }
            if let Some(note) = &pr.merge_note {
                detail.push(Line::from(note.clone().fg(t.secondary())));
            }
            detail.push(Line::from(
                format!("read {} ago", ago(pr.read_at)).fg(t.muted),
            ));
            frame.render_widget(
                Paragraph::new(detail)
                    .wrap(Wrap { trim: false })
                    .block(Block::new().padding(ratatui::widgets::Padding::new(1, 1, 1, 0))),
                area,
            );
        }
    }

    /// A pull request's row: its state in its color, its number, what the poll
    /// said, and whose it is (the item, else the branch), faint.
    fn line(pr: &WatchedPr, numbers: &HashMap<u32, u32>, width: usize, t: &Theme) -> Line<'static> {
        let fit = crate::fit;
        let whose = match (pr.item, &pr.branch) {
            (Some(i), _) => format!("#{}", numbers.get(&i).copied().unwrap_or(i)),
            (None, Some(b)) => b.clone(),
            (None, None) => String::new(),
        };
        let state = format!("{:<11}", pr.state.label());
        let num = format!("{:<7}", number(&pr.url));
        let room = width.saturating_sub(4 + state.len() + num.len() + whose.chars().count() + 2);
        Line::from(vec![
            " ".into(),
            "● ".fg(color(pr.state, t)),
            state.fg(color(pr.state, t)),
            num.fg(t.fg).bold(),
            format!("{:<room$}", fit(&pr.detail, room)).fg(t.secondary()),
            format!("  {whose}").fg(t.muted),
        ])
    }
}

/// A pull request's state in the state colors: red and conflicting are stuck,
/// green is finished, pending is working.
fn color(state: PrState, t: &Theme) -> Color {
    match state {
        PrState::Red | PrState::Conflicting => t.blocked,
        PrState::Green => t.done,
        PrState::Pending => t.working,
        PrState::Unknown => t.muted,
    }
}

/// `PR #41` from its URL, else the URL.
fn number(url: &str) -> String {
    match url.rsplit('/').next().and_then(|n| n.parse::<u32>().ok()) {
        Some(n) => format!("PR #{n}"),
        None => url.to_string(),
    }
}

fn ordinal(n: u32) -> String {
    match n {
        1 => "first".into(),
        2 => "second".into(),
        3 => "third".into(),
        n => format!("{n}th"),
    }
}

fn repo_of(project: &std::path::Path) -> String {
    project.file_name().map_or_else(
        || project.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch() -> Watch {
        let pr = |url: &str, project: &str, state: &str, order: Option<u32>| -> WatchedPr {
            serde_json::from_value(serde_json::json!({
                "url": url, "project": project, "item": 7, "branch": "feat/x", "state": state,
                "detail": format!("{state} detail"), "read_at": 1_000, "order": order,
            }))
            .unwrap()
        };
        Watch {
            prs: vec![
                pr("https://g/o/r/pull/41", "/r/api", "red", Some(2)),
                pr("https://g/o/r/pull/9", "/r/web", "green", None),
                pr("https://g/o/r/pull/40", "/r/api", "green", Some(1)),
            ],
            polled_at: Some(1_000),
            next_at: None,
            on: true,
        }
    }

    #[test]
    fn prs_group_by_repository_in_merge_order_and_open_in_the_browser() {
        let mut page = Page::default();
        page.loaded(Ok(watch()));
        let urls: Vec<&str> = page.prs().iter().map(|p| p.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://g/o/r/pull/40",
                "https://g/o/r/pull/41",
                "https://g/o/r/pull/9"
            ]
        );
        assert_eq!(page.count(), Some(3));
        assert_eq!(
            page.keys(b"j\r"),
            [Do::Open("https://g/o/r/pull/41".into())]
        );
        assert_eq!(page.keys(b"\x1b"), [Do::Close]);

        let numbers = HashMap::from([(7, 3)]);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 20)).unwrap();
        term.draw(|f| page.draw(f, &numbers, 2_000_000, &crate::theme::DRACULA))
            .unwrap();
        let screen: String = term
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        for want in [
            "api 2",
            "web 1",
            "PR #41",
            "red detail",
            "#3",
            "merge it second",
        ] {
            assert!(screen.contains(want), "lacks {want:?}");
        }
    }
}

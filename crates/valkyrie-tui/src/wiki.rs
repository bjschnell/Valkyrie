//! The repository wiki (backlog #210): `W` on the home screen. The pages agents
//! wrote about how a repository is built, stale ones marked; `↩` reads one. `s`
//! starts an agent that maps the repository (or fills the gaps), `S` one that
//! rewrites only the stale pages, each after a second key to confirm, since it
//! starts an agent.

use crate::markdown;
use crate::next_key;
use crate::search::{Search, Searched};
use crate::theme::Theme;
use ratatui::style::{self, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph};
use std::path::PathBuf;
use valkyrie_proto::{WikiListing, WikiPage};

/// What a key on the page asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Do {
    Close,
    /// Read this page in full.
    Read(String),
    /// Start the agent that writes the wiki; `stale`: only the stale pages.
    Explore {
        stale: bool,
    },
    /// Tell the user, in the bar.
    Say(String),
}

/// The open page: the repository it's for, what the daemon said, the selected
/// page, and the page being read.
#[derive(Debug)]
pub(crate) struct Page {
    pub(crate) repo: PathBuf,
    listing: Option<WikiListing>,
    failed: Option<String>,
    cursor: usize,
    /// The page read, and how far down it's scrolled.
    reading: Option<(WikiPage, usize)>,
    /// An exploration asked for, waiting on a second key: `Some(stale)`.
    confirm: Option<bool>,
    /// An exploration started and not answered yet: keys wait.
    pub(crate) starting: bool,
    /// `/`: pages whose title, slug or summary has every word typed.
    pub(crate) search: Search,
}

impl Page {
    /// On the list, no page read and nothing asked: where section keys are the
    /// app's.
    pub(crate) fn at_list(&self) -> bool {
        self.reading.is_none() && self.confirm.is_none() && !self.starting && !self.search.typing
    }

    pub(crate) fn new(repo: PathBuf) -> Self {
        Page {
            repo,
            listing: None,
            failed: None,
            cursor: 0,
            reading: None,
            confirm: None,
            starting: false,
            search: Search::default(),
        }
    }

    pub(crate) fn loaded(&mut self, got: Result<WikiListing, String>) {
        match got {
            Ok(listing) => {
                self.listing = Some(listing);
                self.failed = None;
                // Within what the search shows.
                self.cursor = self.cursor.min(self.pages().len().saturating_sub(1));
            }
            // A live reload that failed keeps the pages shown, and says so.
            Err(e) => self.failed = Some(e),
        }
    }

    pub(crate) fn read(&mut self, page: WikiPage) {
        self.reading = Some((page, 0));
    }

    /// Every page the repository's wiki has.
    fn all(&self) -> &[WikiPage] {
        self.listing.as_ref().map_or(&[], |l| &l.pages)
    }

    /// The pages listed, as far as `/` leaves them.
    fn pages(&self) -> Vec<&WikiPage> {
        self.all()
            .iter()
            .filter(|p| {
                self.search
                    .matches(&format!("{} {} {}", p.title, p.slug, p.summary))
            })
            .collect()
    }

    fn stale(&self) -> usize {
        self.all().iter().filter(|p| p.stale.is_some()).count()
    }

    /// Keys while the page is open. Listing: ↑/↓ (j/k) pick, ↩ reads, `s` maps
    /// the repository, `S` rewrites the stale pages, `/` searches, Esc or `q`
    /// closes. Reading: ↑/↓ and PgUp/PgDn scroll, Esc goes back to the list.
    pub(crate) fn keys(&mut self, mut bytes: &[u8]) -> Vec<Do> {
        let mut out = Vec::new();
        while !bytes.is_empty() {
            let key = next_key(bytes);
            let whole = key.len() == bytes.len();
            bytes = &bytes[key.len()..];
            if let Some((_, scroll)) = &mut self.reading {
                match key {
                    b"\x1b" if whole => self.reading = None,
                    b"q" => self.reading = None,
                    [0x03] => out.push(Do::Close),
                    b"k" | b"\x1b[A" | b"\x1bOA" => *scroll = scroll.saturating_sub(1),
                    b"j" | b"\x1b[B" | b"\x1bOB" => *scroll += 1,
                    b"\x1b[5~" => *scroll = scroll.saturating_sub(20),
                    b"\x1b[6~" | b" " => *scroll += 20,
                    b"g" | b"\x1b[H" | b"\x1b[1~" => *scroll = 0,
                    _ => {}
                }
                continue;
            }
            if let Some(stale) = self.confirm {
                match key {
                    b"\r" | b"\n" | b"s" | b"S" => {
                        self.confirm = None;
                        self.starting = true;
                        out.push(Do::Explore { stale });
                    }
                    [0x03] => out.push(Do::Close),
                    _ => self.confirm = None,
                }
                continue;
            }
            if !self.starting {
                match self.search.key(key, whole) {
                    Searched::No => {}
                    Searched::Kept => continue,
                    Searched::Changed => {
                        self.cursor = 0;
                        continue;
                    }
                }
            }
            let n = self.pages().len();
            match key {
                b"\x1b" if whole => out.push(Do::Close),
                b"q" | [0x03] => out.push(Do::Close),
                _ if self.starting => {}
                b"k" | b"\x1b[A" | b"\x1bOA" => self.cursor = self.cursor.saturating_sub(1),
                b"j" | b"\x1b[B" | b"\x1bOB" => {
                    self.cursor = (self.cursor + 1).min(n.saturating_sub(1))
                }
                b"g" | b"\x1b[H" | b"\x1b[1~" => self.cursor = 0,
                b"G" | b"\x1b[F" | b"\x1b[4~" => self.cursor = n.saturating_sub(1),
                b"\r" | b"\n" => {
                    if let Some(p) = self.pages().get(self.cursor) {
                        out.push(Do::Read(p.slug.clone()));
                    }
                }
                // `s` starts (spec: One key grammar): an agent that explores.
                b"s" if self.listing.is_some() => self.confirm = Some(false),
                b"S" if self.stale() > 0 => self.confirm = Some(true),
                b"S" if self.listing.is_some() => out.push(Do::Say("no page is stale".into())),
                _ => {}
            }
        }
        out
    }

    pub(crate) fn draw(&self, frame: &mut ratatui::Frame, t: &Theme) {
        let area = frame.area();
        let rect = crate::section_rect(area);
        frame.render_widget(Clear, rect);
        let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
        let block = |title: String, hints: Vec<ratatui::text::Span<'static>>, around: &str| {
            crate::section_block(t)
                .title(title.fg(t.fg).bold())
                .title_bottom(
                    crate::page_footer(hints, around, rect.width.saturating_sub(2), t)
                        .left_aligned(),
                )
        };
        let inner = rect.width.saturating_sub(4) as usize;
        let fit = crate::fit;
        let repo = self
            .repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.repo.display().to_string());

        if let Some((p, scroll)) = &self.reading {
            let width = rect.width.saturating_sub(4);
            let mut lines: Vec<Line> = vec![Line::default()];
            if !p.summary.is_empty() {
                lines.push(format!(" {}", fit(&p.summary, inner)).fg(t.muted).into());
            }
            let covers = if p.scope.is_empty() {
                "no paths".to_owned()
            } else {
                p.scope.join(", ")
            };
            lines.push(
                format!(
                    " covers {} · written at {}",
                    fit(&covers, inner.saturating_sub(30)),
                    p.commit.as_deref().unwrap_or("no commit")
                )
                .fg(t.muted)
                .into(),
            );
            if let Some(why) = &p.stale {
                lines.push(
                    format!(" stale: {}", fit(why, inner.saturating_sub(8)))
                        .fg(t.needs)
                        .into(),
                );
            }
            lines.push(Line::default());
            let body = markdown::render(&p.body, width.saturating_sub(1), t);
            lines.extend(body.lines.into_iter().map(|l| {
                let mut spans = vec![" ".into()];
                spans.extend(l.spans);
                Line::from(spans)
            }));
            let room = (rect.height as usize).saturating_sub(2);
            let scroll = (*scroll).min(lines.len().saturating_sub(room));
            let shown: Vec<Line> = lines.into_iter().skip(scroll).take(room).collect();
            let hints = [hint(" ↑↓ ", "scroll "), hint(" esc ", "back ")].concat();
            frame.render_widget(
                Paragraph::new(shown).block(block(format!(" {} ", p.title), hints, "")),
                rect,
            );
            return;
        }

        let mut top = vec![match self.search.shown() {
            true => self.search.line(t),
            false => Line::default(),
        }];
        match (&self.listing, &self.failed) {
            (_, Some(e)) => top.push(
                format!(
                    " Couldn't load the wiki: {} · trying again",
                    fit(e, inner.saturating_sub(30))
                )
                .fg(t.needs)
                .into(),
            ),
            (None, None) => top.push(" Loading…".fg(t.muted).into()),
            (Some(l), None) if l.pages.is_empty() => {
                top.push(" No wiki yet.".fg(t.fg).bold().into());
                top.push(
                    fit(
                        " s starts an agent that explores this repository and writes pages \
                         on how it's built: an overview, then one per part. Every agent \
                         started here then gets their index.",
                        inner,
                    )
                    .fg(t.muted)
                    .into(),
                );
            }
            (Some(l), None) => {
                let stale = self.stale();
                top.push(
                    format!(
                        " {} page{}{} · ↩ reads one",
                        l.pages.len(),
                        if l.pages.len() == 1 { "" } else { "s" },
                        match stale {
                            0 => String::new(),
                            n => format!(", {n} stale"),
                        }
                    )
                    .fg(t.fg)
                    .into(),
                );
            }
        }
        if let Some(stale) = self.confirm {
            let what = if stale {
                format!("Rewrite the {} stale pages?", self.stale())
            } else if self.all().is_empty() {
                "Map this repository?".to_owned()
            } else {
                "Explore again and fill the gaps?".to_owned()
            };
            top.push(Line::default());
            // Each on its own line, so a narrow panel never cuts how to answer.
            top.push(format!(" {}", fit(&what, inner)).fg(t.accent).bold().into());
            top.push(
                fit(
                    &format!(" It starts a Claude session named wiki in {repo}."),
                    inner,
                )
                .fg(t.fg)
                .into(),
            );
            top.push(Line::from(vec![
                " ↩ ".fg(t.accent).bold(),
                "starts it · any other key cancels".fg(t.muted),
            ]));
        } else if self.starting {
            top.push(Line::default());
            top.push(" Starting the agent…".fg(t.muted).into());
        }

        let mut list: Vec<Line> = Vec::new();
        let mut at_cursor = (0, 0);
        for (i, p) in self.pages().iter().enumerate() {
            let here = i == self.cursor;
            let from = list.len();
            list.push(Line::default());
            let mut head = vec![
                if here { "▌".fg(t.accent) } else { " ".into() },
                " ".into(),
                fit(&p.title, inner.saturating_sub(p.slug.len() + 12))
                    .fg(t.fg)
                    .bold(),
                format!("  {}", p.slug).fg(t.muted),
            ];
            if p.stale.is_some() {
                head.push("  stale".fg(t.needs).bold());
            }
            list.push(Line::from(head));
            let sub = match &p.stale {
                Some(why) => (format!("   stale: {why}"), t.needs),
                None => (format!("   {}", p.summary), t.muted),
            };
            list.push(fit(&sub.0, inner).fg(sub.1).into());
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

        let busy = self.starting || self.confirm.is_some();
        let mut hints = Vec::new();
        let mut around = "[ ] tab · j/k move";
        if self.search.typing {
            hints.extend(hint(" ↩ ", "keep "));
            hints.extend(hint(" esc ", "clear "));
            around = "";
        } else if !busy {
            if !self.pages().is_empty() {
                hints.extend(hint(" ↩ ", "read "));
            }
            if self.listing.is_some() {
                hints.extend(if self.all().is_empty() {
                    hint(" s ", "map it ")
                } else {
                    hint(" s ", "explore again ")
                });
            }
            if self.stale() > 0 {
                hints.extend(hint(" S ", "refresh stale "));
            }
            hints.extend(hint(" / ", "search "));
        }
        frame.render_widget(
            Paragraph::new(lines).block(block(format!(" Wiki · {repo} "), hints, around)),
            rect,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(slug: &str, stale: Option<&str>) -> WikiPage {
        WikiPage {
            slug: slug.into(),
            title: format!("The {slug}"),
            summary: format!("How {slug} works"),
            scope: vec![format!("src/{slug}")],
            commit: Some("abc1234".into()),
            updated: 0,
            stale: stale.map(Into::into),
            body: String::new(),
        }
    }

    fn listing(pages: Vec<WikiPage>) -> Result<WikiListing, String> {
        Ok(WikiListing {
            root: "/r/app".into(),
            pages,
        })
    }

    fn screen(page: &Page) -> String {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();
        term.draw(|f| page.draw(f, &crate::theme::DRACULA)).unwrap();
        term.backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn an_empty_wiki_offers_to_map_the_repository_after_a_confirm() {
        let mut p = Page::new("/r/app".into());
        // Nothing loaded yet: `s` waits for the listing.
        assert_eq!(p.keys(b"s\r"), vec![]);
        p.loaded(listing(vec![]));
        assert!(screen(&p).contains("No wiki yet"));
        assert!(screen(&p).contains("map it"));
        // `e` is edit, and the wiki is the agents' to write.
        assert_eq!(p.keys(b"e"), vec![]);
        assert!(!screen(&p).contains("Map this repository?"));
        assert_eq!(p.keys(b"s"), vec![]);
        let s = screen(&p);
        assert!(
            s.contains("Map this repository?") && s.contains("any other key cancels"),
            "{s}"
        );
        // Any other key cancels.
        assert_eq!(p.keys(b"x"), vec![]);
        assert_eq!(p.keys(b"s\r"), vec![Do::Explore { stale: false }]);
        // Waiting on the start: more keys do nothing, Esc still closes.
        assert_eq!(p.keys(b"s\r"), vec![]);
        assert_eq!(p.keys(b"\x1b"), vec![Do::Close]);
    }

    #[test]
    fn stale_pages_are_marked_and_refreshed_on_their_own() {
        let mut p = Page::new("/r/app".into());
        p.loaded(listing(vec![page("overview", None)]));
        assert_eq!(p.keys(b"S"), vec![Do::Say("no page is stale".into())]);
        p.loaded(listing(vec![
            page("overview", None),
            page("daemon", Some("400 lines changed in src/daemon")),
        ]));
        let s = screen(&p);
        for want in [
            "2 pages, 1 stale",
            "The daemon",
            "stale: 400 lines changed",
            "refresh stale",
        ] {
            assert!(s.contains(want), "{want} missing:\n{s}");
        }
        assert_eq!(p.keys(b"SS"), vec![Do::Explore { stale: true }]);
    }

    #[test]
    fn enter_reads_the_selected_page_and_esc_goes_back_to_the_list() {
        let mut p = Page::new("/r/app".into());
        p.loaded(listing(vec![page("overview", None), page("daemon", None)]));
        assert_eq!(p.keys(b"j\r"), vec![Do::Read("daemon".into())]);
        let mut full = page("daemon", None);
        full.body = "# Sessions\n\nIt runs them.".into();
        p.read(full);
        let s = screen(&p);
        assert!(
            s.contains("The daemon")
                && s.contains("It runs them")
                && s.contains("covers src/daemon"),
            "{s}"
        );
        // Esc leaves the page, not the wiki.
        assert_eq!(p.keys(b"\x1b"), vec![]);
        assert!(screen(&p).contains("2 pages"));
        assert_eq!(p.keys(b"\x1b"), vec![Do::Close]);
    }
}

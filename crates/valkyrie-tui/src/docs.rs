//! The Docs page (ADR-0021 §5, backlog #164): `D` on the home screen. The docs
//! agents wrote, agent-level ones first, then each repository's, newest first,
//! with what an agent wrote since you last looked marked unread. `↩` reads one,
//! drawn as markdown; links in it open on a click, and `c` shows its comments
//! and leaves new ones (`comments.rs`).

use std::cell::{Cell, RefCell};

use crate::backlog::fit_hints;
use crate::comments::{self, Comments};
use crate::markdown::{self, Rendered};
use crate::next_key;
use crate::theme::Theme;
use jiff::tz::TimeZone;
use ratatui::layout::{Position, Rect};
use ratatui::style::{self, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use valkyrie_proto::{Doc, DocsListing};

/// What a key on the page asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Do {
    Close,
    /// Fetch the list again.
    Load,
    /// Read the doc with this key.
    Open(String),
    /// Put this on the clipboard.
    Copy(String),
    Say(String),
    /// Fetch the comments on the doc with this key.
    Comments(String),
    /// Leave a comment on a doc.
    Comment {
        key: String,
        quote: Option<String>,
        text: String,
    },
    /// Close a comment on a doc.
    Resolve {
        key: String,
        id: u32,
    },
}

/// A line of the list: a group's heading, or a doc (its index in the listing).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    Head(String, usize),
    Doc(usize),
}

/// The doc being read, and where it's scrolled to.
#[derive(Debug, Default)]
struct Reader {
    key: String,
    doc: Option<Doc>,
    failed: Option<String>,
    top: Cell<u16>,
    /// The last `top` that still fills the area, as last drawn.
    last: Cell<Option<u16>>,
    /// Rows the text area showed last, for paging.
    rows: Cell<u16>,
    /// Where the text was drawn, and its links, for a click.
    area: Cell<Rect>,
    /// The body drawn at a width, kept until the width changes.
    drawn: RefCell<Option<(u16, Rendered)>>,
    comments: Comments,
}

/// The open page: the list, and the doc being read over it, if any.
#[derive(Debug, Default)]
pub(crate) struct Page {
    listing: Option<DocsListing>,
    failed: Option<String>,
    rows: Vec<Row>,
    /// Index among the docs (not the headings).
    cursor: usize,
    reader: Option<Reader>,
}

/// The group a doc is listed under, as the web app has it: "" for agent-level
/// (the top of the folder), the repository's folder for `repos/<name>/…`, else
/// its first folder.
fn group_of(path: &str) -> &str {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        [_] => "",
        ["repos", name, _, ..] => name,
        [first, ..] => first,
        [] => "",
    }
}

/// A doc's text without its front matter, which the header already shows.
pub(crate) fn body(text: &str) -> String {
    // As the web app's `body` does it, so both show the same text.
    let t = text.replace("\r\n", "\n");
    if !t.starts_with("---\n") {
        return t;
    }
    let Some(end) = t[3..].find("\n---").map(|e| e + 3) else {
        return t;
    };
    match t[end + 4..].find('\n').map(|n| n + end + 4) {
        Some(after) => {
            let rest = &t[after + 1..];
            rest.strip_prefix('\n').unwrap_or(rest).to_string()
        }
        None => String::new(),
    }
}

/// `created` (RFC 3339) as a short local date and time.
fn when(created: &str, tz: &TimeZone) -> String {
    created
        .parse::<jiff::Timestamp>()
        .map(|ts| ts.to_zoned(tz.clone()).strftime("%b %-d %H:%M").to_string())
        .unwrap_or_else(|_| created.to_string())
}

impl Page {
    /// The daemon's list.
    pub(crate) fn loaded(&mut self, got: Result<DocsListing, String>) {
        // Read before the rows go: `selected` walks them.
        let keep = self.selected().map(|d| d.key.clone());
        self.rows.clear();
        match got {
            Ok(listing) => {
                let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
                for (i, d) in listing.docs.iter().enumerate() {
                    let g = group_of(&d.path);
                    match groups.iter_mut().find(|(name, _)| *name == g) {
                        Some((_, list)) => list.push(i),
                        None => groups.push((g, vec![i])),
                    }
                }
                // Agent-level first, then each repository's by name.
                // Case aside, as the web app's localeCompare has it.
                groups.sort_by(|(a, _), (b, _)| {
                    (!a.is_empty(), a.to_lowercase(), *a).cmp(&(
                        !b.is_empty(),
                        b.to_lowercase(),
                        *b,
                    ))
                });
                for (name, list) in groups {
                    let head = if name.is_empty() {
                        "agent-level".to_string()
                    } else {
                        name.to_string()
                    };
                    self.rows.push(Row::Head(head, list.len()));
                    self.rows.extend(list.into_iter().map(Row::Doc));
                }
                self.listing = Some(listing);
                self.failed = None;
            }
            Err(e) => {
                self.listing = None;
                self.failed = Some(e);
            }
        }
        // A reload keeps the doc you were on.
        let n = self.docs().count();
        self.cursor = keep
            .and_then(|k| self.docs().position(|d| d.key == k))
            .unwrap_or(self.cursor)
            .min(n.saturating_sub(1));
    }

    /// The doc asked for with `Do::Open`, and what to fetch next: its comments,
    /// if it takes them (one with a `valk-id`, as on the web).
    pub(crate) fn opened(&mut self, key: &str, got: Result<Doc, String>) -> Option<Do> {
        let reader = self.reader.as_mut().filter(|r| r.key == key)?;
        *reader.drawn.borrow_mut() = None;
        match got {
            Ok(doc) => {
                let takes = doc.id > 0;
                reader.doc = Some(doc);
                takes.then(|| Do::Comments(key.to_string()))
            }
            Err(e) => {
                reader.failed = Some(e);
                None
            }
        }
    }

    /// The comments on the doc with this key, fetched, or as they are after a
    /// comment was left or closed (`sent`).
    pub(crate) fn comments(
        &mut self,
        key: &str,
        got: Result<Vec<valkyrie_proto::DocComment>, String>,
        sent: bool,
    ) {
        let Some(reader) = self.reader.as_mut().filter(|r| r.key == key) else {
            return;
        };
        match got {
            Ok(list) if sent => reader.comments.sent(list),
            got => reader.comments.loaded(got),
        }
    }

    fn docs(&self) -> impl Iterator<Item = &Doc> {
        let docs = self
            .listing
            .as_ref()
            .map(|l| l.docs.as_slice())
            .unwrap_or_default();
        self.rows.iter().filter_map(|r| match r {
            Row::Doc(i) => docs.get(*i),
            Row::Head(..) => None,
        })
    }

    fn selected(&self) -> Option<&Doc> {
        self.docs().nth(self.cursor)
    }

    /// Whether a doc is open over the list.
    #[cfg(test)]
    pub(crate) fn reading(&self) -> bool {
        self.reader.is_some()
    }

    /// Keys while the page is open.
    pub(crate) fn keys(&mut self, mut bytes: &[u8]) -> Vec<Do> {
        let mut out = Vec::new();
        while !bytes.is_empty() {
            let key = next_key(bytes);
            let whole = key.len() == bytes.len();
            bytes = &bytes[key.len()..];
            if self.reader.is_some() {
                self.reader_key(key, whole, &mut out);
            } else {
                self.list_key(key, whole, &mut out);
            }
        }
        out
    }

    /// ↑/↓ (j/k) pick a doc, ↩ reads it; `r` loads again; Esc or `q` closes.
    fn list_key(&mut self, key: &[u8], whole: bool, out: &mut Vec<Do>) {
        let n = self.docs().count();
        match key {
            b"k" | b"\x1b[A" | b"\x1bOA" => self.cursor = self.cursor.saturating_sub(1),
            b"j" | b"\x1b[B" | b"\x1bOB" => {
                self.cursor = (self.cursor + 1).min(n.saturating_sub(1))
            }
            b"g" | b"\x1b[H" | b"\x1b[1~" => self.cursor = 0,
            b"G" | b"\x1b[F" | b"\x1b[4~" => self.cursor = n.saturating_sub(1),
            b"\r" | b"\n" => {
                if let Some(d) = self.selected() {
                    if d.key.is_empty() {
                        out.push(Do::Say("that doc can't be opened from here".into()));
                    } else {
                        let key = d.key.clone();
                        self.reader = Some(Reader {
                            key: key.clone(),
                            ..Default::default()
                        });
                        out.push(Do::Open(key));
                    }
                }
            }
            b"r" => out.push(Do::Load),
            b"\x1b" if whole => out.push(Do::Close),
            b"q" | [0x03] => out.push(Do::Close),
            _ => {}
        }
    }

    /// ↑/↓ (j/k) scroll a row, space/PgDn and b/PgUp a page, g/G to the ends;
    /// `y` copies the doc's markdown; `r` reads it again; Esc or `q` goes back.
    fn reader_key(&mut self, key: &[u8], whole: bool, out: &mut Vec<Do>) {
        if let Some(reader) = &mut self.reader
            && reader.comments.is_open()
        {
            let body = body(
                reader
                    .doc
                    .as_ref()
                    .and_then(|d| d.text.as_deref())
                    .unwrap_or_default(),
            );
            let doc = reader.key.clone();
            match reader.comments.key(key, whole, &body) {
                Some(comments::Do::Send { quote, text }) => out.push(Do::Comment {
                    key: doc,
                    quote,
                    text,
                }),
                Some(comments::Do::Resolve(id)) => out.push(Do::Resolve { key: doc, id }),
                None => {}
            }
            return;
        }
        let Some(reader) = &mut self.reader else {
            return;
        };
        let page = i32::from(reader.rows.get().saturating_sub(1).max(1));
        match key {
            b"k" | b"\x1b[A" | b"\x1bOA" => reader.scroll(-1),
            b"j" | b"\x1b[B" | b"\x1bOB" => reader.scroll(1),
            b" " | b"\x1b[6~" => reader.scroll(page),
            b"b" | b"\x1b[5~" => reader.scroll(-page),
            b"g" | b"\x1b[H" | b"\x1b[1~" => reader.top.set(0),
            b"G" | b"\x1b[F" | b"\x1b[4~" => reader.scroll(i32::from(u16::MAX)),
            b"c" => match &reader.doc {
                Some(d) if d.id > 0 => reader.comments.open(),
                Some(_) => out.push(Do::Say(
                    "it takes comments once it has a valk-id: an agent reading it gives it one"
                        .into(),
                )),
                None => {}
            },
            b"y" => match &reader.doc {
                Some(d) => out.push(Do::Copy(body(d.text.as_deref().unwrap_or_default()))),
                None => out.push(Do::Say("nothing to copy".into())),
            },
            b"r" => {
                let key = reader.key.clone();
                self.reader = Some(Reader {
                    key: key.clone(),
                    ..Default::default()
                });
                out.push(Do::Open(key));
            }
            b"\x1b" if whole => self.reader = None,
            b"q" | [0x03] => self.reader = None,
            _ => {}
        }
    }

    /// The wheel over the doc being read scrolls it.
    pub(crate) fn wheel(&self, down: bool, rows: i32) {
        if let Some(r) = &self.reader {
            r.scroll(if down { rows } else { -rows });
        }
    }

    /// The link at screen cell (`x`, `y`) of the doc being read, if any.
    pub(crate) fn link_at(&self, x: u16, y: u16) -> Option<String> {
        let r = self.reader.as_ref()?;
        let area = r.area.get();
        if !area.contains(Position::new(x, y)) {
            return None;
        }
        let row = usize::from(y - area.y) + usize::from(r.top.get());
        let col = x - area.x;
        let drawn = r.drawn.borrow();
        let (_, body) = drawn.as_ref()?;
        body.links
            .iter()
            .find(|l| l.row == row && (l.from..l.to).contains(&col))
            .map(|l| l.url.clone())
    }

    pub(crate) fn draw(&self, frame: &mut ratatui::Frame, t: &Theme) {
        let area = frame.area();
        // Nearly the whole screen, like the backlog and digest pages.
        let rect = Rect::new(
            area.x + 2,
            area.y + 1,
            area.width.saturating_sub(4),
            area.height.saturating_sub(2),
        )
        .intersection(area);
        frame.render_widget(Clear, rect);
        match &self.reader {
            Some(r) => self.draw_reader(frame, rect, r, t),
            None => self.draw_list(frame, rect, t),
        }
    }

    fn draw_list(&self, frame: &mut ratatui::Frame, rect: Rect, t: &Theme) {
        let inner = rect.width.saturating_sub(4) as usize;
        let tz = TimeZone::system();
        let mut top = vec![Line::default()];
        match (&self.listing, &self.failed) {
            (_, Some(e)) => top.push(
                format!(" Couldn't load the docs: {}", fit(e, inner))
                    .fg(t.needs)
                    .into(),
            ),
            (None, None) => top.push(" Loading…".fg(t.muted).into()),
            (Some(l), None) => {
                let unread = match l.unread {
                    0 => String::new(),
                    n => format!(" · {n} new since you last looked"),
                };
                let about = format!(
                    "{} in {}{unread}",
                    plural(l.docs.len(), "doc"),
                    l.root.display()
                );
                top.push(format!(" {}", fit(&about, inner)).fg(t.fg).into());
                if l.docs.is_empty() {
                    top.push(Line::default());
                    top.push(
                        " No docs yet. Agents write them with `valk doc write`."
                            .fg(t.muted)
                            .into(),
                    );
                }
            }
        }

        let docs = self
            .listing
            .as_ref()
            .map(|l| l.docs.as_slice())
            .unwrap_or_default();
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
                Row::Doc(at) => {
                    let Some(d) = docs.get(*at) else { continue };
                    let here = i == self.cursor;
                    let kind = d.kind.as_str();
                    let mut first = vec![
                        if here { "▌".fg(t.accent) } else { " ".into() },
                        if d.unread {
                            "● ".fg(t.accent).bold()
                        } else {
                            "  ".into()
                        },
                        format!("{kind:<9}").fg(t.muted),
                    ];
                    let room = inner.saturating_sub(13);
                    let title = fit(&d.title, room);
                    first.push(if d.unread {
                        title.fg(t.fg).bold()
                    } else {
                        title.fg(t.fg)
                    });
                    let mut sub = vec![d.by.clone(), when(&d.created, &tz)];
                    if let Some(s) = &d.session {
                        sub.push(format!("session {s}"));
                    }
                    if let Some(n) = d.item {
                        sub.push(format!("#{n}"));
                    }
                    let sub = sub
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>();
                    let mut second =
                        Line::from(format!("{:13}{}", "", fit(&sub.join(" · "), room)).fg(t.muted));
                    let mut first = Line::from(first);
                    if here {
                        let bg = style::Style::new().bg(t.selection);
                        first = first.style(bg);
                        second = second.style(bg);
                        at_cursor = (list.len(), list.len() + 2);
                    }
                    list.push(first);
                    list.push(second);
                    i += 1;
                }
            }
        }
        let room = (rect.height as usize).saturating_sub(top.len() + 2);
        let skip = at_cursor.1.saturating_sub(room).min(at_cursor.0);
        let mut lines = top;
        lines.extend(list.into_iter().skip(skip).take(room));

        let hints = [
            hint(" ↩ ", "read ", t),
            hint(" r ", "reload ", t),
            hint(" esc ", "close ", t),
        ]
        .concat();
        let hints = fit_hints(hints, inner);
        let block = page_block(" Docs ".into(), hints, t);
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn draw_reader(&self, frame: &mut ratatui::Frame, rect: Rect, r: &Reader, t: &Theme) {
        let listed = self.docs().find(|d| d.key == r.key);
        let doc = r.doc.as_ref().or(listed);
        let title = doc.map_or_else(|| "Doc".to_string(), |d| d.title.clone());
        let takes = doc.is_some_and(|d| d.id > 0);
        let open = r.comments.open_count();
        let hints = [
            hint(" j/k ", "scroll ", t),
            hint(" space ", "page ", t),
            if takes {
                hint(
                    " c ",
                    if open > 0 {
                        "comments ●"
                    } else {
                        "comments "
                    },
                    t,
                )
            } else {
                Vec::new()
            },
            hint(" y ", "copy ", t),
            hint(" r ", "reload ", t),
            hint(" esc ", "back ", t),
        ]
        .concat();
        let inner_w = rect.width.saturating_sub(4);
        let hints = fit_hints(hints, inner_w as usize);
        let block = page_block(format!(" {} ", fit(&title, inner_w as usize)), hints, t);
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        // A column of room each side, as the list has.
        let inner = Rect::new(
            inner.x + 1,
            inner.y,
            inner.width.saturating_sub(2),
            inner.height,
        );

        let mut head: Vec<Line> = vec![Line::default()];
        if let Some(d) = doc {
            let tz = TimeZone::system();
            let mut about = vec![d.kind.as_str().to_string(), format!("by {}", d.by)];
            about.push(when(&d.created, &tz));
            if let Some(s) = &d.session {
                about.push(format!("session {s}"));
            }
            if let Some(n) = d.item {
                about.push(format!("#{n}"));
            }
            about.push(d.path.clone());
            if open > 0 {
                about.push(plural(open, "open comment"));
            }
            head.push(
                fit(&about.join(" · "), inner.width as usize)
                    .fg(t.muted)
                    .into(),
            );
            for e in &d.edited {
                head.push(
                    fit(&format!("edited {e}"), inner.width as usize)
                        .fg(t.muted)
                        .into(),
                );
            }
            head.push(Line::default());
        }
        let text_area = Rect::new(
            inner.x,
            inner.y + head.len() as u16,
            inner.width,
            inner.height.saturating_sub(head.len() as u16),
        )
        .intersection(inner);
        frame.render_widget(Paragraph::new(head), inner);

        let body_text = match (&r.doc, &r.failed) {
            (_, Some(e)) => {
                let line = format!("Couldn't read it: {e}");
                frame.render_widget(Paragraph::new(line.fg(t.needs)), text_area);
                return;
            }
            (None, None) => {
                frame.render_widget(Paragraph::new("Loading…".fg(t.muted)), text_area);
                return;
            }
            (Some(d), None) => body(d.text.as_deref().unwrap_or_default()),
        };
        let width = text_area.width;
        {
            let mut drawn = r.drawn.borrow_mut();
            if drawn.as_ref().is_none_or(|(w, _)| *w != width) {
                *drawn = Some((width, markdown::render(&body_text, width, t)));
            }
        }
        let drawn = r.drawn.borrow();
        let Some((_, rendered)) = drawn.as_ref() else {
            return;
        };
        let total = rendered.lines.len() as u16;
        let last = total.saturating_sub(text_area.height);
        r.last.set(Some(last));
        r.top.set(r.top.get().min(last));
        r.rows.set(text_area.height);
        r.area.set(text_area);
        let shown: Vec<Line> = rendered
            .lines
            .iter()
            .skip(usize::from(r.top.get()))
            .take(usize::from(text_area.height))
            .cloned()
            .collect();
        frame.render_widget(Paragraph::new(shown), text_area);
        if r.top.get() < last && text_area.height > 0 {
            let more = format!(" ↓ {} more rows ", last - r.top.get());
            let w = (more.chars().count() as u16).min(text_area.width);
            let row = Rect::new(text_area.right() - w, text_area.bottom() - 1, w, 1);
            frame.render_widget(
                Paragraph::new(more.fg(t.accent2).bold()).style(style::Style::new().bg(t.panel)),
                row,
            );
        }
        r.comments.draw(frame, rect, t);
    }
}

impl Reader {
    fn scroll(&self, rows: i32) {
        // Clamped now, not at the next draw, so a click in the same read finds
        // the row that will show.
        let last = i32::from(self.last.get().unwrap_or(u16::MAX));
        let top = (i32::from(self.top.get()) + rows).clamp(0, last);
        self.top.set(top as u16);
    }
}

fn hint(k: &'static str, w: &'static str, t: &Theme) -> Vec<ratatui::text::Span<'static>> {
    vec![k.fg(t.accent).bold(), w.fg(t.muted)]
}

fn page_block(
    title: String,
    hints: Vec<ratatui::text::Span<'static>>,
    t: &Theme,
) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style::Style::new().fg(t.accent))
        .title(title.fg(t.fg).bold())
        .title_bottom(Line::from(hints).centered())
        .style(style::Style::new().bg(t.panel).fg(t.fg))
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

fn fit(text: &str, max: usize) -> String {
    crate::fit(text, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::DocKind;

    fn doc(key: &str, path: &str, unread: bool) -> Doc {
        Doc {
            id: key.parse().unwrap_or(0),
            kind: DocKind::Plan,
            title: format!("title {key}"),
            path: path.into(),
            key: key.into(),
            unread,
            by: "claude".into(),
            created: "2026-11-09T22:00:00Z".into(),
            session: Some("docs-pass".into()),
            commit: None,
            item: Some(164),
            repo: None,
            edited: vec![],
            hash: None,
            text: None,
        }
    }

    fn listing(docs: Vec<Doc>) -> DocsListing {
        DocsListing {
            docs,
            root: "/home/u/vault/valk".into(),
            unread: 1,
            at: 100,
        }
    }

    fn screen(page: &Page, w: u16, h: u16) -> String {
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| page.draw(f, &crate::theme::DRACULA)).unwrap();
        let buf = term.backend().buffer();
        buf.content
            .chunks(buf.area.width as usize)
            .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn groups_agent_level_first_then_repositories_by_name() {
        assert_eq!(group_of("2026-11-09-a.md"), "");
        assert_eq!(group_of("repos/valkyrie/2026-11-09-b.md"), "valkyrie");
        assert_eq!(group_of("notes/c.md"), "notes");
        let mut page = Page::default();
        page.loaded(Ok(listing(vec![
            doc("1", "repos/zeta/a.md", false),
            doc("2", "repos/alpha/b.md", true),
            doc("3", "c.md", false),
            doc("4", "repos/zeta/d.md", false),
        ])));
        let heads: Vec<_> = page
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Head(h, n) => Some((h.as_str(), *n)),
                Row::Doc(_) => None,
            })
            .collect();
        assert_eq!(heads, [("agent-level", 1), ("alpha", 1), ("zeta", 2)]);
        let order: Vec<_> = page.docs().map(|d| d.key.as_str()).collect();
        assert_eq!(order, ["3", "2", "1", "4"]);
    }

    #[test]
    fn reads_a_doc_and_goes_back_to_the_list() {
        let mut page = Page::default();
        page.loaded(Ok(listing(vec![
            doc("1", "a.md", false),
            doc("2", "b.md", true),
        ])));
        assert_eq!(page.keys(b"j\r"), vec![Do::Open("2".into())]);
        assert!(page.reading());
        let mut read = doc("2", "b.md", true);
        read.text = Some("---\nvalk-id: 2\n---\n\n# Plan\n\nSome **words**.\n".into());
        page.opened("2", Ok(read));
        let shown = screen(&page, 80, 20);
        assert!(shown.contains("title 2"), "{shown}");
        assert!(shown.contains("Some words."), "{shown}");
        assert!(
            !shown.contains("valk-id") && !shown.contains("**"),
            "{shown}"
        );
        assert_eq!(
            page.keys(b"y"),
            vec![Do::Copy("# Plan\n\nSome **words**.\n".into())]
        );
        assert_eq!(page.keys(b"\x1b"), vec![]);
        assert!(!page.reading());
        assert_eq!(page.keys(b"\x1b"), vec![Do::Close]);
    }

    #[test]
    fn an_answer_for_another_doc_is_dropped() {
        let mut page = Page::default();
        page.loaded(Ok(listing(vec![
            doc("1", "a.md", false),
            doc("2", "b.md", false),
        ])));
        page.keys(b"\r");
        page.keys(b"\x1b");
        page.keys(b"j\r");
        page.opened("1", Ok(doc("1", "a.md", false)));
        assert!(page.reader.as_ref().unwrap().doc.is_none());
    }

    #[test]
    fn draws_the_list_with_unread_marks() {
        let mut page = Page::default();
        page.loaded(Ok(listing(vec![
            doc("1", "a.md", false),
            doc("2", "repos/valkyrie/b.md", true),
        ])));
        let shown = screen(&page, 80, 20);
        for want in [
            "Docs",
            "agent-level",
            "valkyrie",
            "● plan",
            "title 2",
            "claude",
            "#164",
            "1 new",
        ] {
            assert!(shown.contains(want), "{want} missing:\n{shown}");
        }
    }

    #[test]
    fn scrolls_a_long_doc_and_finds_its_links() {
        let mut page = Page::default();
        page.loaded(Ok(listing(vec![doc("1", "a.md", false)])));
        page.keys(b"\r");
        let mut read = doc("1", "a.md", false);
        let mut text: String = (0..60).map(|i| format!("line {i}\n\n")).collect();
        text.push_str("[x](https://x.dev)\n");
        read.text = Some(text);
        page.opened("1", Ok(read));
        let shown = screen(&page, 60, 20);
        assert!(shown.contains("more rows"), "{shown}");
        page.keys(b"G");
        let shown = screen(&page, 60, 20);
        assert!(shown.contains("x https://x.dev"), "{shown}");
        let r = page.reader.as_ref().unwrap();
        let area = r.area.get();
        let y = (area.y..area.bottom())
            .find(|&y| page.link_at(area.x, y).is_some())
            .expect("a link row");
        assert_eq!(page.link_at(area.x, y).as_deref(), Some("https://x.dev"));
        assert_eq!(page.link_at(area.x + 40, y), None);
    }

    #[test]
    fn a_reload_keeps_the_doc_you_were_on() {
        let mut page = Page::default();
        page.loaded(Ok(listing(vec![
            doc("1", "a.md", false),
            doc("2", "b.md", false),
        ])));
        page.keys(b"j");
        assert_eq!(page.selected().unwrap().key, "2");
        page.loaded(Ok(listing(vec![
            doc("3", "new.md", true),
            doc("1", "a.md", false),
            doc("2", "b.md", false),
        ])));
        assert_eq!(page.selected().unwrap().key, "2");
    }

    #[test]
    fn groups_sort_without_case() {
        let mut page = Page::default();
        page.loaded(Ok(listing(vec![
            doc("1", "repos/Zeta/a.md", false),
            doc("2", "repos/alpha/b.md", false),
        ])));
        assert_eq!(page.rows[0], Row::Head("alpha".into(), 1));
    }

    #[test]
    fn scrolling_past_the_end_stops_there_before_the_next_draw() {
        let mut page = Page::default();
        page.loaded(Ok(listing(vec![doc("1", "a.md", false)])));
        page.keys(b"\r");
        let mut read = doc("1", "a.md", false);
        read.text = Some((0..60).map(|i| format!("line {i}\n\n")).collect());
        page.opened("1", Ok(read));
        screen(&page, 60, 20);
        page.keys(b"G");
        page.wheel(true, 3);
        let r = page.reader.as_ref().unwrap();
        assert_eq!(Some(r.top.get()), r.last.get());
    }

    #[test]
    fn a_doc_with_an_id_takes_comments() {
        let mut page = Page::default();
        page.loaded(Ok(listing(vec![doc("7", "a.md", false)])));
        page.keys(b"\r");
        let mut read = doc("7", "a.md", false);
        read.text = Some("---\nvalk-id: 7\n---\n\n# Plan\n\nStep one.\n".into());
        assert_eq!(page.opened("7", Ok(read)), Some(Do::Comments("7".into())));
        page.comments("7", Ok(vec![]), false);
        // c, n, past the whole doc and the heading to the paragraph, ↩, type, ↩.
        let got = page.keys(b"cnjj\rok\r");
        assert_eq!(
            got,
            vec![Do::Comment {
                key: "7".into(),
                quote: Some("Step one.".into()),
                text: "ok".into()
            }]
        );
        // Keys go to the panel while it's open, then back to the reader.
        page.comments("7", Ok(vec![]), true);
        assert_eq!(page.keys(b"\x1b"), vec![]);
        assert!(page.reading());
        assert_eq!(page.keys(b"\x1b"), vec![]);
        assert!(!page.reading());
    }

    #[test]
    fn a_doc_without_an_id_says_why_it_takes_none() {
        let mut page = Page::default();
        page.loaded(Ok(listing(vec![doc(
            "p0123456789abcdef",
            "mine.md",
            false,
        )])));
        page.keys(b"\r");
        let mut read = doc("p0123456789abcdef", "mine.md", false);
        read.id = 0;
        assert_eq!(page.opened("p0123456789abcdef", Ok(read)), None);
        let got = page.keys(b"c");
        assert!(
            matches!(got.as_slice(), [Do::Say(s)] if s.contains("valk-id")),
            "{got:?}"
        );
    }

    #[test]
    fn strips_front_matter_only_when_it_is_there() {
        assert_eq!(body("---\na: 1\n---\n\nhi\n"), "hi\n");
        // An empty block is all that goes; a rule further down stays.
        assert_eq!(
            body("---\n---\n# T\n\nx\n\n---\n\ny"),
            "# T\n\nx\n\n---\n\ny"
        );
        // One blank line after it goes, as on the web; more stay.
        assert_eq!(body("---\na: 1\n---\n\n\nhi"), "\nhi");
        assert_eq!(body("no front matter\n"), "no front matter\n");
        assert_eq!(body("---\nunclosed\n"), "---\nunclosed\n");
    }
}

//! The Docs page (ADR-0021 §5, backlog #164): `D` on the home screen. The docs
//! agents wrote, agent-level ones first, then each repository's, newest first,
//! with what an agent wrote since you last looked marked unread. `↩` reads one,
//! drawn as markdown; links in it open on a click, and `c` shows its comments
//! and leaves new ones (`comments.rs`); `e` edits it in your editor, and a
//! save that finds it changed shows both versions (`conflict.rs`). `h` shows its
//! past versions to restore (`history.rs`); `d d` deletes it, `u` on the list
//! undoes that, and `t` there shows Recently deleted to put back.

use std::cell::{Cell, RefCell};

use crate::backlog::fit_hints;
use crate::comments::{self, Comments};
use crate::conflict::{Choice, Conflict};
use crate::history::{self, History};
use crate::markdown::{self, Rendered};
use crate::next_key;
use crate::theme::Theme;
use jiff::tz::TimeZone;
use ratatui::layout::{Position, Rect};
use ratatui::style::{self, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use valkyrie_proto::{DeletedDoc, Doc, DocVersion, DocsListing};

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
    /// Edit `body` in your editor, then save it over the version `base` names.
    Edit {
        key: String,
        base: String,
        body: String,
        title: String,
    },
    /// Save `body` over the version `base` names.
    Save {
        key: String,
        base: String,
        body: String,
    },
    /// Fetch a doc's past versions.
    History(String),
    /// Fetch one past version's text.
    Peek {
        key: String,
        version: String,
    },
    /// Put a past version back over the version `base` names.
    Restore {
        key: String,
        base: String,
        version: String,
    },
    /// Delete a doc, if it's still the version `base` names.
    Delete {
        key: String,
        base: String,
        title: String,
    },
    /// Put a deleted doc back.
    Undelete(String),
    /// Fetch the docs that can be put back.
    Deleted,
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
    /// A save found it changed: yours and theirs, until you choose.
    conflict: Option<Conflict>,
    /// Its past versions, while `h` has them open.
    history: Option<History>,
    /// `d` was pressed once: another deletes it.
    deleting: bool,
}

/// Docs you deleted that can still be put back.
#[derive(Debug, Default)]
struct Trash {
    docs: Option<Vec<DeletedDoc>>,
    failed: Option<String>,
    cursor: usize,
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
    /// The doc you just deleted, by its token and title, for `u`.
    undo: Option<(String, String)>,
    /// Recently deleted, while `t` has it open.
    trash: Option<Trash>,
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
            // A send that failed: the comment stays in its box, to send again.
            Err(_) if sent => reader.comments.unsent(),
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

    /// A save went through: the reader shows the doc as written. One of your own
    /// docs gets an id, and so a new key, the first time it's saved.
    pub(crate) fn saved(&mut self, key: &str, doc: Doc) {
        let Some(reader) = self.reader.as_mut().filter(|r| r.key == key) else {
            return;
        };
        if !doc.key.is_empty() {
            reader.key = doc.key.clone();
        }
        reader.doc = Some(doc);
        reader.conflict = None;
        *reader.drawn.borrow_mut() = None;
    }

    /// A save found the doc changed since it was opened; nothing was written.
    /// Whether it's shown: not when you've gone on to another doc.
    pub(crate) fn conflict(&mut self, key: &str, mine: String, theirs: Doc) -> bool {
        let Some(reader) = self.reader.as_mut().filter(|r| r.key == key) else {
            return false;
        };
        reader.conflict = Some(Conflict { mine, theirs });
        true
    }

    /// A doc's past versions, fetched.
    pub(crate) fn history(&mut self, key: &str, got: Result<Vec<DocVersion>, String>) {
        if let Some(h) = self.reader_for(key).and_then(|r| r.history.as_mut()) {
            h.loaded(got);
        }
    }

    /// One past version's text, fetched.
    pub(crate) fn peeked(&mut self, key: &str, version: &str, text: String) {
        if let Some(h) = self.reader_for(key).and_then(|r| r.history.as_mut()) {
            h.peeked(version, text);
        }
    }

    /// A restore or delete found the doc changed since it was opened: the reader
    /// shows it as it is now.
    pub(crate) fn changed(&mut self, key: &str, doc: Doc) {
        if let Some(r) = self.reader_for(key) {
            r.history = None;
        }
        self.saved(key, doc);
    }

    /// The doc was deleted: back to the list, with `u` to put it back.
    pub(crate) fn deleted(&mut self, key: &str, token: String, title: String) {
        if self.reader.as_ref().is_some_and(|r| r.key == key) {
            self.reader = None;
        }
        self.undo = Some((token, title));
    }

    /// Recently deleted, fetched.
    pub(crate) fn trash(&mut self, got: Result<Vec<DeletedDoc>, String>) {
        if let Some(t) = &mut self.trash {
            match got {
                Ok(docs) => {
                    t.cursor = t.cursor.min(docs.len().saturating_sub(1));
                    t.docs = Some(docs);
                    t.failed = None;
                }
                Err(e) => t.failed = Some(e),
            }
        }
    }

    /// A doc was put back: it opens, as on the web.
    pub(crate) fn undeleted(&mut self, token: &str, doc: &Doc) -> Option<Do> {
        if self.undo.as_ref().is_some_and(|(t, _)| t == token) {
            self.undo = None;
        }
        self.trash = None;
        if doc.key.is_empty() {
            return None;
        }
        self.reader = Some(Reader {
            key: doc.key.clone(),
            ..Default::default()
        });
        Some(Do::Open(doc.key.clone()))
    }

    fn reader_for(&mut self, key: &str) -> Option<&mut Reader> {
        self.reader.as_mut().filter(|r| r.key == key)
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
        if let Some(trash) = &mut self.trash {
            let n = trash.docs.as_ref().map_or(0, Vec::len);
            match key {
                b"k" | b"\x1b[A" | b"\x1bOA" => trash.cursor = trash.cursor.saturating_sub(1),
                b"j" | b"\x1b[B" | b"\x1bOB" => {
                    trash.cursor = (trash.cursor + 1).min(n.saturating_sub(1))
                }
                b"\r" | b"\n" => {
                    if let Some(d) = trash.docs.as_ref().and_then(|d| d.get(trash.cursor)) {
                        out.push(Do::Undelete(d.token.clone()));
                    }
                }
                b"\x1b" if whole => self.trash = None,
                b"q" | [0x03] => self.trash = None,
                _ => {}
            }
            return;
        }
        let n = self.docs().count();
        match key {
            b"u" => match &self.undo {
                Some((token, _)) => out.push(Do::Undelete(token.clone())),
                None => out.push(Do::Say("nothing to undo".into())),
            },
            b"t" => {
                self.trash = Some(Trash::default());
                out.push(Do::Deleted);
            }
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
            && let Some(c) = &reader.conflict
        {
            let doc = reader.key.clone();
            let base = c.theirs.hash.clone();
            match c.key(key) {
                // Without theirs' hash there's nothing to save over.
                Some(Choice::Edit | Choice::Mine) if base.is_none() => {
                    out.push(Do::Say("can't save over it: t, then edit again".into()))
                }
                Some(Choice::Theirs) => {
                    let theirs = c.theirs.clone();
                    self.saved(&doc, theirs);
                }
                Some(Choice::Edit) => {
                    let title = c.theirs.title.clone();
                    let body = c.mine.clone();
                    out.push(Do::Edit {
                        key: doc,
                        base: base.unwrap_or_default(),
                        body,
                        title,
                    });
                }
                Some(Choice::Mine) => {
                    let body = c.mine.clone();
                    out.push(Do::Save {
                        key: doc,
                        base: base.unwrap_or_default(),
                        body,
                    });
                }
                // Yours isn't saved anywhere else: no key leaves without a choice.
                None if matches!(key, b"\x1b" | b"q") => out.push(Do::Say(
                    "yours isn't saved: t takes theirs, e edits yours again, s saves yours over it"
                        .into(),
                )),
                None => {}
            }
            return;
        }
        if let Some(reader) = &mut self.reader
            && let Some(h) = &mut reader.history
        {
            let (open, act) = h.key(key, whole);
            if !open {
                reader.history = None;
            }
            match act {
                Some(history::Do::Peek(version)) => out.push(Do::Peek {
                    key: reader.key.clone(),
                    version,
                }),
                Some(history::Do::Restore(version)) => {
                    match reader.doc.as_ref().and_then(|d| d.hash.clone()) {
                        Some(base) => out.push(Do::Restore {
                            key: reader.key.clone(),
                            base,
                            version,
                        }),
                        None => out.push(Do::Say("it can't be restored from here".into())),
                    }
                }
                None => {}
            }
            return;
        }
        // `d` once asks; the next key deletes it only if it's `d` again.
        if let Some(reader) = &mut self.reader
            && std::mem::take(&mut reader.deleting)
        {
            if key == b"d" {
                if let Some(d) = &reader.doc
                    && let Some(base) = d.hash.clone()
                {
                    out.push(Do::Delete {
                        key: reader.key.clone(),
                        base,
                        title: if d.title.is_empty() {
                            d.path.clone()
                        } else {
                            d.title.clone()
                        },
                    });
                }
            } else {
                out.push(Do::Say("not deleted".into()));
            }
            return;
        }
        if let Some(reader) = &mut self.reader
            && reader.comments.is_open()
        {
            let text = &reader.doc;
            let body = || {
                body(
                    text.as_ref()
                        .and_then(|d| d.text.as_deref())
                        .unwrap_or_default(),
                )
            };
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
            b"h" => match &reader.doc {
                Some(d) if d.id > 0 => {
                    reader.history = Some(History::default());
                    out.push(Do::History(reader.key.clone()));
                }
                Some(_) => out.push(Do::Say(
                    "it has a history once it has a valk-id: an agent reading it gives it one"
                        .into(),
                )),
                None => {}
            },
            b"d" => match &reader.doc {
                Some(d) if d.hash.is_some() => {
                    reader.deleting = true;
                    out.push(Do::Say(
                        "d again deletes it (it can be put back); any other key doesn't".into(),
                    ));
                }
                Some(_) => out.push(Do::Say("it can't be deleted from here".into())),
                None => {}
            },
            b"c" => match &reader.doc {
                Some(d) if d.id > 0 => reader.comments.open(),
                Some(_) => out.push(Do::Say(
                    "it takes comments once it has a valk-id: an agent reading it gives it one"
                        .into(),
                )),
                None => {}
            },
            b"e" => match &reader.doc {
                Some(d) if d.hash.is_some() && d.text.is_some() => out.push(Do::Edit {
                    key: reader.key.clone(),
                    base: d.hash.clone().unwrap_or_default(),
                    body: body(d.text.as_deref().unwrap_or_default()),
                    title: d.title.clone(),
                }),
                Some(_) => out.push(Do::Say("it can't be edited from here".into())),
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
        if let Some(r) = self.reader.as_ref().filter(|r| !r.covered()) {
            r.scroll(if down { rows } else { -rows });
        }
    }

    /// A mouse press: a `d` waiting for its second press is off.
    pub(crate) fn pressed(&mut self) {
        if let Some(r) = &mut self.reader {
            r.deleting = false;
        }
    }

    /// Putting `token` back failed: it's gone (put back elsewhere, or expired),
    /// so `u` stops offering it.
    pub(crate) fn undo_failed(&mut self, token: &str) {
        if self.undo.as_ref().is_some_and(|(t, _)| t == token) {
            self.undo = None;
        }
    }

    /// The link at screen cell (`x`, `y`) of the doc being read, if any.
    pub(crate) fn link_at(&self, x: u16, y: u16) -> Option<String> {
        // Not through a panel over the doc: its links are hidden.
        let r = self.reader.as_ref().filter(|r| !r.covered())?;
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
                if let Some((_, title)) = &self.undo {
                    let line = format!(" Deleted “{title}” · u puts it back");
                    top.push(fit(&line, inner).fg(t.accent2).into());
                }
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
            if self.undo.is_some() {
                hint(" u ", "undo ", t)
            } else {
                Vec::new()
            },
            hint(" t ", "recently deleted ", t),
            hint(" r ", "reload ", t),
            hint(" esc ", "close ", t),
        ]
        .concat();
        let hints = fit_hints(hints, inner);
        let block = page_block(" Docs ".into(), hints, t);
        frame.render_widget(Paragraph::new(lines).block(block), rect);
        if let Some(trash) = &self.trash {
            draw_trash(frame, rect, trash, t);
        }
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
            hint(" e ", "edit ", t),
            if takes {
                hint(" h ", "history ", t)
            } else {
                Vec::new()
            },
            hint(" d ", "delete ", t),
            hint(" y ", "copy ", t),
            hint(" r ", "reload ", t),
            hint(" esc ", "back ", t),
        ]
        .concat();
        let hints = if r.conflict.is_some() {
            [
                hint(" t ", "use theirs ", t),
                hint(" e ", "keep editing ", t),
                hint(" s ", "save mine over it ", t),
            ]
            .concat()
        } else {
            hints
        };
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

        if let Some(c) = &r.conflict {
            c.draw(frame, text_area, t);
            return;
        }
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
        if let Some(h) = &r.history {
            h.draw(frame, rect, t);
        }
    }
}

/// Recently deleted, over the list: each with where it was and when it went.
fn draw_trash(frame: &mut ratatui::Frame, area: Rect, trash: &Trash, t: &Theme) {
    let width = area.width.saturating_sub(6).min(96);
    let room = width.saturating_sub(4) as usize;
    let tz = TimeZone::system();
    let mut lines: Vec<Line> = vec![Line::default()];
    match (&trash.docs, &trash.failed) {
        (_, Some(e)) => lines.push(
            fit(&format!("Couldn't load them: {e}"), room)
                .fg(t.needs)
                .into(),
        ),
        (None, None) => lines.push("Loading…".fg(t.muted).into()),
        (Some(d), None) if d.is_empty() => lines.push(
            "Nothing deleted. Docs you delete here stay 30 days."
                .fg(t.muted)
                .into(),
        ),
        (Some(docs), None) => {
            for (i, d) in docs.iter().enumerate() {
                let here = i == trash.cursor;
                let name = if d.title.is_empty() {
                    &d.path
                } else {
                    &d.title
                };
                let bg = style::Style::new().bg(t.selection);
                let mut first = Line::from(vec![
                    if here { "▌".fg(t.accent) } else { " ".into() },
                    fit(name, room.saturating_sub(1)).fg(t.fg),
                ]);
                let sub = format!(" {} · deleted {}", d.path, history::when(d.at, &tz));
                let mut second = Line::from(fit(&sub, room).fg(t.muted));
                if here {
                    first = first.style(bg);
                    second = second.style(bg);
                }
                lines.push(first);
                lines.push(second);
            }
        }
    }
    let title = match &trash.docs {
        Some(d) => format!(" Recently deleted · {} ", d.len()),
        None => " Recently deleted ".into(),
    };
    let hints = [hint(" ↩ ", "put back ", t), hint(" esc ", "back ", t)].concat();
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(3)
        .min(area.height.saturating_sub(2))
        .max(5);
    let rect = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
    .intersection(area);
    let rows = rect.height.saturating_sub(2) as usize;
    let skip = (trash.cursor * 2 + 3).saturating_sub(rows);
    let lines: Vec<Line> = lines.into_iter().skip(skip).collect();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(style::Style::new().fg(t.accent))
        .title(title.fg(t.fg).bold())
        .title_bottom(Line::from(fit_hints(hints, room)).centered())
        .padding(ratatui::widgets::Padding::horizontal(1))
        .style(style::Style::new().bg(t.panel).fg(t.fg));
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

impl Reader {
    /// A panel is over the doc (comments, history, a conflict).
    fn covered(&self) -> bool {
        self.comments.is_open() || self.history.is_some() || self.conflict.is_some()
    }

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

    fn reading(key: &str, text: &str) -> Page {
        let mut page = Page::default();
        page.loaded(Ok(listing(vec![doc(key, "a.md", false)])));
        page.keys(b"\r");
        let mut read = doc(key, "a.md", false);
        read.text = Some(text.into());
        read.hash = Some("h1".into());
        page.opened(key, Ok(read));
        page
    }

    #[test]
    fn e_edits_the_body_from_the_version_read() {
        let mut page = reading("7", "---\nvalk-id: 7\n---\n\nHello\n");
        assert_eq!(
            page.keys(b"e"),
            vec![Do::Edit {
                key: "7".into(),
                base: "h1".into(),
                body: "Hello\n".into(),
                title: "title 7".into()
            }]
        );
    }

    #[test]
    fn a_conflict_waits_for_a_choice() {
        let mut page = reading("7", "Hello\n");
        let mut theirs = doc("7", "a.md", false);
        theirs.text = Some("Hello there\n".into());
        theirs.hash = Some("h2".into());
        page.conflict("7", "Hello, mine\n".into(), theirs.clone());
        let shown = screen(&page, 120, 24);
        assert!(
            shown.contains("Yours")
                && shown.contains("On disk now")
                && shown.contains("Hello, mine"),
            "{shown}"
        );
        // Esc doesn't drop yours.
        assert!(matches!(page.keys(b"\x1b").as_slice(), [Do::Say(_)]));
        assert!(page.reading());
        assert_eq!(
            page.keys(b"s"),
            vec![Do::Save {
                key: "7".into(),
                base: "h2".into(),
                body: "Hello, mine\n".into()
            }]
        );
        assert_eq!(
            page.keys(b"e"),
            vec![Do::Edit {
                key: "7".into(),
                base: "h2".into(),
                body: "Hello, mine\n".into(),
                title: "title 7".into()
            }]
        );
        assert_eq!(page.keys(b"t"), vec![]);
        let r = page.reader.as_ref().unwrap();
        assert!(r.conflict.is_none());
        assert_eq!(r.doc.as_ref().unwrap().hash.as_deref(), Some("h2"));
    }

    #[test]
    fn a_conflict_for_a_doc_you_left_is_not_shown() {
        let mut page = reading("7", "Hello\n");
        assert!(!page.conflict("8", "mine".into(), doc("8", "b.md", false)));
        page.keys(b"\x1b");
        assert!(!page.conflict("7", "mine".into(), doc("7", "a.md", false)));
    }

    #[test]
    fn a_first_save_moves_your_own_doc_to_its_new_key() {
        let mut page = reading("p0123456789abcdef", "mine\n");
        let mut written = doc("12", "a.md", false);
        written.text = Some("---\nvalk-id: 12\n---\nmine, edited\n".into());
        page.saved("p0123456789abcdef", written);
        assert_eq!(page.reader.as_ref().unwrap().key, "12");
        // Later answers come by the new key.
        page.comments("12", Ok(vec![]), false);
        assert_eq!(page.keys(b"c"), vec![]);
        assert!(page.reader.as_ref().unwrap().comments.is_open());
    }

    #[test]
    fn d_twice_deletes_and_u_puts_it_back() {
        let mut page = reading("7", "Hello\n");
        // d, then anything else: nothing happens.
        assert!(matches!(page.keys(b"d").as_slice(), [Do::Say(_)]));
        assert_eq!(page.keys(b"j"), vec![Do::Say("not deleted".into())]);
        assert!(matches!(page.keys(b"d").as_slice(), [Do::Say(_)]));
        assert_eq!(
            page.keys(b"d"),
            vec![Do::Delete {
                key: "7".into(),
                base: "h1".into(),
                title: "title 7".into()
            }]
        );
        page.deleted("7", "tok".into(), "title 7".into());
        assert!(!page.reading());
        assert!(screen(&page, 80, 20).contains("Deleted “title 7” · u puts it back"));
        assert_eq!(page.keys(b"u"), vec![Do::Undelete("tok".into())]);
        let mut back = doc("7", "a.md", false);
        back.key = "7".into();
        assert_eq!(page.undeleted("tok", &back), Some(Do::Open("7".into())));
        assert!(page.reading());
        assert_eq!(page.keys(b"\x1b"), vec![]);
        assert_eq!(page.keys(b"u"), vec![Do::Say("nothing to undo".into())]);
    }

    #[test]
    fn the_mouse_leaves_a_covered_doc_alone_and_disarms_d() {
        let mut page = reading("7", "[x](https://x.dev)\n");
        screen(&page, 60, 20);
        let r = page.reader.as_ref().unwrap();
        let area = r.area.get();
        let y = (area.y..area.bottom())
            .find(|&y| page.link_at(area.x, y).is_some())
            .unwrap();
        page.keys(b"h");
        assert_eq!(
            page.link_at(area.x, y),
            None,
            "not through the history panel"
        );
        page.keys(b"\x1b");
        page.keys(b"d");
        page.pressed();
        assert!(!page.reader.as_ref().unwrap().deleting);
        page.deleted("7", "tok".into(), "t".into());
        page.undo_failed("other");
        assert!(page.undo.is_some());
        page.undo_failed("tok");
        assert!(page.undo.is_none());
    }

    #[test]
    fn recently_deleted_puts_one_back() {
        let mut page = Page::default();
        page.loaded(Ok(listing(vec![doc("1", "a.md", false)])));
        assert_eq!(page.keys(b"t"), vec![Do::Deleted]);
        page.trash(Ok(vec![
            DeletedDoc {
                token: "t1".into(),
                at: 10,
                path: "repos/x/old.md".into(),
                title: "Old plan".into(),
            },
            DeletedDoc {
                token: "t2".into(),
                at: 5,
                path: "older.md".into(),
                title: String::new(),
            },
        ]));
        let shown = screen(&page, 80, 20);
        assert!(
            shown.contains("Recently deleted · 2")
                && shown.contains("Old plan")
                && shown.contains("older.md"),
            "{shown}"
        );
        assert_eq!(page.keys(b"j\r"), vec![Do::Undelete("t2".into())]);
        assert_eq!(page.keys(b"\x1b"), vec![]);
        assert!(page.trash.is_none());
    }

    #[test]
    fn h_shows_history_and_restores_a_version() {
        let mut page = reading("7", "Hello\n");
        assert_eq!(page.keys(b"h"), vec![Do::History("7".into())]);
        page.history(
            "7",
            Ok(vec![DocVersion {
                version: "v1".into(),
                at: 10,
                by: "claude".into(),
                size: 10,
            }]),
        );
        assert_eq!(
            page.keys(b"\r"),
            vec![Do::Peek {
                key: "7".into(),
                version: "v1".into()
            }]
        );
        page.peeked("7", "v1", "Old\n".into());
        assert_eq!(
            page.keys(b"R"),
            vec![Do::Restore {
                key: "7".into(),
                base: "h1".into(),
                version: "v1".into()
            }]
        );
        // A restore that found it changed: the new version, the panel closed.
        let mut now = doc("7", "a.md", false);
        now.text = Some("Newer\n".into());
        now.hash = Some("h3".into());
        page.changed("7", now);
        let r = page.reader.as_ref().unwrap();
        assert!(r.history.is_none());
        assert_eq!(r.doc.as_ref().unwrap().hash.as_deref(), Some("h3"));
    }

    #[test]
    fn a_doc_without_an_id_has_no_history() {
        let mut page = reading("p0123456789abcdef", "mine\n");
        page.reader.as_mut().unwrap().doc.as_mut().unwrap().id = 0;
        assert!(matches!(page.keys(b"h").as_slice(), [Do::Say(s)] if s.contains("valk-id")));
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

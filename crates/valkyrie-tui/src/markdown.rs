//! Markdown as agents write it (decision and skill bodies, later docs), drawn the
//! way glow draws it: headings stand out, code sits on a panel, lists and quotes
//! keep a hanging indent. Lines come back already wrapped to the width given, so a
//! wrapped list item lines up under its text instead of under its bullet, and so
//! the caller's row count is the real one.
//!
//! Text stays text: HTML shows as written, and only http(s) links are links.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Theme;

/// A link drawn at `row` of the lines, over columns `from..to`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub row: usize,
    pub from: u16,
    pub to: u16,
    pub url: String,
}

pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<Link>,
}

/// `src` drawn in `width` columns.
pub fn render(src: &str, width: u16, t: &Theme) -> Rendered {
    let mut w = Writer::new(width.max(1), t);
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(src, options) {
        w.event(event);
    }
    w.flush();
    while w.out.last().is_some_and(|l| l.width() == 0) {
        w.out.pop();
    }
    // Deep nesting or a table can still overrun a very narrow width: cut it.
    let width = width.max(1);
    let lines = w.out.into_iter().map(|l| clip(l, width as usize)).collect();
    let links = w
        .links
        .into_iter()
        .filter(|l| l.from < width)
        .map(|l| Link {
            to: l.to.min(width),
            ..l
        })
        .collect();
    Rendered { lines, links }
}

/// `line` cut to `width` columns.
fn clip(line: Line<'static>, width: usize) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    let mut left = width;
    let mut spans = Vec::new();
    for span in line.spans {
        if left == 0 {
            break;
        }
        let w = span.width();
        if w <= left {
            left -= w;
            spans.push(span);
        } else {
            let text = cut(&span.content, left).swap_remove(0);
            left = 0;
            spans.push(Span::styled(text, span.style));
        }
    }
    Line::from(spans)
}

/// A run of text in one style, and the link it belongs to, if any.
#[derive(Clone)]
struct Piece {
    text: String,
    style: Style,
    link: Option<usize>,
}

enum Container {
    Quote,
    /// A list item: its marker, until the item's first line has used it.
    Item {
        marker: String,
        width: usize,
        used: bool,
    },
}

struct List {
    /// The next item's number, or None for bullets.
    next: Option<u64>,
    /// Its items hold paragraphs, so they get a blank line between them.
    loose: bool,
    items: usize,
}

struct Table {
    rows: Vec<Vec<Vec<Piece>>>,
    head: usize,
}

struct Writer<'t> {
    t: &'t Theme,
    width: u16,
    out: Vec<Line<'static>>,
    links: Vec<Link>,
    urls: Vec<String>,
    /// Inline text of the block being read.
    pieces: Vec<Piece>,
    bold: u8,
    italic: u8,
    strike: u8,
    /// The links being read, innermost last; an image is one too.
    link: Vec<usize>,
    heading: Option<HeadingLevel>,
    code: Option<String>,
    containers: Vec<Container>,
    lists: Vec<List>,
    /// The next block follows without a blank line (the first in an item).
    tight: bool,
    table: Option<Table>,
}

impl<'t> Writer<'t> {
    fn new(width: u16, t: &'t Theme) -> Self {
        Writer {
            t,
            width,
            out: Vec::new(),
            links: Vec::new(),
            urls: Vec::new(),
            pieces: Vec::new(),
            bold: 0,
            italic: 0,
            strike: 0,
            link: Vec::new(),
            heading: None,
            code: None,
            containers: Vec::new(),
            lists: Vec::new(),
            tight: false,
            table: None,
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(s) => {
                if let Some(code) = &mut self.code {
                    code.push_str(&s);
                } else {
                    self.text(&s);
                }
            }
            Event::Code(s) => {
                let style = self.style().fg(self.t.accent2).bg(self.t.panel);
                self.push(s.into_string(), style);
            }
            Event::Html(s) | Event::InlineHtml(s) => {
                let style = self.style().fg(self.t.muted);
                self.push(s.trim_end_matches('\n').to_string(), style);
            }
            Event::InlineMath(s) | Event::DisplayMath(s) => {
                self.push(s.into_string(), self.style())
            }
            Event::FootnoteReference(s) => {
                let style = self.style().fg(self.t.muted);
                self.push(format!("[^{s}]"), style);
            }
            Event::SoftBreak => self.push(" ".into(), self.style()),
            Event::HardBreak => self.push("\n".into(), self.style()),
            Event::Rule => {
                self.flush();
                self.gap();
                let avail = self.avail();
                let rule = Piece {
                    text: "─".repeat(avail),
                    style: Style::new().fg(self.t.border),
                    link: None,
                };
                self.emit(vec![rule]);
            }
            Event::TaskListMarker(done) => {
                if let Some(Container::Item {
                    marker,
                    width,
                    used: false,
                }) = self.containers.last_mut()
                {
                    *marker = if done { "☑ " } else { "☐ " }.into();
                    *width = 2;
                }
            }
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                if matches!(self.containers.last(), Some(Container::Item { .. }))
                    && let Some(list) = self.lists.last_mut()
                {
                    list.loose = true;
                }
                self.gap();
            }
            Tag::Heading { level, .. } => {
                self.flush();
                // A heading gets room above it, even right after a list.
                self.tight = false;
                self.gap();
                self.heading = Some(level);
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.gap();
                self.containers.push(Container::Quote);
                self.tight = true;
            }
            Tag::CodeBlock(_) => {
                self.flush();
                self.gap();
                self.code = Some(String::new());
            }
            Tag::List(start) => {
                self.flush();
                // A list right under an item's text belongs to it: no gap.
                if self.lists.is_empty() {
                    self.gap();
                }
                self.lists.push(List {
                    next: start,
                    loose: false,
                    items: 0,
                });
            }
            Tag::Item => {
                self.flush();
                // Items of a loose list stand apart, like paragraphs.
                if self.lists.last().is_some_and(|l| l.loose && l.items > 0) {
                    self.tight = false;
                    self.gap();
                }
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(list) => {
                        list.items += 1;
                        match &mut list.next {
                            Some(n) => {
                                let m = format!("{n}. ");
                                *n += 1;
                                m
                            }
                            None => format!("{} ", ["•", "◦", "▪"][depth % 3]),
                        }
                    }
                    None => "• ".into(),
                };
                let width = marker.width();
                self.containers.push(Container::Item {
                    marker,
                    width,
                    used: false,
                });
                self.tight = true;
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => {
                self.urls.push(dest_url.into_string());
                self.link.push(self.urls.len() - 1);
            }
            Tag::Image { dest_url, .. } => {
                self.urls.push(dest_url.into_string());
                self.link.push(self.urls.len() - 1);
                let style = self.style();
                self.push("image: ".into(), style);
            }
            Tag::Table(_) => {
                self.flush();
                self.gap();
                self.table = Some(Table {
                    rows: Vec::new(),
                    head: 0,
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableCell => self.pieces.clear(),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.flush(),
            TagEnd::Heading(_) => {
                self.flush();
                self.heading = None;
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.containers.pop();
                // What follows is a block of its own, with room above it.
                self.tight = false;
            }
            TagEnd::CodeBlock => {
                let code = self.code.take().unwrap_or_default();
                self.code_block(&code);
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                // What follows is a block of its own, with room above it.
                self.tight = false;
            }
            TagEnd::Item => {
                self.flush();
                // An empty item still shows its marker.
                if matches!(
                    self.containers.last(),
                    Some(Container::Item { used: false, .. })
                ) {
                    self.emit(Vec::new());
                }
                self.containers.pop();
                // What follows is a block of its own, with room above it.
                self.tight = false;
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link | TagEnd::Image => {
                let Some(i) = self.link.pop() else { return };
                // The URL follows its text, dimmed, unless the text already is it.
                let url = self.urls[i].clone();
                let shown: String = self
                    .pieces
                    .iter()
                    .filter(|p| p.link == Some(i))
                    .map(|p| p.text.as_str())
                    .collect();
                if shown.trim() != url && !url.is_empty() {
                    let style = Style::new()
                        .fg(self.t.muted)
                        .add_modifier(Modifier::UNDERLINED);
                    self.push(" ".into(), self.style());
                    self.pieces.push(Piece {
                        text: url,
                        style,
                        link: Some(i),
                    });
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.head = table.rows.len();
                }
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.pieces);
                if let Some(row) = self.table.as_mut().and_then(|t| t.rows.last_mut()) {
                    row.push(cell);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.draw_table(table);
                }
            }
            _ => {}
        }
    }

    /// The style for inline text here.
    fn style(&self) -> Style {
        let mut s = Style::new().fg(self.t.fg);
        if self.bold > 0 {
            s = s.add_modifier(Modifier::BOLD);
        }
        if self.italic > 0 {
            s = s.add_modifier(Modifier::ITALIC);
        }
        if self.strike > 0 {
            s = s.add_modifier(Modifier::CROSSED_OUT);
        }
        if !self.link.is_empty() {
            s = s.fg(self.t.accent).add_modifier(Modifier::UNDERLINED);
        }
        s
    }

    fn push(&mut self, text: String, style: Style) {
        self.pieces.push(Piece {
            text,
            style,
            link: self.link.last().copied(),
        });
    }

    /// Plain text, with any bare http(s) URL in it made a link.
    fn text(&mut self, s: &str) {
        let style = self.style();
        if !self.link.is_empty() {
            self.push(s.to_string(), style);
            return;
        }
        let mut rest = s;
        while let Some(at) = find_url(rest) {
            let (before, from) = rest.split_at(at);
            if !before.is_empty() {
                self.push(before.to_string(), style);
            }
            let len = url_len(from);
            let (url, after) = from.split_at(len);
            self.urls.push(url.to_string());
            self.pieces.push(Piece {
                text: url.to_string(),
                style: style.fg(self.t.accent).add_modifier(Modifier::UNDERLINED),
                link: Some(self.urls.len() - 1),
            });
            rest = after;
        }
        if !rest.is_empty() {
            self.push(rest.to_string(), style);
        }
    }

    /// A blank line before a block, unless it's the first one, or the first in
    /// its item or quote.
    fn gap(&mut self) {
        if std::mem::take(&mut self.tight) || self.out.is_empty() {
            return;
        }
        if self.out.last().is_some_and(|l| l.width() == 0) {
            return;
        }
        let (prefix, _) = self.prefix(false);
        let text: String = prefix.iter().map(|p| p.content.as_ref()).collect();
        if text.trim().is_empty() {
            self.out.push(Line::default());
        } else {
            // Inside a quote the bar runs on through the gap.
            let mut spans = prefix;
            if let Some(last) = spans.last_mut() {
                last.content = last.content.trim_end().to_string().into();
            }
            self.out.push(Line::from(spans));
        }
    }

    /// The spans in front of a line, and their width. `first` takes an unused
    /// item marker (and marks it used).
    fn prefix(&mut self, first: bool) -> (Vec<Span<'static>>, usize) {
        let mut spans = Vec::new();
        let mut width = 0;
        for c in &mut self.containers {
            match c {
                Container::Quote => {
                    spans.push(Span::styled("│ ", Style::new().fg(self.t.border)));
                    width += 2;
                }
                Container::Item {
                    marker,
                    width: w,
                    used,
                } => {
                    if first && !*used {
                        spans.push(Span::styled(marker.clone(), Style::new().fg(self.t.accent)));
                        *used = true;
                    } else {
                        spans.push(Span::raw(" ".repeat(*w)));
                    }
                    width += *w;
                }
            }
        }
        (spans, width)
    }

    /// Columns left for text after the prefix.
    fn avail(&self) -> usize {
        let used: usize = self
            .containers
            .iter()
            .map(|c| match c {
                Container::Quote => 2,
                Container::Item { width, .. } => *width,
            })
            .sum();
        (self.width as usize).saturating_sub(used).max(1)
    }

    /// Writes the inline text read so far as a wrapped block.
    fn flush(&mut self) {
        if self.table.is_some() {
            return;
        }
        let mut pieces = std::mem::take(&mut self.pieces);
        if pieces.iter().all(|p| p.text.trim().is_empty()) {
            return;
        }
        match self.heading {
            Some(HeadingLevel::H1) => {
                let chip = Style::new()
                    .fg(self.t.bg)
                    .bg(self.t.accent)
                    .add_modifier(Modifier::BOLD);
                for p in &mut pieces {
                    p.style = p.style.patch(chip);
                }
                pieces.insert(
                    0,
                    Piece {
                        text: "\u{a0}".into(),
                        style: chip,
                        link: None,
                    },
                );
                pieces.push(Piece {
                    text: "\u{a0}".into(),
                    style: chip,
                    link: None,
                });
            }
            Some(level) => {
                let color = if level == HeadingLevel::H2 {
                    self.t.accent
                } else {
                    self.t.accent2
                };
                for p in &mut pieces {
                    p.style = p.style.fg(color).add_modifier(Modifier::BOLD);
                }
            }
            None => {}
        }
        self.emit(pieces);
    }

    /// Wraps `pieces` to the room left and writes them, prefixed.
    fn emit(&mut self, pieces: Vec<Piece>) {
        let avail = self.avail();
        for (n, row) in wrap(&pieces, avail).into_iter().enumerate() {
            let (mut spans, mut x) = self.prefix(n == 0);
            for seg in merge(row) {
                let w = seg.text.width();
                if let Some(i) = seg.link.filter(|&i| clickable(&self.urls[i])) {
                    self.links.push(Link {
                        row: self.out.len(),
                        from: x as u16,
                        to: (x + w) as u16,
                        url: self.urls[i].clone(),
                    });
                }
                x += w;
                spans.push(Span::styled(seg.text, seg.style));
            }
            self.out.push(Line::from(spans));
        }
    }

    /// A fenced or indented block: on the panel color, cut (not wrapped at
    /// words) to fit.
    fn code_block(&mut self, code: &str) {
        let avail = self.avail();
        let inner = avail.saturating_sub(2).max(1);
        let style = Style::new().fg(self.t.fg).bg(self.t.panel);
        let code = code.strip_suffix('\n').unwrap_or(code);
        for line in code.split('\n') {
            let line = line.replace('\t', "    ");
            for chunk in cut(&line, inner) {
                let pad = inner.saturating_sub(chunk.width());
                let text = format!(" {chunk}{} ", " ".repeat(pad));
                let (mut spans, _) = self.prefix(true);
                spans.push(Span::styled(text, style));
                self.out.push(Line::from(spans));
            }
        }
    }

    fn draw_table(&mut self, table: Table) {
        let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let text = |cell: &[Piece]| -> String { cell.iter().map(|p| p.text.as_str()).collect() };
        let mut widths = vec![0; cols];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(text(cell).width());
            }
        }
        // Too wide: the widest columns give up room first, down to 3.
        let avail = self.avail();
        let seps = 3 * (cols - 1);
        while widths.iter().sum::<usize>() + seps > avail {
            let (i, &w) = widths.iter().enumerate().max_by_key(|(_, w)| **w).unwrap();
            if w <= 3 {
                break;
            }
            widths[i] -= 1;
        }
        let sep = Style::new().fg(self.t.border);
        for (r, row) in table.rows.iter().enumerate() {
            if r == table.head && r > 0 {
                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                let (mut spans, _) = self.prefix(true);
                spans.push(Span::styled(rule.join("─┼─"), sep));
                self.out.push(Line::from(spans));
            }
            let (mut spans, mut x) = self.prefix(true);
            for (i, &w) in widths.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(" │ ", sep));
                    x += 3;
                }
                let cell = row.get(i).map(Vec::as_slice).unwrap_or_default();
                // A cell too wide keeps a column for the …
                let limit = if text(cell).width() > w {
                    w.saturating_sub(1)
                } else {
                    w
                };
                let mut used = 0;
                for p in merge(cell.to_vec()) {
                    let mut shown = String::new();
                    for c in p.text.chars() {
                        let cw = c.width().unwrap_or(0);
                        if used + cw > limit {
                            break;
                        }
                        shown.push(c);
                        used += cw;
                    }
                    if shown.is_empty() {
                        continue;
                    }
                    let sw = shown.width();
                    if let Some(l) = p.link.filter(|&l| clickable(&self.urls[l])) {
                        self.links.push(Link {
                            row: self.out.len(),
                            from: x as u16,
                            to: (x + sw) as u16,
                            url: self.urls[l].clone(),
                        });
                    }
                    x += sw;
                    let style = if r < table.head {
                        p.style.add_modifier(Modifier::BOLD)
                    } else {
                        p.style
                    };
                    spans.push(Span::styled(shown, style));
                }
                if limit < w {
                    spans.push(Span::styled("…", Style::new().fg(self.t.muted)));
                    used += 1;
                    x += 1;
                }
                let pad = w.saturating_sub(used);
                spans.push(Span::raw(" ".repeat(pad)));
                x += pad;
            }
            self.out.push(Line::from(spans));
        }
    }
}

/// Only http(s) links open: agents write these bodies, so no `file:` or
/// `mailto:` reaches the opener.
fn clickable(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// Where the first http(s) URL in `s` starts.
fn find_url(s: &str) -> Option<usize> {
    let at = [s.find("https://"), s.find("http://")]
        .into_iter()
        .flatten()
        .min()?;
    // Just the scheme is not a link.
    (url_len(&s[at..]) > "https://".len())
        .then_some(at)
        .or_else(|| {
            let skip = at + 4;
            find_url(&s[skip..]).map(|n| n + skip)
        })
}

/// How much of `s`, which starts with a URL, the URL takes: up to a space, less
/// the punctuation that ends a sentence around it.
fn url_len(s: &str) -> usize {
    let end = s
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '`'))
        .unwrap_or(s.len());
    let mut url = &s[..end];
    loop {
        let trimmed = url.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
        // A closing paren the URL didn't open belongs to the sentence.
        let trimmed = if trimmed.ends_with(')')
            && trimmed.matches('(').count() < trimmed.matches(')').count()
        {
            &trimmed[..trimmed.len() - 1]
        } else {
            trimmed
        };
        if trimmed.len() == url.len() {
            return url.len();
        }
        url = trimmed;
    }
}

/// `s` cut into pieces of at most `width` columns.
fn cut(s: &str, width: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > width && w > 0 {
            out.push(String::new());
            w = 0;
        }
        out.last_mut().unwrap().push(c);
        w += cw;
    }
    out
}

enum Token {
    Word(Vec<Piece>),
    Space(Piece),
    Break,
}

/// The pieces as words, spaces and hard breaks. A word may run across pieces
/// (`**bold**,`), so it never breaks between them.
fn tokens(pieces: &[Piece]) -> Vec<Token> {
    let mut out = Vec::new();
    let mut word: Vec<Piece> = Vec::new();
    let end_word = |word: &mut Vec<Piece>, out: &mut Vec<Token>| {
        if !word.is_empty() {
            out.push(Token::Word(std::mem::take(word)));
        }
    };
    for p in pieces {
        let mut run = String::new();
        for c in p.text.chars() {
            if c == '\n' || c == ' ' {
                if !run.is_empty() {
                    word.push(Piece {
                        text: std::mem::take(&mut run),
                        ..p.clone()
                    });
                }
                end_word(&mut word, &mut out);
                out.push(if c == '\n' {
                    Token::Break
                } else {
                    Token::Space(Piece {
                        text: " ".into(),
                        ..p.clone()
                    })
                });
            } else {
                run.push(c);
            }
        }
        if !run.is_empty() {
            word.push(Piece {
                text: run,
                ..p.clone()
            });
        }
    }
    end_word(&mut word, &mut out);
    out
}

/// Greedy word wrap into rows of at most `width` columns. A word longer than a
/// row is cut.
fn wrap(pieces: &[Piece], width: usize) -> Vec<Vec<Piece>> {
    let mut rows: Vec<Vec<Piece>> = vec![Vec::new()];
    let mut w = 0;
    let mut space: Option<Piece> = None;
    for token in tokens(pieces) {
        match token {
            Token::Break => {
                rows.push(Vec::new());
                w = 0;
                space = None;
            }
            Token::Space(p) => {
                if w > 0 {
                    space = Some(p);
                }
            }
            Token::Word(word) => {
                let ww: usize = word.iter().map(|p| p.text.width()).sum();
                let sw = usize::from(space.is_some());
                if w > 0 && w + sw + ww > width {
                    rows.push(Vec::new());
                    w = 0;
                    space = None;
                }
                if let Some(s) = space.take() {
                    rows.last_mut().unwrap().push(s);
                    w += 1;
                }
                for p in word {
                    for (i, chunk) in cut(&p.text, width).into_iter().enumerate() {
                        let cw = chunk.width();
                        if (i > 0 || w + cw > width) && w > 0 {
                            rows.push(Vec::new());
                            w = 0;
                        }
                        rows.last_mut().unwrap().push(Piece {
                            text: chunk,
                            ..p.clone()
                        });
                        w += cw;
                    }
                }
            }
        }
    }
    rows
}

/// Neighbouring pieces in the same style and link, as one.
fn merge(row: Vec<Piece>) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for p in row {
        match out.last_mut() {
            Some(last) if last.style == p.style && last.link == p.link => {
                last.text.push_str(&p.text)
            }
            _ => out.push(p),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> &'static Theme {
        &crate::theme::DRACULA
    }

    fn text(r: &Rendered) -> Vec<String> {
        r.lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn marks_are_gone_and_styles_stay() {
        let r = render("# Title\n\nSome **bold** and `code`.", 40, theme());
        let t = text(&r);
        assert_eq!(t[0].trim_matches('\u{a0}'), "Title");
        assert_eq!(t[1], "");
        assert_eq!(t[2], "Some bold and code.");
        let bold = r.lines[2]
            .spans
            .iter()
            .find(|s| s.content == "bold")
            .unwrap();
        assert!(bold.style.add_modifier.contains(Modifier::BOLD));
        let code = r.lines[2]
            .spans
            .iter()
            .find(|s| s.content == "code")
            .unwrap();
        assert_eq!(code.style.bg, Some(theme().panel));
    }

    #[test]
    fn a_wrapped_item_hangs_under_its_text() {
        let r = render("- one two three four five six", 12, theme());
        assert_eq!(text(&r), ["• one two", "  three four", "  five six"]);
        let r = render("1. alpha\n2. beta gamma delta", 12, theme());
        assert_eq!(text(&r), ["1. alpha", "2. beta", "   gamma", "   delta"]);
    }

    #[test]
    fn nested_lists_and_tasks() {
        let r = render("- a\n  - b\n- [x] done\n- [ ] todo", 30, theme());
        assert_eq!(text(&r), ["• a", "  ◦ b", "☑ done", "☐ todo"]);
    }

    #[test]
    fn quotes_carry_a_bar_through_gaps() {
        let r = render("> one\n>\n> two", 20, theme());
        assert_eq!(text(&r), ["│ one", "│", "│ two"]);
    }

    #[test]
    fn code_blocks_sit_on_the_panel_and_are_cut_to_fit() {
        let r = render("```\nfn main() {}\n\tx\n```", 10, theme());
        let t = text(&r);
        assert_eq!(t, [" fn main( ", " ) {}     ", "     x    "]);
        assert!(r.lines.iter().all(|l| l.width() <= 10));
        assert_eq!(r.lines[0].spans[0].style.bg, Some(theme().panel));
    }

    #[test]
    fn links_show_their_url_and_know_where_they_are() {
        let r = render("See [the docs](https://x.dev/d) now", 60, theme());
        assert_eq!(text(&r), ["See the docs https://x.dev/d now"]);
        let urls: Vec<_> = r
            .links
            .iter()
            .map(|l| (l.from, l.to, l.url.as_str()))
            .collect();
        assert_eq!(
            urls,
            [(4, 12, "https://x.dev/d"), (13, 28, "https://x.dev/d")]
        );
    }

    #[test]
    fn bare_urls_are_links_without_trailing_punctuation() {
        let r = render("Read https://a.dev/x (and https://b.dev/y).", 60, theme());
        let urls: Vec<_> = r.links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, ["https://a.dev/x", "https://b.dev/y"]);
        // A link whose text is its URL isn't shown twice.
        let r = render("<https://a.dev>", 60, theme());
        assert_eq!(text(&r), ["https://a.dev"]);
    }

    #[test]
    fn a_link_on_a_wrapped_row_is_found_there() {
        let r = render("- word word https://a.dev/long", 20, theme());
        assert_eq!(text(&r), ["• word word", "  https://a.dev/long"]);
        assert_eq!(
            r.links,
            [Link {
                row: 1,
                from: 2,
                to: 20,
                url: "https://a.dev/long".into()
            }]
        );
    }

    #[test]
    fn html_is_text() {
        let r = render("a <b>b</b> c", 40, theme());
        assert_eq!(text(&r), ["a <b>b</b> c"]);
    }

    #[test]
    fn tables_line_up() {
        let r = render("| a | bb |\n|---|---|\n| ccc | d |", 40, theme());
        assert_eq!(text(&r), ["a   │ bb", "────┼───", "ccc │ d "]);
    }

    #[test]
    fn nothing_is_wider_than_asked() {
        let src = "# A long heading that wraps\n\n> quote with words that wrap around\n\n- item https://example.com/a/very/long/path/that/must/be/cut\n\n| col | another col |\n|--|--|\n| x | y |\n\n---";
        for width in [1, 5, 12, 33] {
            for l in render(src, width, theme()).lines {
                assert!(l.width() <= width as usize, "{width}: {l:?}");
            }
        }
    }

    #[test]
    fn a_block_after_a_tight_list_gets_its_gap() {
        let r = render("- a\n- b\n\n> q", 20, theme());
        assert_eq!(text(&r), ["• a", "• b", "", "│ q"]);
    }

    #[test]
    fn empty_items_keep_their_marker_and_loose_lists_their_gaps() {
        let r = render("1.\n2. b", 20, theme());
        assert_eq!(text(&r), ["1. ", "2. b"]);
        let r = render("- a\n\n- b", 20, theme());
        assert_eq!(text(&r), ["• a", "", "• b"]);
    }

    #[test]
    fn only_http_links_open() {
        let r = render(
            "[mail](mailto:a@b.c) [file](file:///etc/passwd) [ok](https://a.dev)",
            80,
            theme(),
        );
        let urls: Vec<_> = r.links.iter().map(|l| l.url.as_str()).collect();
        assert_eq!(urls, ["https://a.dev", "https://a.dev"]);
    }

    #[test]
    fn links_in_tables_open() {
        let r = render(
            "| a | b |\n|---|---|\n| x | [y](https://y.dev) |",
            60,
            theme(),
        );
        assert_eq!(text(&r)[2], "x │ y https://y.dev");
        assert_eq!(
            r.links[0],
            Link {
                row: 2,
                from: 4,
                to: 5,
                url: "https://y.dev".into()
            }
        );
        assert_eq!(r.links[1].from, 6);
    }

    #[test]
    fn hard_breaks_and_rules() {
        let r = render("a  \nb\n\n---", 5, theme());
        assert_eq!(text(&r), ["a", "b", "", "─────"]);
    }
}

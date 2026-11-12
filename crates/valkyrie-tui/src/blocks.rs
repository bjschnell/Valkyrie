//! The blocks of a doc a comment can be about (ADR-0021 §6), split and quoted
//! exactly as the web app does it (`parseMarkdown` in web/src/lib/chat.ts,
//! `quoteOf` in web/src/lib/docs.ts). A comment finds its block again by its
//! quote, so a comment left here must quote a block the way the web app would,
//! and the other way round.

/// What kind of block it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Paragraph,
    Heading,
    Code,
    Item,
}

impl Kind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Kind::Paragraph => "para",
            Kind::Heading => "heading",
            Kind::Code => "code",
            Kind::Item => "item",
        }
    }
}

/// A block that takes comments: its kind, and its quote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    pub(crate) kind: Kind,
    pub(crate) quote: String,
}

/// Longest quote the daemon keeps, in characters.
const MAX_QUOTE: usize = 500;

/// A block's text as a comment quotes it: hidden characters out, on one line, cut
/// at 500 characters.
pub(crate) fn quote_of(text: &str) -> String {
    let shown: String = text
        .chars()
        .map(|c| if hidden(c) { ' ' } else { c })
        .collect();
    let line = shown
        .split(is_space)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if line.chars().count() > MAX_QUOTE {
        let mut cut: String = line.chars().take(MAX_QUOTE).collect();
        cut.push('…');
        cut
    } else {
        line
    }
}

/// Control characters and bidi overrides, as the daemon drops them.
fn hidden(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// JavaScript's `\s` (and what its `trim` drops): Rust's whitespace less
/// U+0085, plus U+FEFF.
fn is_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// Whether JavaScript's `.` matches all of `s`: it doesn't match line breaks,
/// so a `(.*)$` fails on a line that holds one.
fn dot_all(s: &str) -> bool {
    !s.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
}

fn blank(line: &str) -> bool {
    line.trim_matches(is_space).is_empty()
}

fn trim(line: &str) -> &str {
    line.trim_matches(is_space)
}

fn trim_start(line: &str) -> &str {
    line.trim_start_matches(is_space)
}

/// `^\s*([-*+]|\d+[.)])\s+(.*)$`: whether it's ordered, and the text after the marker.
fn list_item(line: &str) -> Option<(bool, &str)> {
    let rest = trim_start(line);
    let (ordered, after) = if let Some(r) = rest.strip_prefix(['-', '*', '+']) {
        (false, r)
    } else {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            return None;
        }
        (true, rest[digits..].strip_prefix(['.', ')'])?)
    };
    let text = trim_start(after);
    // At least one space after the marker.
    (text.len() < after.len() && dot_all(text)).then_some((ordered, text))
}

/// `^(#{1,6})\s+(.*)$`: the heading's text.
fn heading(line: &str) -> Option<&str> {
    let hashes = line.len() - line.trim_start_matches('#').len();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let after = &line[hashes..];
    let text = trim_start(after);
    (text.len() < after.len() && dot_all(text)).then_some(text)
}

/// `^#{1,6}\s`: a heading's start, as `startsBlock` looks for it (no `$`).
fn heading_start(line: &str) -> bool {
    let hashes = line.len() - line.trim_start_matches('#').len();
    (1..=6).contains(&hashes) && line[hashes..].starts_with(is_space)
}

/// `^\s*([-*_])(\s*\1){2,}\s*$`: a rule.
fn rule(line: &str) -> bool {
    let t = trim(line);
    let Some(c) = t.chars().next() else {
        return false;
    };
    matches!(c, '-' | '*' | '_')
        && t.chars().all(|x| x == c || is_space(x))
        && t.chars().filter(|&x| x == c).count() >= 3
}

/// `^\s*(```+|~~~+)`: the fence.
fn fence(line: &str) -> Option<&str> {
    let t = trim_start(line);
    for mark in ['`', '~'] {
        let n = t.len() - t.trim_start_matches(mark).len();
        if n >= 3 {
            return Some(&t[..n]);
        }
    }
    None
}

/// `^\s{2,}\S`: indented by two or more.
fn indented(line: &str) -> bool {
    let rest = trim_start(line);
    !rest.is_empty() && line[..line.len() - rest.len()].chars().count() >= 2
}

/// A line that starts a block of its own, so it can't continue a list item.
fn starts_block(line: &str) -> bool {
    let t = trim_start(line);
    t.starts_with("```")
        || t.starts_with("~~~")
        || heading_start(line)
        || rule(line)
        || t.starts_with('|')
        || line.starts_with('>')
}

/// `^\[([ xX])\]\s+(.*)$`: an item's text without its task box.
fn task(item: &str) -> &str {
    for b in ["[ ]", "[x]", "[X]"] {
        if let Some(after) = item.strip_prefix(b) {
            let text = trim_start(after);
            if text.len() < after.len() {
                return text;
            }
        }
    }
    item
}

/// The blocks of `src` (a doc's body, without its front matter) that take
/// comments, in order.
pub(crate) fn blocks(src: &str) -> Vec<Block> {
    let src = src.replace("\r\n", "\n");
    let lines: Vec<&str> = src.split('\n').collect();
    let mut out = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let push = |out: &mut Vec<Block>, kind, text: &str| {
        out.push(Block {
            kind,
            quote: quote_of(text),
        })
    };
    let flush = |out: &mut Vec<Block>, para: &mut Vec<&str>| {
        if !para.is_empty() {
            push(out, Kind::Paragraph, &para.join("\n"));
            para.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(mark) = fence(line) {
            flush(&mut out, &mut para);
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !trim_start(lines[i]).starts_with(mark) {
                body.push(lines[i]);
                i += 1;
            }
            push(&mut out, Kind::Code, &body.join("\n"));
            i += 1;
            continue;
        }
        if blank(line) {
            flush(&mut out, &mut para);
            i += 1;
            continue;
        }
        if let Some(text) = heading(line) {
            flush(&mut out, &mut para);
            push(&mut out, Kind::Heading, text);
            i += 1;
            continue;
        }
        if rule(line) {
            flush(&mut out, &mut para);
            i += 1;
            continue;
        }
        if trim_start(line).starts_with('|') {
            // A table, or pipe lines: no comments on either.
            flush(&mut out, &mut para);
            while i < lines.len() && trim_start(lines[i]).starts_with('|') {
                i += 1;
            }
            continue;
        }
        if let Some((ordered, first)) = list_item(line) {
            flush(&mut out, &mut para);
            let same = |l: Option<&&str>| {
                l.and_then(|l| list_item(l))
                    .is_some_and(|(o, _)| o == ordered)
            };
            let mut items = vec![first.to_string()];
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                if let Some((_, text)) = list_item(next) {
                    if !same(Some(&next)) {
                        break;
                    }
                    items.push(text.to_string());
                } else if blank(next) {
                    // A blank line ends the list, unless another item or an
                    // indented paragraph of this one comes after it.
                    let mut j = i + 1;
                    while j < lines.len() && blank(lines[j]) {
                        j += 1;
                    }
                    if same(lines.get(j)) {
                        i = j - 1;
                    } else if j < lines.len() && indented(lines[j]) {
                        let last = items.last_mut().unwrap();
                        last.push_str("\n\n");
                        last.push_str(trim(lines[j]));
                        i = j;
                    } else {
                        break;
                    }
                } else if indented(next) || !starts_block(next) {
                    let last = items.last_mut().unwrap();
                    last.push('\n');
                    last.push_str(trim(next));
                } else {
                    break;
                }
                i += 1;
            }
            for item in &items {
                push(&mut out, Kind::Item, task(item));
            }
            continue;
        }
        if line.starts_with('>') {
            // A quote takes no comments.
            flush(&mut out, &mut para);
            while i < lines.len() && lines[i].starts_with('>') {
                i += 1;
            }
            continue;
        }
        para.push(line);
        i += 1;
    }
    flush(&mut out, &mut para);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quotes(src: &str) -> Vec<(Kind, String)> {
        blocks(src).into_iter().map(|b| (b.kind, b.quote)).collect()
    }

    #[test]
    fn quotes_like_the_web_app() {
        assert_eq!(quote_of("  a\n\tb  \u{202e}c\u{1}d "), "a b c d");
        let long = "x".repeat(600);
        let q = quote_of(&long);
        assert_eq!(q.chars().count(), 501);
        assert!(q.ends_with('…'));
    }

    #[test]
    fn splits_blocks_like_the_web_app() {
        let src = "# Title\n\nA paragraph\nover two lines.\n\n- one\n- [x] two\n  more of two\n\n```sh\nls -la\n```\n\n> quoted\n\n| a | b |\n|---|---|\n| c | d |\n\n---\n\n1. first\n\n2. second\n\n   still second";
        assert_eq!(
            quotes(src),
            vec![
                (Kind::Heading, "Title".into()),
                (Kind::Paragraph, "A paragraph over two lines.".into()),
                (Kind::Item, "one".into()),
                (Kind::Item, "two more of two".into()),
                (Kind::Code, "ls -la".into()),
                (Kind::Item, "first".into()),
                (Kind::Item, "second still second".into()),
            ]
        );
    }

    #[test]
    fn a_list_ends_at_a_block_or_a_list_of_the_other_kind() {
        assert_eq!(
            quotes("- a\n# H\n1. b\n- c"),
            vec![
                (Kind::Item, "a".into()),
                (Kind::Heading, "H".into()),
                (Kind::Item, "b".into()),
                (Kind::Item, "c".into()),
            ]
        );
        // A plain line right under an item continues it.
        assert_eq!(
            quotes("- step\nrun this"),
            vec![(Kind::Item, "step run this".into())]
        );
    }

    #[test]
    fn not_a_heading_or_item_is_paragraph_text() {
        assert_eq!(
            quotes("#nospace\n####### seven\n-nospace"),
            vec![(Kind::Paragraph, "#nospace ####### seven -nospace".into())]
        );
    }

    #[test]
    fn line_breaks_js_dot_skips_and_u85_are_as_on_the_web() {
        assert_eq!(
            quotes("# a\u{2028}b"),
            vec![(Kind::Paragraph, "# a b".into())]
        );
        assert_eq!(quotes("- x\n- a\rb"), vec![(Kind::Item, "x - a b".into())]);
        assert_eq!(quotes("#\u{85}x"), vec![(Kind::Paragraph, "# x".into())]);
    }

    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        assert_eq!(quotes("```\na\nb"), vec![(Kind::Code, "a b".into())]);
    }
}

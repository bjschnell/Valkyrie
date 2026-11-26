//! Links in a session: finding the URL under a click, and opening it in a browser
//! on the machine the TUI runs on (Windows' from WSL). Over SSH there is no browser
//! here, so the link goes to the clipboard instead.

use std::process::{Command, Stdio};
use valkyrie_proto::Row;

/// Rows a link may run across, wrapped, above or below the clicked one.
const MAX_ROWS: u16 = 16;

/// The http(s) URL at cell (`x`, `y`) of `rows`, if there is one. A URL may run on
/// to the next row: one the terminal wrapped, or one a program broke at the edge
/// itself (Claude Code does), when it fills the row to the last column.
pub fn url_at(rows: &[Row], cols: u16, x: u16, y: u16) -> Option<String> {
    let cells = |y: u16| -> Vec<u8> {
        let row = rows.iter().find(|r| r.y == y);
        // A cell is its character when that is one ASCII byte; anything else
        // (wide, combining, blank) can't be in a URL here.
        crate::mouse::row_cells(row)
            .iter()
            .map(|c| match c.as_bytes() {
                [b] if b.is_ascii_graphic() => *b,
                _ => b' ',
            })
            .collect()
    };
    // Whether row `y` runs on into the next.
    let runs_on = |y: u16| -> bool {
        let row = rows.iter().find(|r| r.y == y);
        row.is_some_and(|r| r.wrapped) || {
            let c = cells(y);
            c.len() >= cols as usize && c.last().is_some_and(|&b| url_char(b))
        }
    };
    let mut top = y;
    while top > 0 && y - top < MAX_ROWS && runs_on(top - 1) {
        top -= 1;
    }
    let mut bottom = y;
    while bottom - y < MAX_ROWS && runs_on(bottom) && rows.iter().any(|r| r.y == bottom + 1) {
        bottom += 1;
    }
    let mut line = Vec::new();
    let mut at = 0;
    for row in top..=bottom {
        if row == y {
            at = line.len() + x as usize;
        }
        let mut c = cells(row);
        if row < bottom {
            c.resize(cols as usize, b' ');
        }
        line.extend(c);
    }
    url_in(&line, at)
}

/// The doc mentioned at cell (`x`, `y`) of `rows`, by its valk-id (backlog #311):
/// what `valk doc write` and `write_doc` print ("doc 8 written: …"), a read or an
/// edit of one (`valk doc read 8`), and "doc #8", as the web app's chat finds them.
pub fn doc_at(rows: &[Row], x: u16, y: u16) -> Option<u32> {
    let row = rows.iter().find(|r| r.y == y);
    // One byte a cell, as `url_at` reads them, so a byte is a column.
    let line: String = crate::mouse::row_cells(row)
        .iter()
        .map(|c| match c.as_bytes() {
            [b] if b.is_ascii_graphic() => *b as char,
            _ => ' ',
        })
        .collect();
    doc_in(&line, x as usize)
}

/// The doc mentioned in `line` over byte `at`.
fn doc_in(line: &str, at: usize) -> Option<u32> {
    let lower = line.to_ascii_lowercase();
    let word_start = |i: usize| i == 0 || !lower.as_bytes()[i - 1].is_ascii_alphanumeric();
    let digits = |from: usize| {
        let n = lower[from..].bytes().take_while(u8::is_ascii_digit).count();
        let after = lower.as_bytes().get(from + n);
        (n > 0 && n <= 10 && !after.is_some_and(u8::is_ascii_alphanumeric)).then_some(n)
    };
    for (start, _) in lower.match_indices("doc ") {
        // "valk doc read 8" reaches back to "valk"; "doc …" starts here.
        let (from, rest) = match start.checked_sub(5) {
            Some(v) if lower[v..].starts_with("valk ") && word_start(v) => (v, start + 4),
            _ => (start, start + 4),
        };
        if !word_start(from) {
            continue;
        }
        let tail = &lower[rest..];
        let (num, needs) = if from < start {
            match ["read ", "edit "].iter().find(|v| tail.starts_with(**v)) {
                Some(v) => (rest + v.len(), None),
                None => continue,
            }
        } else if tail.starts_with('#') {
            (rest + 1, None)
        } else {
            (rest, Some(" written"))
        };
        let Some(n) = digits(num) else { continue };
        let end = num + n;
        if let Some(needs) = needs
            && !lower[end..].starts_with(needs)
        {
            continue;
        }
        let id: u32 = lower[num..end].parse().ok()?;
        if id > 0 && (from..end).contains(&at) {
            return Some(id);
        }
    }
    None
}

/// The URL in `line` covering byte `at`.
fn url_in(line: &[u8], at: usize) -> Option<String> {
    if !line.get(at).is_some_and(|&b| url_char(b)) {
        return None;
    }
    let mut start = at;
    while start > 0 && url_char(line[start - 1]) {
        start -= 1;
    }
    let mut end = at + 1;
    while end < line.len() && url_char(line[end]) {
        end += 1;
    }
    let word = std::str::from_utf8(&line[start..end]).ok()?;
    // The last scheme that starts at or before the click.
    let click = at - start;
    let from = word
        .match_indices("http")
        .map(|(i, _)| i)
        .filter(|&i| i <= click)
        .filter(|&i| word[i..].starts_with("https://") || word[i..].starts_with("http://"))
        .max()?;
    let url = trim_end(&word[from..]);
    let host = url.split_once("://")?.1;
    (from + url.len() > click && !host.is_empty()).then(|| url.to_owned())
}

/// Bytes that may be in a URL as it is printed; quotes, brackets for markup and
/// spaces end it.
fn url_char(b: u8) -> bool {
    b.is_ascii_graphic()
        && !matches!(
            b,
            b'<' | b'>' | b'"' | b'\'' | b'`' | b'{' | b'}' | b'|' | b'^' | b'\\'
        )
}

/// Drops punctuation that ends a sentence, and a closing bracket the URL didn't open.
fn trim_end(mut url: &str) -> &str {
    loop {
        let Some(last) = url.chars().last() else {
            return url;
        };
        let unopened =
            |open, close| last == close && url.matches(close).count() > url.matches(open).count();
        if matches!(last, '.' | ',' | ';' | ':' | '!' | '?' | '*' | '_')
            || unopened('(', ')')
            || unopened('[', ']')
        {
            url = &url[..url.len() - 1];
        } else {
            return url;
        }
    }
}

/// What became of a link.
#[derive(Debug, PartialEq, Eq)]
pub enum Opened {
    Browser,
    /// Over SSH: put on the clipboard of the terminal in front of you.
    Copied,
}

#[cfg(test)]
thread_local! {
    /// What tests opened, instead of a browser.
    pub static OPENED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Opens `url` in a browser on this machine, or over SSH copies it.
pub fn open(url: &str) -> std::io::Result<Opened> {
    #[cfg(test)]
    {
        OPENED.with(|o| o.borrow_mut().push(url.to_owned()));
        return Ok(Opened::Browser);
    }
    #[cfg_attr(test, allow(unreachable_code))]
    if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
        crate::mouse::copy_to_clipboard(url)?;
        return Ok(Opened::Copied);
    }
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        Command::new("explorer.exe")
    } else if is_wsl() {
        // wslu's opener if it's there, else Windows' own (it takes a URL, and
        // with no shell between, `&` and the like are safe).
        if on_path("wslview") {
            Command::new("wslview")
        } else {
            Command::new("explorer.exe")
        }
    } else {
        Command::new("xdg-open")
    };
    let mut child = command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // Reaped off the UI thread, so a slow opener never holds up a frame.
    std::thread::spawn(move || child.wait());
    Ok(Opened::Browser)
}

pub fn is_wsl() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .is_ok_and(|r| r.to_ascii_lowercase().contains("microsoft"))
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::{Span, Style};

    fn row(y: u16, text: &str, wrapped: bool) -> Row {
        Row {
            y,
            spans: vec![Span {
                x: 0,
                text: text.into(),
                style: Style::default(),
            }],
            wrapped,
        }
    }

    fn at(text: &str, x: usize) -> Option<String> {
        url_at(&[row(0, text, false)], 80, x as u16, 0)
    }

    #[test]
    fn finds_the_doc_under_the_click() {
        let doc = |text: &str, x: usize| doc_at(&[row(0, text, false)], x as u16, 0);
        let written = "doc 8 written: /home/me/docs/plan.md";
        assert_eq!(doc(written, 0), Some(8));
        assert_eq!(doc(written, 4), Some(8));
        assert_eq!(doc(written, 6), None, "past the mention");
        let read = "run valk doc read 12 now";
        assert_eq!(doc(read, 4), Some(12));
        assert_eq!(doc(read, 19), Some(12));
        assert_eq!(doc(read, 2), None);
        assert_eq!(doc("  valk doc edit 3 --base ab -", 10), Some(3));
        assert_eq!(doc("Wrote it up as doc #9.", 19), Some(9));
        for text in [
            "doc 8",
            "docs 8 written",
            "doc #0",
            "undoc #5",
            "valk doc list 3",
            "doc #12ab",
        ] {
            assert!((0..text.len()).all(|x| doc(text, x).is_none()), "{text}");
        }
    }

    #[test]
    fn finds_the_url_under_the_click() {
        let text = "see https://example.com/a?b=1&c=2 for more";
        assert_eq!(
            at(text, 4).as_deref(),
            Some("https://example.com/a?b=1&c=2")
        );
        assert_eq!(
            at(text, 20).as_deref(),
            Some("https://example.com/a?b=1&c=2")
        );
        assert_eq!(at(text, 2), None, "on a plain word");
        assert_eq!(at(text, 3), None, "on the space");
        assert_eq!(at("no links here", 4), None);
        assert_eq!(at("https:// alone", 2), None);
    }

    #[test]
    fn leaves_off_punctuation_and_markup_around_it() {
        assert_eq!(
            at("(https://x.dev/a_(b)).", 5).as_deref(),
            Some("https://x.dev/a_(b)")
        );
        assert_eq!(
            at("go to https://x.dev.", 8).as_deref(),
            Some("https://x.dev")
        );
        assert_eq!(
            at("[docs](https://x.dev/d)", 10).as_deref(),
            Some("https://x.dev/d")
        );
        assert_eq!(
            at("`http://localhost:8790`", 3).as_deref(),
            Some("http://localhost:8790")
        );
        assert_eq!(at("│https://x.dev│", 3).as_deref(), Some("https://x.dev"));
    }

    #[test]
    fn follows_a_url_onto_the_next_row() {
        // Wrapped by the terminal.
        let rows = [
            row(0, "go https://exa", true),
            row(1, "mple.com/x now", false),
        ];
        let want = Some("https://example.com/x".to_owned());
        assert_eq!(url_at(&rows, 14, 5, 0), want);
        assert_eq!(url_at(&rows, 14, 2, 1), want);
        // Broken at the edge by the program: a full row, not marked wrapped.
        let rows = [row(0, "go https://exa", false), row(1, "mple.com/x", false)];
        assert_eq!(url_at(&rows, 14, 3, 1), want);
        // A short row ends the URL.
        let rows = [row(0, "go https://ex", false), row(1, "ample", false)];
        assert_eq!(url_at(&rows, 14, 5, 0).as_deref(), Some("https://ex"));
    }

    #[test]
    fn wide_characters_before_a_url_keep_the_columns_right() {
        // 日 takes two cells, so the URL starts at column 3.
        let rows = [row(0, "日 https://x.dev", false)];
        assert_eq!(url_at(&rows, 40, 3, 0).as_deref(), Some("https://x.dev"));
        assert_eq!(url_at(&rows, 40, 1, 0), None);
    }
}

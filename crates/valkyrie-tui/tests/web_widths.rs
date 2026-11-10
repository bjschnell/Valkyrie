//! The web app counts a character's columns from a table (`web/src/lib/widths.ts`)
//! so that it agrees with the daemon's terminal, which asks the unicode-width
//! crate. This keeps the two from drifting when the crate is updated.
//!
//! To make the table again: for U+0080 to U+3FFFF, list the ranges where
//! `char::width()` is 0 (`ZERO`) and where it is 2 (`WIDE`), as pairs of first and
//! last code point.

use unicode_width::UnicodeWidthChar;

/// The numbers of the array called `name` in the table's source.
fn table(source: &str, name: &str) -> Vec<(u32, u32)> {
    let start = source
        .find(&format!("export const {name}"))
        .unwrap_or_else(|| panic!("no {name} in widths.ts"));
    let body = &source[start..];
    let body = &body[body.find("= [").unwrap() + 3..body.find("];").unwrap()];
    let numbers: Vec<u32> = body
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|n| u32::from_str_radix(n.trim_start_matches("0x"), 16).unwrap())
        .collect();
    assert_eq!(numbers.len() % 2, 0, "{name} is pairs");
    numbers.chunks(2).map(|pair| (pair[0], pair[1])).collect()
}

/// `charWidth` in `web/src/lib/screen.ts`, by the same steps.
fn web_width(zero: &[(u32, u32)], wide: &[(u32, u32)], cp: u32) -> usize {
    let within = |table: &[(u32, u32)]| table.iter().any(|&(a, b)| a <= cp && cp <= b);
    if cp < 0xad {
        1
    } else if cp > 0x3ffff {
        if (0xe0000..=0xe0fff).contains(&cp) {
            0
        } else {
            1
        }
    } else if within(zero) {
        0
    } else if within(wide) {
        2
    } else {
        1
    }
}

#[test]
fn web_widths_match_the_terminals() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../web/src/lib/widths.ts");
    let source = std::fs::read_to_string(path).unwrap();
    let (zero, wide) = (table(&source, "ZERO"), table(&source, "WIDE"));
    for table in [&zero, &wide] {
        assert!(table.windows(2).all(|w| w[0].1 < w[1].0), "sorted, apart");
    }
    let mut wrong = Vec::new();
    // From the first printable character: control characters have no width.
    for cp in 0x20..=0x10ffff_u32 {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        let Some(width) = c.width() else { continue };
        // The crate's one three-column character (a Khmer sign): a terminal's
        // cells are one or two columns, and the table has it as one.
        if width > 2 {
            continue;
        }
        let web = web_width(&zero, &wide, cp);
        if web != width {
            wrong.push(format!("U+{cp:04X}: terminal {width}, web {web}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} characters differ; make web/src/lib/widths.ts again (see this file's top):\n{}",
        wrong.len(),
        wrong[..wrong.len().min(20)].join("\n")
    );
}

//! The effects of the themes with rain (matrix, violet, cyberpunk, valkyrie): code
//! or runes falling through the home screen's empty rows, a logo that glitches into
//! the same glyphs, and a blinking cursor. All of it is stateless: a
//! column's speed, trail and phase come from a hash of its x, so a frame depends
//! only on the clock.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

use crate::theme::Rain;

fn mix(mut x: u64) -> u64 {
    // splitmix64's finalizer.
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn glyph(seed: u64, glyphs: &str) -> char {
    let n = glyphs.chars().count() as u64;
    glyphs.chars().nth((seed % n) as usize).unwrap_or(' ')
}

/// Rain through `regions`, falling the full height of `screen` so drops pass from
/// one panel into the next. Only rows of a region that are blank all the way across
/// get any: the rain runs behind text, never through it.
pub fn draw(buf: &mut Buffer, screen: Rect, regions: &[Rect], now_ms: u64, colors: &Rain) {
    let height = screen.height as u64;
    for region in regions {
        let region = region.intersection(screen);
        for y in region.top()..region.bottom() {
            let blank = (region.left()..region.right())
                .all(|x| buf.cell((x, y)).is_some_and(|c| c.symbol() == " "));
            if !blank {
                continue;
            }
            for x in region.left()..region.right() {
                if let Some((ch, shade)) = drop_at(x, y - screen.y, height, now_ms, colors.glyphs) {
                    let cell = &mut buf[(x, y)];
                    cell.set_char(ch);
                    match shade {
                        None => {
                            cell.set_fg(colors.head).modifier.insert(Modifier::BOLD);
                        }
                        Some(i) => {
                            cell.set_fg(colors.trail[i]);
                        }
                    }
                }
            }
        }
    }
}

/// What column `x` shows at row `y`: a glyph and, for the trail, its shade (an
/// index into `Rain::trail`); `None` is the head. Or nothing.
fn drop_at(
    x: u16,
    y: u16,
    height: u64,
    now_ms: u64,
    glyphs: &str,
) -> Option<(char, Option<usize>)> {
    let h = mix(x as u64);
    // About half the columns are dry.
    if h % 100 < 45 {
        return None;
    }
    let rows_per_sec = 4 + (h >> 8) % 9;
    let trail = 4 + (h >> 16) % 10;
    let gap = (h >> 24) % (height + 12);
    let cycle = height + trail + gap;
    let head = (now_ms * rows_per_sec / 1000 + (h >> 40)) % cycle;
    let behind = head.checked_sub(y as u64).filter(|&d| d < trail)?;
    // The head flickers every frame; the trail changes its glyphs slowly.
    let every = 120 + (h >> 32) % 400;
    let tick = if behind == 0 {
        now_ms / 100
    } else {
        now_ms / every
    };
    let ch = glyph(mix(h ^ ((y as u64) << 32) ^ tick), glyphs);
    if behind == 0 {
        return Some((ch, None));
    }
    let shade = (behind * 3 / trail) as usize;
    Some((ch, Some(shade.min(2))))
}

/// `text` with a letter or two swapped for rain `glyphs` during a short burst every
/// few seconds, otherwise as is.
pub fn glitch(text: &str, now_ms: u64, glyphs: &str) -> String {
    const EVERY_MS: u64 = 4_000;
    const FOR_MS: u64 = 220;
    let burst = now_ms / EVERY_MS;
    if now_ms % EVERY_MS >= FOR_MS {
        return text.to_owned();
    }
    let frame = now_ms / 60;
    let letters = text.chars().filter(|c| c.is_alphanumeric()).count() as u64;
    if letters == 0 {
        return text.to_owned();
    }
    let a = mix(burst ^ frame) % letters;
    let b = mix(!burst ^ frame) % letters;
    let mut n = 0;
    text.chars()
        .map(|c| {
            if !c.is_alphanumeric() {
                return c;
            }
            n += 1;
            if n - 1 == a || n - 1 == b {
                glyph(mix(frame ^ n), glyphs)
            } else {
                c
            }
        })
        .collect()
}

/// A block cursor that blinks about once a second.
pub fn cursor(now_ms: u64) -> &'static str {
    if (now_ms / 530).is_multiple_of(2) {
        "█"
    } else {
        " "
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GREEN: &Rain = crate::theme::MATRIX.rain.as_ref().unwrap();
    const RUNES: &Rain = crate::theme::VALKYRIE.rain.as_ref().unwrap();

    #[test]
    fn rain_keeps_out_of_text() {
        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        buf.set_string(3, 5, "hello", ratatui::style::Style::new());
        for now in (0..20_000).step_by(37) {
            let mut b = buf.clone();
            draw(&mut b, area, &[area], now, GREEN);
            let row: String = (0..40).map(|x| b[(x, 5)].symbol().to_owned()).collect();
            assert_eq!(row.trim(), "hello", "at {now}");
        }
    }

    #[test]
    fn rain_falls() {
        let area = Rect::new(0, 0, 80, 24);
        let lit = |now| {
            let mut b = Buffer::empty(area);
            draw(&mut b, area, &[area], now, GREEN);
            b.content().iter().filter(|c| c.symbol() != " ").count()
        };
        assert!((0..50).map(|i| lit(i * 1000)).any(|n| n > 0));
        let mut a = Buffer::empty(area);
        let mut b = Buffer::empty(area);
        draw(&mut a, area, &[area], 1_000, GREEN);
        draw(&mut b, area, &[area], 2_000, GREEN);
        assert_ne!(a, b, "a second later the rain has moved");
    }

    #[test]
    fn runes_fall() {
        let area = Rect::new(0, 0, 80, 24);
        let mut b = Buffer::empty(area);
        draw(&mut b, area, &[area], 5_000, RUNES);
        let lit: Vec<&str> = b
            .content()
            .iter()
            .map(|c| c.symbol())
            .filter(|s| *s != " ")
            .collect();
        assert!(!lit.is_empty());
        assert!(lit.iter().all(|s| RUNES.glyphs.contains(s)), "{lit:?}");
    }

    #[test]
    fn glitch_keeps_shape() {
        for now in (0..10_000).step_by(13) {
            let g = glitch("◆ VALKYRIE", now, GREEN.glyphs);
            assert_eq!(g.chars().count(), "◆ VALKYRIE".chars().count());
            assert!(g.starts_with("◆ "));
        }
        assert_eq!(glitch("VALKYRIE", 1_000, GREEN.glyphs), "VALKYRIE");
        assert_ne!(
            (0..220)
                .map(|t| glitch("VALKYRIE", t, GREEN.glyphs))
                .filter(|g| g != "VALKYRIE")
                .count(),
            0
        );
    }
}

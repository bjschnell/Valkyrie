//! A theme pack's avatar: a figure made of the rain on the home screen. The pack
//! ships her as frames of glyphs (`tui/avatar.json`, made from its art by the
//! pack's own tools) at a few sizes; the largest that fits is drawn at the foot
//! of the emptier panel, in the rows the rain may use, so she stays behind text.
//!
//! What she shows follows the sessions, which only a pack may do (a built-in
//! look never reacts to state): with nothing going on she is latent, only a
//! glint where drops fall through her; while agents work she gathers out of the
//! rain, a little more of her the longer it runs; when something needs you she
//! is formed. Like the rain, a frame depends only on the clock.

use std::collections::HashMap;
use std::sync::OnceLock;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use serde::Deserialize;

use crate::rain;
use crate::theme::{Rain, Theme};

/// Her body is rain: each tone band is a pool of glyphs that keeps sliding down
/// its column.
const BANDS: [&str; 5] = [".,:'`", ":;-~=", "=+x*o", "xo*%#", "#%&@8B"];
/// How long gathering takes to show all of her.
const GATHER_MS: u64 = 45_000;
/// The longest a frame may hold, so a loop's length can't overflow.
const MAX_FRAME_MS: u64 = 3_600_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    /// Nothing is going on: she is a glint in the rain.
    Latent,
    /// Agents are working, since this time (ms since the epoch).
    Gathering { since_ms: u64 },
    /// Something needs you.
    Formed,
}

pub struct Avatar {
    sizes: Vec<Size>,
    blink: Vec<(String, u64)>,
    /// The face she shows when formed, if the pack has one.
    formed: Option<String>,
}

struct Size {
    rows: u16,
    cols: u16,
    frames: HashMap<String, Vec<Cell>>,
}

/// One cell of a frame: what's drawn there and how bright (0..=9).
#[derive(Clone, Copy)]
struct Cell {
    /// `' '` is not her; `'.'` is her body, drawn from the band for `level`; any
    /// other glyph is a line of hers, drawn as is.
    glyph: char,
    level: u8,
}

#[derive(Deserialize)]
struct File {
    sizes: Vec<FileSize>,
    blink: Vec<FileBlink>,
    #[serde(default)]
    formed: Option<String>,
}

#[derive(Deserialize)]
struct FileSize {
    rows: u16,
    cols: u16,
    frames: HashMap<String, FileFrame>,
}

#[derive(Deserialize)]
struct FileFrame {
    glyphs: Vec<String>,
    levels: Vec<String>,
}

#[derive(Deserialize)]
struct FileBlink {
    frame: String,
    ms: u64,
}

impl Avatar {
    pub fn parse(json: &str) -> Result<Avatar, String> {
        let file: File = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut sizes = Vec::new();
        for size in file.sizes {
            let mut frames = HashMap::new();
            for (name, frame) in size.frames {
                if frame.glyphs.len() != size.rows as usize
                    || frame.levels.len() != size.rows as usize
                {
                    return Err(format!(
                        "{name} at {}x{}: wrong number of rows",
                        size.cols, size.rows
                    ));
                }
                let mut cells = Vec::with_capacity(size.rows as usize * size.cols as usize);
                for (glyphs, levels) in frame.glyphs.iter().zip(&frame.levels) {
                    let glyphs: Vec<char> = glyphs.chars().collect();
                    let levels: Vec<char> = levels.chars().collect();
                    if glyphs.len() != size.cols as usize || levels.len() != size.cols as usize {
                        return Err(format!(
                            "{name} at {}x{}: wrong row width",
                            size.cols, size.rows
                        ));
                    }
                    for (&glyph, &level) in glyphs.iter().zip(&levels) {
                        let glyph = if unicode_width::UnicodeWidthChar::width(glyph) == Some(1) {
                            glyph
                        } else {
                            ' '
                        };
                        cells.push(Cell {
                            glyph,
                            level: level.to_digit(10).unwrap_or(0) as u8,
                        });
                    }
                }
                frames.insert(name, cells);
            }
            sizes.push(Size {
                rows: size.rows,
                cols: size.cols,
                frames,
            });
        }
        sizes.sort_by_key(|s| s.rows);
        let blink: Vec<(String, u64)> = file
            .blink
            .into_iter()
            .map(|b| (b.frame, b.ms.clamp(1, MAX_FRAME_MS)))
            .collect();
        if blink.is_empty() {
            return Err("no blink frames".into());
        }
        if let Some(missing) = sizes.iter().find_map(|s| {
            blink
                .iter()
                .map(|(f, _)| f)
                .chain(file.formed.as_ref())
                .find(|f| !s.frames.contains_key(*f))
        }) {
            return Err(format!("frame {missing} is missing at some size"));
        }
        Ok(Avatar {
            sizes,
            blink,
            formed: file.formed,
        })
    }

    /// The frame to show at `now_ms`: the blink loop, or the formed face.
    fn frame<'a>(&'a self, size: &'a Size, mood: Mood, now_ms: u64) -> Option<&'a [Cell]> {
        if mood == Mood::Formed
            && let Some(face) = self.formed.as_ref().and_then(|f| size.frames.get(f))
        {
            return Some(face);
        }
        let total: u64 = self.blink.iter().map(|(_, ms)| ms).sum();
        let mut t = now_ms % total;
        let name = self
            .blink
            .iter()
            .find(|(_, ms)| {
                let here = t < *ms;
                t = t.saturating_sub(*ms);
                here
            })
            .map_or(&self.blink[0].0, |(f, _)| f);
        size.frames.get(name).map(Vec::as_slice)
    }
}

/// The avatar of `theme`, if it comes from a pack that has one. Read once.
pub fn of(theme: &Theme) -> Option<&'static Avatar> {
    if cfg!(test) {
        return None;
    }
    static ALL: OnceLock<HashMap<String, Avatar>> = OnceLock::new();
    ALL.get_or_init(|| {
        valkyrie_proto::themes::load()
            .into_iter()
            // A pack named like a built-in isn't offered, so its avatar mustn't
            // show on that built-in: those never react to state.
            .filter(|pack| {
                !crate::theme::THEMES
                    .iter()
                    .any(|t| t.name.eq_ignore_ascii_case(&pack.name))
            })
            .filter_map(|pack| {
                let json = std::fs::read_to_string(pack.dir.join("tui/avatar.json")).ok()?;
                Avatar::parse(&json).ok().map(|a| (pack.name, a))
            })
            .collect()
    })
    .get(theme.name)
}

/// Where she goes: the region among `regions` with the most rows free for rain
/// (`free`, from `rain::free_rows` before the rain is drawn), and which of its
/// rows those are.
pub fn pick(regions: &[Rect], free: &[Vec<bool>]) -> Option<usize> {
    (0..regions.len()).max_by_key(|&i| free[i].iter().filter(|&&f| f).count())
}

/// Draws `avatar` at the foot of `region`, centered, in the rows `free` marks
/// (the rows rain may use), in the colors of `t`'s rain. Rain is drawn first;
/// she is drawn over it.
pub fn draw(
    buf: &mut Buffer,
    screen: Rect,
    (region, free): (Rect, &[bool]),
    now_ms: u64,
    t: &Theme,
    avatar: &Avatar,
    mood: Mood,
) {
    let Some(colors) = &t.rain else {
        return;
    };
    let Some(size) = avatar
        .sizes
        .iter()
        .rev()
        .find(|s| s.rows <= region.height && s.cols <= region.width)
    else {
        return;
    };
    let Some(frame) = avatar.frame(size, mood, now_ms) else {
        return;
    };
    let left = region.x + (region.width - size.cols) / 2;
    let top = region.bottom() - size.rows;
    let gathered = match mood {
        Mood::Gathering { since_ms } => {
            (now_ms.saturating_sub(since_ms) as f32 / GATHER_MS as f32).min(1.0)
        }
        _ => 0.0,
    };
    for r in 0..size.rows {
        let y = top + r;
        if !free.get((y - region.y) as usize).copied().unwrap_or(false) {
            continue;
        }
        for c in 0..size.cols {
            let cell = frame[r as usize * size.cols as usize + c as usize];
            if cell.glyph == ' ' || cell.level == 0 {
                continue;
            }
            let x = left + c;
            let level = cell.level as f32 / 9.0;
            let line = cell.glyph != '.';
            let drop = rain::drop_at(x, y - screen.y, screen.height as u64, now_ms, colors.glyphs);
            let seed = rain::mix(((x as u64) << 32) | y as u64);
            // Gathering lets her lines stick first, then her body.
            let reach = if line {
                (gathered * 1.6 + 0.1).min(1.0)
            } else {
                gathered
            };
            let stuck = gathered > 0.0 && ((seed % 1000) as f32) < reach * 1000.0;
            let brightness = match (mood, drop) {
                (Mood::Formed, Some((_, None))) => (0.35 + 0.85 * level).min(1.0),
                (Mood::Formed, _) => 0.15 + 0.8 * level,
                (_, _) if stuck => 0.1 + 0.5 * level,
                // A trail falling through her spells her for a moment.
                (_, Some((_, Some(shade)))) => (0.35 + 0.6 * level) * (1.0 - 0.3 * shade as f32),
                _ => continue,
            };
            if brightness < 0.08 {
                continue;
            }
            let glyph = if line {
                cell.glyph
            } else {
                let band = BANDS[((level * BANDS.len() as f32) as usize).min(BANDS.len() - 1)];
                // The body slides down its column, so even formed she is still rain.
                let rate = 3 + rain::mix(x as u64) % 6;
                let slid = (y as u64).wrapping_sub(now_ms * rate / 1000);
                band.chars()
                    .nth(
                        (rain::mix(slid ^ ((x as u64) << 40)) % band.chars().count() as u64)
                            as usize,
                    )
                    .unwrap_or('.')
            };
            let out = &mut buf[(x, y)];
            out.set_char(glyph);
            out.set_fg(ramp(t.bg, colors, brightness));
            if brightness > 0.8 {
                out.modifier.insert(Modifier::BOLD);
            } else {
                out.modifier.remove(Modifier::BOLD);
            }
        }
    }
}

/// Her color at `v` (0..1): from the page up the rain's trail to its head.
fn ramp(bg: Color, rain: &Rain, v: f32) -> Color {
    let stops = [bg, rain.trail[2], rain.trail[1], rain.trail[0], rain.head];
    let at = v.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
    let i = (at as usize).min(stops.len() - 2);
    blend(stops[i], stops[i + 1], at - i as f32)
}

fn blend(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
            Color::Rgb(f(r1, r2), f(g1, g2), f(b1, b2))
        }
        _ => b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNES: &Rain = crate::theme::VALKYRIE.rain.as_ref().unwrap();

    fn avatar() -> Avatar {
        // A 4x3 figure: a line across the top, body below.
        let frame = |top: &str| {
            serde_json::json!({
                "glyphs": [top, ".  .", "...."],
                "levels": ["9999", "5005", "9999"],
            })
        };
        let json = serde_json::json!({
            "sizes": [{"rows": 3, "cols": 4, "frames": {
                "open": frame("----"), "shut": frame("____"), "worried": frame("\\\\//"),
            }}],
            "blink": [{"frame": "open", "ms": 3000}, {"frame": "shut", "ms": 100}],
            "formed": "worried",
        });
        Avatar::parse(&json.to_string()).unwrap()
    }

    fn drawn(mood: Mood, now: u64) -> (Buffer, Rect) {
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        draw(
            &mut buf,
            area,
            (area, &[true; 10]),
            now,
            &crate::theme::VALKYRIE,
            &avatar(),
            mood,
        );
        (buf, area)
    }

    fn row(buf: &Buffer, y: u16) -> String {
        (8..12).map(|x| buf[(x, y)].symbol().to_owned()).collect()
    }

    #[test]
    fn formed_she_stands_at_the_foot_centered() {
        let (buf, _) = drawn(Mood::Formed, 1_000);
        assert_eq!(row(&buf, 7), "\\\\//", "the formed face");
        assert_eq!(row(&buf, 6), "    ");
        let body = row(&buf, 9);
        assert!(body.chars().all(|c| c != ' '), "{body}");
    }

    #[test]
    fn she_blinks() {
        assert_eq!(row(&drawn(Mood::Formed, 0).0, 7), "\\\\//");
        let avatar = avatar();
        let size = &avatar.sizes[0];
        let open = avatar.frame(size, Mood::Latent, 10).unwrap()[0].glyph;
        let shut = avatar.frame(size, Mood::Latent, 3_050).unwrap()[0].glyph;
        assert_eq!((open, shut), ('-', '_'));
    }

    #[test]
    fn latent_she_only_glints_where_rain_falls() {
        for now in (0..20_000).step_by(250) {
            let (buf, area) = drawn(Mood::Latent, now);
            for y in 7..10 {
                for x in 8..12 {
                    if buf[(x, y)].symbol() != " " {
                        assert!(
                            rain::drop_at(x, y, area.height as u64, now, RUNES.glyphs).is_some(),
                            "({x},{y}) at {now}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn gathering_shows_more_of_her_the_longer_it_runs() {
        let lit = |since: u64| {
            let (buf, _) = drawn(Mood::Gathering { since_ms: since }, 100_000);
            (7..10)
                .flat_map(|y| (8..12).map(move |x| (x, y)))
                .filter(|&p| buf[p].symbol() != " ")
                .count()
        };
        assert!(lit(100_000 - 44_000) >= lit(100_000 - 2_000));
        assert_eq!(lit(0), 10, "all of her after a long run");
    }

    #[test]
    fn she_keeps_out_of_rows_with_text() {
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        let mut free = [true; 10];
        free[8] = false;
        draw(
            &mut buf,
            area,
            (area, &free),
            1_000,
            &crate::theme::VALKYRIE,
            &avatar(),
            Mood::Formed,
        );
        assert_eq!(row(&buf, 8), "    ");
        assert_ne!(row(&buf, 9), "    ");
    }

    #[test]
    fn too_small_a_panel_shows_nothing() {
        let area = Rect::new(0, 0, 3, 2);
        let mut buf = Buffer::empty(area);
        draw(
            &mut buf,
            area,
            (area, &[true; 2]),
            1_000,
            &crate::theme::VALKYRIE,
            &avatar(),
            Mood::Formed,
        );
        assert!(buf.content().iter().all(|c| c.symbol() == " "));
    }

    #[test]
    fn huge_frame_times_are_held_in_bounds() {
        let json = serde_json::json!({
            "sizes": [{"rows": 1, "cols": 1, "frames": {"a": {"glyphs": ["-"], "levels": ["9"]}}}],
            "blink": [{"frame": "a", "ms": u64::MAX}, {"frame": "a", "ms": u64::MAX}],
        });
        let avatar = Avatar::parse(&json.to_string()).unwrap();
        assert!(
            avatar
                .frame(&avatar.sizes[0], Mood::Latent, u64::MAX)
                .is_some()
        );
    }

    #[test]
    fn bad_files_are_refused() {
        assert!(Avatar::parse("{}").is_err());
        let short = serde_json::json!({
            "sizes": [{"rows": 2, "cols": 2, "frames": {"a": {"glyphs": ["--"], "levels": ["99"]}}}],
            "blink": [{"frame": "a", "ms": 10}],
        });
        assert!(Avatar::parse(&short.to_string()).is_err());
        let missing = serde_json::json!({
            "sizes": [{"rows": 1, "cols": 2, "frames": {"a": {"glyphs": ["--"], "levels": ["99"]}}}],
            "blink": [{"frame": "b", "ms": 10}],
        });
        assert!(Avatar::parse(&missing.to_string()).is_err());
    }
}

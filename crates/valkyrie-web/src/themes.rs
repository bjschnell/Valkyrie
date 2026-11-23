//! Theme packs for the web app (see `valkyrie_proto::themes`): the list, each
//! with the CSS variables its look sets, and its module's files.
//!
//! A module and the files it loads are fetched by the browser itself (an
//! `import()`, an image), which can't carry the device's token, so they are
//! served under a path with a key in it instead: random for each run of the
//! server, and handed out only with the list, which needs a paired device.

use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::sync::OnceLock;

use serde::Serialize;
use valkyrie_proto::themes::{self, Pack};

#[derive(Debug, Serialize, PartialEq)]
pub struct PackInfo {
    pub name: String,
    pub about: String,
    /// The CSS variables of its look, `--page` and the rest.
    pub vars: BTreeMap<String, String>,
    /// Its rain's glyphs, when it has rain.
    pub glyphs: Option<String>,
    /// Where its module's files are served from, ending in `/`, when it has one.
    pub base: Option<String>,
}

/// The key in this run's pack URLs.
pub fn key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("no randomness for the theme key");
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    })
}

pub fn list() -> Vec<PackInfo> {
    themes::load().iter().filter_map(info).collect()
}

fn info(pack: &Pack) -> Option<PackInfo> {
    let vars = vars(pack)?;
    Some(PackInfo {
        name: pack.name.clone(),
        about: pack.about().to_owned(),
        vars,
        glyphs: pack.get("rain.glyphs").map(str::to_owned),
        base: pack
            .has_web_module()
            .then(|| format!("/themes/{}/{}/", key(), pack.name)),
    })
}

/// The file at `rest` in pack `name`'s web folder, if `key` is this run's and the
/// path stays inside that folder.
pub fn file(key_given: &str, name: &str, rest: &str) -> Option<Vec<u8>> {
    if key_given != key() || !themes::valid_name(name) {
        return None;
    }
    let rel = Path::new(rest);
    if !servable(rest) || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
        return None;
    }
    let root = themes::dir().join(name).join("web").canonicalize().ok()?;
    let path = root.join(rel).canonicalize().ok()?;
    // A link inside the folder mustn't lead out of it.
    if !path.starts_with(&root) || !path.is_file() {
        return None;
    }
    std::fs::read(path).ok()
}

/// What a module loads: scripts, data, styles, images and fonts. Never a page or
/// an SVG, which would run in the app's origin if opened directly.
pub fn servable(rest: &str) -> bool {
    let ext = rest.rsplit_once('.').map_or("", |(_, e)| e);
    matches!(
        ext,
        "js" | "mjs" | "json" | "css" | "png" | "webp" | "jpg" | "jpeg" | "gif" | "woff2" | "wasm"
    )
}

/// The web app's CSS variables for `pack`: what its theme.toml names, derived as
/// the TUI does where it doesn't, then any `web.<var>` as is. `None` without the
/// three colors a look needs.
fn vars(pack: &Pack) -> Option<BTreeMap<String, String>> {
    let color = |key: &str| pack.get(key).and_then(themes::rgb);
    let bg = color("bg")?;
    let fg = color("fg")?;
    let accent = color("accent")?;
    let or = |key: &str, default: [u8; 3]| color(key).unwrap_or(default);
    let muted = or("muted", mix(fg, bg, 0.55));
    let blocked = or("blocked", [0xff, 0x55, 0x55]);
    let trail = pack
        .get("rain.trail")
        .map(themes::list)
        .and_then(|l| l.first().and_then(|c| themes::rgb(c)));
    let head = or("rain.head", fg);
    let mut v: Vec<(&str, [u8; 3])> = vec![
        ("--page", bg),
        ("--surface", or("panel", mix(bg, fg, 0.04))),
        ("--card", or("card", mix(bg, fg, 0.08))),
        ("--raised", or("selection", mix(bg, accent, 0.25))),
        ("--line", or("border", mix(bg, fg, 0.2))),
        ("--fg", fg),
        ("--muted", muted),
        ("--soft", mix(fg, muted, 0.45)),
        ("--accent", accent),
        ("--accent2", trail.unwrap_or(accent)),
        ("--needs", or("needs", [0xf1, 0xfa, 0x8c])),
        ("--blocked", blocked),
        ("--done", or("done", [0x50, 0xfa, 0x7b])),
        ("--working", or("working", accent)),
        (
            "--interrupted",
            or("interrupted", mix(blocked, [0xff, 0xb0, 0x40], 0.5)),
        ),
        ("--idle", mix(muted, bg, 0.1)),
        ("--term-bg", or("panel", mix(bg, fg, 0.04))),
        ("--term-fg", fg),
        ("--on-accent", or("strip", mix(bg, [0, 0, 0], 0.4))),
    ];
    let ansi: Vec<[u8; 3]> = pack
        .get("ansi")
        .map(themes::list)
        .unwrap_or_default()
        .into_iter()
        .filter_map(themes::rgb)
        .collect();
    let names: Vec<String> = (0..16).map(|i| format!("--ansi-{i}")).collect();
    for (i, name) in names.iter().enumerate() {
        if let Some(&c) = ansi.get(i) {
            v.push((name, c));
        }
    }
    let mut out: BTreeMap<String, String> =
        v.into_iter().map(|(k, c)| (k.to_owned(), hex(c))).collect();
    if pack.get("rain.glyphs").is_some() {
        out.insert("--font".into(), "var(--mono)".into());
        out.insert(
            "--glow".into(),
            format!("{} {} {}", accent[0], accent[1], accent[2]),
        );
        out.insert("--glitch-a".into(), hex(accent));
        out.insert("--glitch-b".into(), hex(head));
        out.insert("--rain-head".into(), hex(head));
        out.insert("--rain-trail".into(), hex(trail.unwrap_or(accent)));
    }
    for (key, value) in &pack.values {
        if let Some(var) = key.strip_prefix("web.")
            && !var.is_empty()
            && var.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && css_value_safe(value)
        {
            out.insert(format!("--{var}"), value.clone());
        }
    }
    Some(out)
}

/// A value that stays inside its own declaration: none of `{ } ; < \\ " '`, no
/// comment, and its brackets balanced, so one pack's slip can't swallow the next
/// rule (the web app checks the same before writing them).
fn css_value_safe(value: &str) -> bool {
    if value.contains(['{', '}', ';', '<', '\\', '"', '\'']) || value.contains("/*") {
        return false;
    }
    let mut stack = Vec::new();
    for c in value.chars() {
        match c {
            '(' => stack.push(')'),
            '[' => stack.push(']'),
            ')' | ']' if stack.pop() != Some(c) => return false,
            _ => {}
        }
    }
    stack.is_empty()
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])]
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(values: &[(&str, &str)]) -> Pack {
        Pack {
            name: "mine".into(),
            dir: std::path::PathBuf::from("/nowhere/mine"),
            values: values
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn a_look_needs_three_colors() {
        assert!(info(&pack(&[("bg", "#000000"), ("fg", "#ffffff")])).is_none());
        let i = info(&pack(&[
            ("bg", "#000000"),
            ("fg", "#ffffff"),
            ("accent", "#ff00ff"),
        ]))
        .unwrap();
        assert_eq!(i.vars["--page"], "#000000");
        assert_eq!(i.vars["--accent"], "#ff00ff");
        assert!(!i.vars.contains_key("--rain-head"));
        assert_eq!(i.glyphs, None);
        assert_eq!(i.base, None);
    }

    #[test]
    fn rain_and_web_overrides() {
        let i = info(&pack(&[
            ("bg", "#07050d"),
            ("fg", "#efe6ff"),
            ("accent", "#b46bff"),
            ("rain.glyphs", "ｱｲ01"),
            ("rain.trail", "#c03ae0 #500070"),
            ("web.accent2", "#ff3ed8"),
            ("web.bad}", "x"),
            ("web.glow", "1 2 3; } body { x"),
        ]))
        .unwrap();
        assert_eq!(i.glyphs.as_deref(), Some("ｱｲ01"));
        assert_eq!(i.vars["--rain-trail"], "#c03ae0");
        assert_eq!(i.vars["--accent2"], "#ff3ed8");
        assert_eq!(
            i.vars["--glow"], "180 107 255",
            "a value that escapes its rule is dropped"
        );
        assert!(!i.vars.keys().any(|k| k.contains('}')));
    }

    #[test]
    fn a_value_stays_in_its_declaration() {
        for bad in ["calc(1px", "x /* y", "a\\", "\"x\"", "a)(", "[x)"] {
            assert!(!css_value_safe(bad), "{bad}");
        }
        for good in [
            "#fff",
            "0.85",
            "calc((1px + 2px) * 3)",
            "var(--mono)",
            "180 107 255",
        ] {
            assert!(css_value_safe(good), "{good}");
        }
    }

    #[test]
    fn only_what_a_module_loads_is_served() {
        for ok in ["index.js", "a/b.json", "x.webp", "f.woff2", "s.css"] {
            assert!(servable(ok), "{ok}");
        }
        for no in ["page.html", "x.svg", "theme.toml", "noext"] {
            assert!(!servable(no), "{no}");
        }
    }

    #[test]
    fn files_need_the_key_and_stay_in_the_folder() {
        assert!(file("wrong", "neon", "index.js").is_none());
        for bad in ["../theme.toml", "/etc/passwd", "", "a/../../x"] {
            assert!(file(key(), "neon", bad).is_none(), "{bad}");
        }
        assert!(file(key(), "../x", "index.js").is_none());
    }
}

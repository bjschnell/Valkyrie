//! Theme packs: looks that live on one machine rather than in Valkyrie. Each is a
//! folder in `themes/` under the config directory holding a `theme.toml` (plain
//! `key = value` lines, like `settings.toml`): its colors, its rain, and, for the
//! web app, any CSS variable to set as is (`web.<name> = value`). The TUI and the
//! web app offer a pack beside the built-in looks on the machine that has it, and
//! nowhere else.
//!
//! A pack may also bring an avatar: `tui/avatar.json` (frames of glyphs the TUI
//! draws in its rain) and `web/index.js` (a module the web app mounts in place
//! of its rain, with the files beside it). Both are told what the sessions are
//! doing, which a built-in look never is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One pack, as read from its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    /// Its folder's name, lowercase: what settings and the web app call it.
    pub name: String,
    pub dir: PathBuf,
    /// Every `key = value` in its theme.toml, quotes taken off.
    pub values: BTreeMap<String, String>,
}

impl Pack {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// A few words on it, for the pickers.
    pub fn about(&self) -> &str {
        self.get("about").unwrap_or("A theme pack on this machine")
    }

    /// The folder the web app's module and its files are served from.
    pub fn web_dir(&self) -> PathBuf {
        self.dir.join("web")
    }

    /// Whether the web app mounts a module of the pack's in place of its rain.
    pub fn has_web_module(&self) -> bool {
        self.web_dir().join("index.js").is_file()
    }
}

/// Where packs live: `themes/` in the config directory.
pub fn dir() -> PathBuf {
    crate::config_dir().join("themes")
}

/// The packs in `dir()`, by name. A folder without a theme.toml, or whose name
/// isn't lowercase letters, digits and dashes, is passed over.
pub fn load() -> Vec<Pack> {
    load_from(&dir())
}

pub fn load_from(dir: &Path) -> Vec<Pack> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut packs: Vec<Pack> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            if !valid_name(&name) {
                return None;
            }
            let dir = entry.path();
            let text = std::fs::read_to_string(dir.join("theme.toml")).ok()?;
            Some(Pack {
                name,
                dir,
                values: parse(&text),
            })
        })
        .collect();
    packs.sort_by(|a, b| a.name.cmp(&b.name));
    packs
}

/// A name safe in a URL path, a CSS selector and settings.toml.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-')
}

/// `key = value` lines; ` #` starts a comment outside quotes, and a value's
/// surrounding quotes are dropped. Section headers aren't used: keys are dotted.
fn parse(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.starts_with('#') {
            continue;
        }
        let value = value.trim();
        let value = if let Some(rest) = value.strip_prefix('"') {
            match rest.find('"') {
                Some(end) => &rest[..end],
                None => rest,
            }
        } else {
            // A comment starts at a `#` after a space, so a bare `#rrggbb` is a value.
            value
                .find([' ', '\t'])
                .and_then(|at| value[at..].find('#').map(|h| at + h))
                .map_or(value, |end| &value[..end])
                .trim()
        };
        out.insert(key.to_owned(), value.to_owned());
    }
    out
}

/// `#rrggbb` as its three bytes.
pub fn rgb(value: &str) -> Option<[u8; 3]> {
    let hex = value.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// A list value: colors or words split on spaces and commas.
pub fn list(value: &str) -> Vec<&str> {
    value
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_values() {
        let v = parse(
            "# a pack\nabout = \"Neon # and rain\"  # trailing\nbg = #0b0a14 # comment\n\
             rain.trail = \"#ff00ff #aa00aa #330033\"\nnot a line\n",
        );
        assert_eq!(v["about"], "Neon # and rain");
        assert_eq!(v["bg"], "#0b0a14");
        assert_eq!(list(&v["rain.trail"]).len(), 3);
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn colors() {
        assert_eq!(rgb("#0b0A14"), Some([0x0b, 0x0a, 0x14]));
        assert_eq!(rgb("0b0a14"), None);
        assert_eq!(rgb("#0b0a1"), None);
        assert_eq!(rgb("#ééé"), None);
    }

    #[test]
    fn names() {
        assert!(valid_name("neon"));
        assert!(valid_name("my-look-2"));
        for bad in ["", "Neon", "../x", "a b", "-x", "a.b"] {
            assert!(!valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn loads_folders_with_a_theme() {
        let root = std::env::temp_dir().join(format!("valk-packs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (name, toml) in [
            ("zed", Some("bg = #000000")),
            ("neon", Some("about = Hers")),
            ("empty", None),
            ("Bad", Some("")),
        ] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            if let Some(t) = toml {
                std::fs::write(root.join(name).join("theme.toml"), t).unwrap();
            }
        }
        let packs = load_from(&root);
        let names: Vec<&str> = packs.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["neon", "zed"]);
        assert_eq!(packs[0].about(), "Hers");
        assert!(!packs[0].has_web_module());
        std::fs::remove_dir_all(&root).unwrap();
        assert!(load_from(&root).is_empty());
    }
}

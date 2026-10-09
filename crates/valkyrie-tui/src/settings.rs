//! What the TUI looks and sounds like: the theme and whether sessions take its
//! colors, how the tabs are drawn and on which side, and pings on or off. Kept in
//! `settings.toml` in Valkyrie's config directory, which `,` edits (or any editor:
//! it is plain `key = value`). At start `$VALK_THEME`, `$VALK_SESSION_COLORS`,
//! `$VALK_TABS`, `$VALK_TAB_SIDE` and `$VALK_SOUND` win over it.

use crate::theme::{DRACULA, THEMES, Theme};
use std::path::{Path, PathBuf};

/// How the tab strip is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabStyle {
    /// Flat tabs on the page, the attached one underlined in the accent.
    Underline,
    /// Flat tabs; the attached one has an accent bar along its top and no rule
    /// under it, so it opens into the session like a folder's tab.
    Folder,
    /// Cards on a darker strip, the attached one marked with a bar.
    Cards,
}

/// Where the tabs are: a strip above the session, or a column beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabSide {
    Top,
    Left,
    Right,
}

impl TabStyle {
    pub const ALL: &[TabStyle] = &[TabStyle::Underline, TabStyle::Folder, TabStyle::Cards];

    pub fn name(self) -> &'static str {
        match self {
            TabStyle::Underline => "underline",
            TabStyle::Folder => "folder",
            TabStyle::Cards => "cards",
        }
    }

    /// Rows a strip on top takes: the name, then what runs and its state; under
    /// flat tabs their rule, and over folder tabs the attached one's bar.
    pub fn rows(self) -> u16 {
        match self {
            TabStyle::Folder => 4,
            TabStyle::Underline => 3,
            TabStyle::Cards => 2,
        }
    }
}

impl TabSide {
    pub const ALL: &[TabSide] = &[TabSide::Top, TabSide::Left, TabSide::Right];

    pub fn name(self) -> &'static str {
        match self {
            TabSide::Top => "top",
            TabSide::Left => "left",
            TabSide::Right => "right",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Settings {
    pub theme: &'static Theme,
    /// Sessions draw their default and ANSI colors from the theme, not from the
    /// terminal Valkyrie runs in.
    pub themed_sessions: bool,
    pub tabs: TabStyle,
    pub tab_side: TabSide,
    pub sound: bool,
}

impl PartialEq for Settings {
    fn eq(&self, other: &Self) -> bool {
        (
            self.theme.name,
            self.themed_sessions,
            self.tabs,
            self.tab_side,
            self.sound,
        ) == (
            other.theme.name,
            other.themed_sessions,
            other.tabs,
            other.tab_side,
            other.sound,
        )
    }
}

impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.to_toml())
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: &DRACULA,
            themed_sessions: true,
            tabs: TabStyle::Underline,
            tab_side: TabSide::Top,
            sound: true,
        }
    }
}

/// The rows of the settings panel, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Theme,
    SessionColors,
    Tabs,
    TabSide,
    Sound,
}

impl Field {
    pub const ALL: &[Field] = &[
        Field::Theme,
        Field::SessionColors,
        Field::Tabs,
        Field::TabSide,
        Field::Sound,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Field::Theme => "Theme",
            Field::SessionColors => "Sessions",
            Field::Tabs => "Tab style",
            Field::TabSide => "Tabs on",
            Field::Sound => "Sound",
        }
    }
}

impl Settings {
    /// The file, else the picks older versions kept one per file, then the
    /// environment over either.
    pub fn load() -> Settings {
        let mut settings = Settings::kept(&path());
        for (key, var) in [
            ("theme", "VALK_THEME"),
            ("session_colors", "VALK_SESSION_COLORS"),
            ("tabs", "VALK_TABS"),
            ("tab_side", "VALK_TAB_SIDE"),
            ("sound", "VALK_SOUND"),
        ] {
            if let Ok(value) = std::env::var(var) {
                settings.set(key, &value);
            }
        }
        settings
    }

    /// What `path` holds, else the picks older versions kept one per file; no
    /// environment.
    pub fn kept(path: &Path) -> Settings {
        let mut settings = Settings::default();
        match std::fs::read_to_string(path) {
            Ok(text) => settings.apply(&text),
            Err(_) => settings.apply_legacy(),
        }
        settings
    }

    /// Keeps `field` as it is in `self`, and only it: the rest of the file stays as
    /// it is, so neither an environment override nor another TUI's older picks
    /// are written over it. Failing to save is not worth an error.
    pub fn save_field(&self, field: Field, path: &Path) {
        let mut kept = Settings::kept(path);
        match field {
            Field::Theme => kept.theme = self.theme,
            Field::SessionColors => kept.themed_sessions = self.themed_sessions,
            Field::Tabs => kept.tabs = self.tabs,
            Field::TabSide => kept.tab_side = self.tab_side,
            Field::Sound => kept.sound = self.sound,
        }
        let Some(dir) = path.parent() else { return };
        let _ = std::fs::create_dir_all(dir);
        // Whole or not at all: a new file renamed over the old.
        let new = dir.join(".settings.toml.new");
        if std::fs::write(&new, kept.to_toml()).is_ok() {
            let _ = std::fs::rename(&new, path);
        }
    }

    pub fn to_toml(self) -> String {
        format!(
            "# Valkyrie's TUI settings. `,` in valk changes them too.\n\
             theme = \"{}\"     # {}\n\
             session_colors = \"{}\"  # theme | terminal\n\
             tabs = \"{}\"      # {}\n\
             tab_side = \"{}\"  # {}\n\
             sound = {}\n",
            self.theme.name,
            names(THEMES.iter().map(|t| t.name)),
            self.value(Field::SessionColors),
            self.tabs.name(),
            names(TabStyle::ALL.iter().map(|s| s.name())),
            self.tab_side.name(),
            names(TabSide::ALL.iter().map(|s| s.name())),
            self.sound,
        )
    }

    /// Takes `key = value` lines (a flat TOML file); unknown keys, unknown values
    /// and anything else are skipped, so a typo costs one setting, not all of them.
    pub fn apply(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("");
            if let Some((key, value)) = line.split_once('=') {
                let value = value.trim();
                let value = value.trim_matches('"').trim_matches('\'');
                self.set(key.trim(), value);
            }
        }
    }

    fn set(&mut self, key: &str, value: &str) {
        let value = value.trim();
        match key {
            "theme" => {
                if let Some(theme) = Theme::by_name(value) {
                    self.theme = theme;
                }
            }
            "session_colors" => match value {
                "theme" => self.themed_sessions = true,
                "terminal" => self.themed_sessions = false,
                _ => {}
            },
            "tabs" => {
                if let Some(&style) = find(TabStyle::ALL, value, |s| s.name()) {
                    self.tabs = style;
                }
            }
            "tab_side" => {
                if let Some(&side) = find(TabSide::ALL, value, |s| s.name()) {
                    self.tab_side = side;
                }
            }
            "sound" => match value {
                "true" | "on" => self.sound = true,
                "false" | "off" => self.sound = false,
                _ => {}
            },
            _ => {}
        }
    }

    /// Before settings.toml, each pick had its own file in the state directory.
    fn apply_legacy(&mut self) {
        let dir = valkyrie_proto::state_dir();
        for (key, file) in [
            ("theme", "tui-theme"),
            ("tabs", "tui-tabs"),
            ("sound", "tui-sound"),
        ] {
            if let Ok(value) = std::fs::read_to_string(dir.join(file)) {
                self.set(key, &value);
            }
        }
    }

    /// The value shown for `field` in the panel.
    pub fn value(&self, field: Field) -> &'static str {
        match field {
            Field::Theme => self.theme.name,
            Field::SessionColors => {
                if self.themed_sessions {
                    "theme"
                } else {
                    "terminal"
                }
            }
            Field::Tabs => self.tabs.name(),
            Field::TabSide => self.tab_side.name(),
            Field::Sound => {
                if self.sound {
                    "on"
                } else {
                    "off"
                }
            }
        }
    }

    /// Steps `field` to its next value (`by` 1) or its previous (`by` -1).
    pub fn cycle(&mut self, field: Field, by: isize) {
        match field {
            Field::Theme => self.theme = *step(THEMES, self.theme.name, by, |t: &&Theme| t.name),
            Field::Tabs => self.tabs = *step(TabStyle::ALL, self.tabs.name(), by, |s| s.name()),
            Field::TabSide => {
                self.tab_side = *step(TabSide::ALL, self.tab_side.name(), by, |s| s.name())
            }
            Field::SessionColors => self.themed_sessions = !self.themed_sessions,
            Field::Sound => self.sound = !self.sound,
        }
    }
}

/// `settings.toml` under `$XDG_CONFIG_HOME/valkyrie`, else `~/.config/valkyrie`
/// (`%APPDATA%\Valkyrie` on Windows).
pub fn path() -> PathBuf {
    valkyrie_proto::config_dir().join("settings.toml")
}

fn names<'a>(all: impl Iterator<Item = &'a str>) -> String {
    all.collect::<Vec<_>>().join(" | ")
}

fn find<'a, T>(all: &'a [T], name: &str, name_of: impl Fn(&T) -> &str) -> Option<&'a T> {
    all.iter()
        .find(|x| name_of(x).eq_ignore_ascii_case(name.trim()))
}

fn step<'a, T>(all: &'a [T], current: &str, by: isize, name_of: impl Fn(&T) -> &str) -> &'a T {
    let i = all.iter().position(|x| name_of(x) == current).unwrap_or(0);
    &all[(i as isize + by).rem_euclid(all.len() as isize) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_settings_read_back() {
        let mut s = Settings::default();
        s.cycle(Field::Theme, 1);
        s.cycle(Field::Tabs, 1);
        s.cycle(Field::TabSide, -1);
        s.cycle(Field::Sound, 1);
        s.cycle(Field::SessionColors, 1);
        let mut back = Settings::default();
        back.apply(&s.to_toml());
        assert_eq!(back, s);
        assert_eq!(
            back.tab_side,
            TabSide::Right,
            "-1 from top wraps to the last"
        );
        assert!(!back.sound);
        assert!(!back.themed_sessions);
    }

    #[test]
    fn a_bad_line_costs_only_itself() {
        let mut s = Settings::default();
        s.apply("theme = \"nope\"\ntabs = cards  # a comment\nsound = maybe\nwhat = 1\n");
        assert_eq!(s.theme.name, DRACULA.name);
        assert_eq!(s.tabs, TabStyle::Cards);
        assert!(s.sound);
    }

    #[test]
    fn saving_one_field_keeps_the_rest_of_the_file() {
        let dir = std::env::temp_dir().join(format!("valkyrie-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("settings.toml");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, "theme = 'dracula'\ntabs = \"cards\"\n").unwrap();
        // Started with VALK_THEME=cyberpunk, then muted: only sound is saved.
        let mut running = Settings::kept(&file);
        running.set("theme", "cyberpunk");
        running.cycle(Field::Sound, 1);
        running.save_field(Field::Sound, &file);
        let kept = Settings::kept(&file);
        assert_eq!(kept.theme.name, "dracula");
        assert_eq!(kept.tabs, TabStyle::Cards);
        assert!(!kept.sound);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn every_value_cycles_back_to_itself() {
        let start = Settings::default();
        for &field in Field::ALL {
            let mut s = start;
            let n = match field {
                Field::Theme => THEMES.len(),
                Field::Tabs => TabStyle::ALL.len(),
                Field::TabSide => TabSide::ALL.len(),
                Field::SessionColors | Field::Sound => 2,
            };
            for _ in 0..n {
                s.cycle(field, 1);
            }
            assert_eq!(s.value(field), start.value(field), "{field:?}");
        }
    }
}

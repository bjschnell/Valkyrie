//! The keys that act straight past the program in a session, and on the home
//! screen: which key does what, as `.` on the home screen remaps it. Kept in
//! `settings.toml` as `keys.<action> = "<key>"`, only where it differs from the
//! default; `"none"` unbinds.
//!
//! A key here is Alt and a key (the terminal sends Esc then the key) or a control
//! key. Esc then `[` or `O` starts a CSI or SS3 sequence, so Alt-[ and Alt-O can't
//! be told from arrows and such; and Ctrl-C, Ctrl-D, Ctrl-Z, Ctrl-Q and Ctrl-S, and
//! the control keys that are Backspace, Tab, Enter and Esc, belong to the program.
//! Ctrl-] always goes home, whatever is mapped, so a map can't lock you out.

/// What a direct key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Tab mode: the next key splits, renames, … the attached tab.
    Tabs,
    Home,
    NewTab,
    /// The tab you were last on.
    Last,
    NextTab,
    PrevTab,
    /// The next session that needs you, then back where you started.
    Needing,
    /// The tab's menu, on Close.
    Close,
    /// The next pane of a split tab.
    Pane,
    /// The backlog page, straight into a new item, then back where you were.
    Capture,
    /// The tab at this place, from 0.
    Tab(u8),
}

impl Action {
    pub const ALL: [Action; 19] = [
        Action::Tabs,
        Action::Home,
        Action::NewTab,
        Action::Last,
        Action::NextTab,
        Action::PrevTab,
        Action::Needing,
        Action::Close,
        Action::Pane,
        Action::Capture,
        Action::Tab(0),
        Action::Tab(1),
        Action::Tab(2),
        Action::Tab(3),
        Action::Tab(4),
        Action::Tab(5),
        Action::Tab(6),
        Action::Tab(7),
        Action::Tab(8),
    ];

    fn index(self) -> usize {
        Action::ALL.iter().position(|&a| a == self).unwrap()
    }

    /// Its name in `settings.toml`.
    pub fn name(self) -> String {
        match self {
            Action::Tabs => "tab_mode".into(),
            Action::Home => "home".into(),
            Action::NewTab => "new_tab".into(),
            Action::Last => "last_tab".into(),
            Action::NextTab => "next_tab".into(),
            Action::PrevTab => "prev_tab".into(),
            Action::Needing => "needs_you".into(),
            Action::Close => "close_tab".into(),
            Action::Pane => "next_pane".into(),
            Action::Capture => "capture".into(),
            Action::Tab(i) => format!("tab_{}", i + 1),
        }
    }

    /// What it does, for the keys page.
    pub fn label(self) -> String {
        match self {
            Action::Tabs => "Actions".into(),
            Action::Home => "Home".into(),
            Action::NewTab => "New tab".into(),
            Action::Last => "Last tab".into(),
            Action::NextTab => "Next tab".into(),
            Action::PrevTab => "Previous tab".into(),
            Action::Needing => "Next needing you".into(),
            Action::Close => "Close tab".into(),
            Action::Pane => "Next pane".into(),
            Action::Capture => "Add to backlog".into(),
            Action::Tab(i) => format!("Tab {}", i + 1),
        }
    }

    fn default_key(self) -> Option<Key> {
        Some(match self {
            Action::Tabs => Key::Ctrl(0x1c),
            Action::Home => Key::Alt(b'h'),
            Action::NewTab => Key::Alt(b't'),
            Action::Last => Key::Alt(b'p'),
            Action::NextTab => Key::Alt(b'j'),
            Action::PrevTab => Key::Alt(b'k'),
            Action::Needing => Key::Alt(b'n'),
            Action::Close => Key::Alt(b'w'),
            Action::Pane => Key::Alt(b'o'),
            Action::Capture => Key::Alt(b'a'),
            Action::Tab(i) => Key::Alt(b'1' + i),
        })
    }
}

/// A key a direct action can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Alt and this printable character.
    Alt(u8),
    /// This control byte.
    Ctrl(u8),
}

/// Ctrl-] — home, always; no map takes it.
pub const HOME_KEY: u8 = 0x1d;

/// What a Mac's Option and a key type on a US layout when the terminal doesn't send
/// Option as Meta, and the key each stands for. Letters of other languages (`å`, `ç`,
/// `ø`, `œ`, `ß`) are left out; Option-e, -i, -n, -u and -` are dead keys that type
/// nothing alone.
const OPTION_CHARS: &[(char, u8)] = &[
    ('¡', b'1'),
    ('™', b'2'),
    ('£', b'3'),
    ('¢', b'4'),
    ('∞', b'5'),
    ('§', b'6'),
    ('¶', b'7'),
    ('•', b'8'),
    ('ª', b'9'),
    ('º', b'0'),
    ('∫', b'b'),
    ('∂', b'd'),
    ('ƒ', b'f'),
    ('©', b'g'),
    ('˙', b'h'),
    ('∆', b'j'),
    ('˚', b'k'),
    ('¬', b'l'),
    ('µ', b'm'),
    ('π', b'p'),
    ('®', b'r'),
    ('†', b't'),
    ('√', b'v'),
    ('∑', b'w'),
    ('≈', b'x'),
    ('¥', b'y'),
    ('Ω', b'z'),
];

/// The key a whole read stands for, if it is one character a Mac's Option types in
/// place of Alt and that key (see `OPTION_CHARS`).
pub fn option_typed(bytes: &[u8]) -> Option<u8> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut chars = text.chars();
    let (c, None) = (chars.next()?, chars.next()) else {
        return None;
    };
    OPTION_CHARS.iter().find(|(o, _)| *o == c).map(|(_, k)| *k)
}

/// Where to make Option send Alt, in the terminal `var` (the environment) names.
/// Over SSH only `TERM` comes along, and often iTerm2's `LC_TERMINAL`.
pub fn option_fix(var: impl Fn(&str) -> Option<String>) -> &'static str {
    let program = var("TERM_PROGRAM").unwrap_or_default();
    let term = var("TERM").unwrap_or_default();
    if program == "Apple_Terminal" {
        "Terminal: Settings → Profiles → Keyboard → Use Option as Meta key"
    } else if program == "iTerm.app" || var("LC_TERMINAL").as_deref() == Some("iTerm2") {
        "iTerm2: Settings → Profiles → Keys → Left Option key: Esc+"
    } else if program == "ghostty" || term == "xterm-ghostty" {
        "Ghostty: add macos-option-as-alt = left to its config"
    } else if term == "xterm-kitty" {
        "kitty: add macos_option_as_alt left to kitty.conf"
    } else if program == "WezTerm" {
        "WezTerm: set send_composed_key_when_left_alt_is_pressed = false"
    } else {
        "set your terminal to send Option as Meta (README: On a Mac)"
    }
}

/// Names for the characters `settings.toml` can't hold as they are.
const NAMED: &[(u8, &str)] = &[
    (b' ', "space"),
    (b'#', "hash"),
    (b'"', "quote"),
    (b'\'', "apostrophe"),
    (b'=', "equals"),
    (b'\\', "backslash"),
];

impl Key {
    /// Whether this key can be mapped (see the module's notes).
    pub fn bindable(self) -> bool {
        match self {
            Key::Alt(c) => (0x20..0x7f).contains(&c) && c != b'[' && c != b'O',
            Key::Ctrl(b) => match b {
                0x1c | 0x1e | 0x1f => true,
                // Ctrl-A to Ctrl-Z, less the program's own.
                0x01..=0x1a => !b"cdhijmqsz".contains(&(b + b'a' - 1)),
                _ => false,
            },
        }
    }

    /// The bytes the terminal sends for it.
    pub fn bytes(self) -> Vec<u8> {
        match self {
            Key::Alt(c) => vec![0x1b, c],
            Key::Ctrl(b) => vec![b],
        }
    }

    /// The key a whole read is, if it is one key that can be mapped.
    pub fn from_bytes(bytes: &[u8]) -> Option<Key> {
        let key = match bytes {
            [0x1b, c] => Key::Alt(*c),
            [b] if *b < 0x20 => Key::Ctrl(*b),
            _ => return None,
        };
        key.bindable().then_some(key)
    }

    /// Its name in `settings.toml`: `alt+h`, `alt+space`, `ctrl+t`, `ctrl+backslash`.
    pub fn name(self) -> String {
        let (mod_, c) = match self {
            Key::Alt(c) => ("alt", c),
            // Ctrl-\ is 0x1c, Ctrl-A 0x01: the character is the byte plus 0x40.
            Key::Ctrl(b) => ("ctrl", (b + 0x40).to_ascii_lowercase()),
        };
        match NAMED.iter().find(|(n, _)| *n == c) {
            Some((_, name)) => format!("{mod_}+{name}"),
            None => format!("{mod_}+{}", c as char),
        }
    }

    pub fn parse(name: &str) -> Option<Key> {
        let (mod_, rest) = name.trim().split_once('+')?;
        let c = match NAMED.iter().find(|(_, n)| *n == rest) {
            Some((c, _)) => *c,
            None => match rest.as_bytes() {
                [c] => *c,
                _ => return None,
            },
        };
        let key = match mod_.to_ascii_lowercase().as_str() {
            "alt" => Key::Alt(c),
            "ctrl" => Key::Ctrl(c.to_ascii_uppercase().checked_sub(0x40)?),
            _ => return None,
        };
        key.bindable().then_some(key)
    }

    /// How the bar and the keys page show it: `alt h`, `^\`.
    pub fn label(self) -> String {
        match self {
            Key::Alt(b' ') => "alt space".into(),
            Key::Alt(c) => format!("alt {}", c as char),
            Key::Ctrl(b) => format!("^{}", (b + 0x40) as char),
        }
    }
}

/// Which key each action has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keymap([Option<Key>; Action::ALL.len()]);

impl Default for Keymap {
    fn default() -> Self {
        Keymap(Action::ALL.map(Action::default_key))
    }
}

impl Keymap {
    pub fn get(&self, action: Action) -> Option<Key> {
        self.0[action.index()]
    }

    /// Gives `action` the key, and returns the action that had it before.
    pub fn bind(&mut self, action: Action, key: Option<Key>) -> Option<Action> {
        let had = key.and_then(|k| self.action(k)).filter(|&a| a != action);
        if let Some(had) = had {
            self.0[had.index()] = None;
        }
        self.0[action.index()] = key;
        had
    }

    pub fn reset(&mut self, action: Action) -> Option<Action> {
        self.bind(action, action.default_key())
    }

    pub fn is_default(&self, action: Action) -> bool {
        self.get(action) == action.default_key()
    }

    /// The action `key` does.
    pub fn action(&self, key: Key) -> Option<Action> {
        Action::ALL.into_iter().find(|&a| self.get(a) == Some(key))
    }

    /// The action at the start of `bytes`, and how many bytes its key takes.
    /// Ctrl-] is home whatever the map says.
    pub fn at(&self, bytes: &[u8]) -> Option<(Action, usize)> {
        let (key, len) = match bytes {
            [HOME_KEY, ..] => return Some((Action::Home, 1)),
            [0x1b, c, ..] => (Key::Alt(*c), 2),
            [b, ..] if *b < 0x20 => (Key::Ctrl(*b), 1),
            _ => return None,
        };
        Some((self.action(key)?, len))
    }

    /// `keys.<action> = "<key>"` lines, for the actions not on their defaults.
    pub fn to_toml(self) -> String {
        Action::ALL
            .into_iter()
            .filter(|&a| !self.is_default(a))
            .map(|a| {
                let key = self.get(a).map_or("none".into(), Key::name);
                format!("keys.{} = \"{key}\"\n", a.name())
            })
            .collect()
    }

    /// One `keys.<action>` setting; a name it doesn't know changes nothing.
    pub fn set(&mut self, action: &str, value: &str) {
        let Some(action) = Action::ALL.into_iter().find(|a| a.name() == action) else {
            return;
        };
        match value {
            "none" => {
                self.bind(action, None);
            }
            _ => {
                if let Some(key) = Key::parse(value) {
                    self.bind(action, Some(key));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bindable_key_names_and_parses_back() {
        for b in 0..0x80u8 {
            for key in [Key::Alt(b), Key::Ctrl(b)] {
                if key.bindable() {
                    assert_eq!(Key::parse(&key.name()), Some(key), "{}", key.name());
                    assert_eq!(Key::from_bytes(&key.bytes()), Some(key));
                }
            }
        }
        assert_eq!(Key::Ctrl(0x1c).name(), "ctrl+backslash");
        assert_eq!(Key::Ctrl(0x14).name(), "ctrl+t");
        assert_eq!(Key::Alt(b'#').name(), "alt+hash");
        assert_eq!(Key::Ctrl(0x1c).label(), "^\\");
    }

    /// What the program needs, and what can't be told from a sequence, can't be
    /// mapped; Ctrl-] is home under any map.
    #[test]
    fn some_keys_stay_the_programs() {
        for bytes in [
            &b"\x1b["[..],
            b"\x1bO",
            b"\x03",
            b"\x04",
            b"\r",
            b"\t",
            b"\x7f",
            b"\x1d",
        ] {
            assert_eq!(Key::from_bytes(bytes), None, "{bytes:?}");
        }
        let mut map = Keymap::default();
        map.bind(Action::Home, None);
        assert_eq!(map.at(&[HOME_KEY]), Some((Action::Home, 1)));
        assert_eq!(map.at(b"\x1bh"), None);
    }

    /// One Option character alone is a key; in text, or a letter of a language, it
    /// isn't.
    #[test]
    fn option_characters_stand_for_their_keys() {
        assert_eq!(option_typed("˙".as_bytes()), Some(b'h'));
        assert_eq!(option_typed("¡".as_bytes()), Some(b'1'));
        assert_eq!(option_typed("ª".as_bytes()), Some(b'9'));
        for text in ["π r", "ø", "ß", "h", "", "\x1bh"] {
            assert_eq!(option_typed(text.as_bytes()), None, "{text:?}");
        }
    }

    #[test]
    fn option_fix_names_the_terminal() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert!(option_fix(env(&[("TERM_PROGRAM", "Apple_Terminal")])).starts_with("Terminal:"));
        // Over SSH from iTerm2, only LC_TERMINAL says so.
        assert!(option_fix(env(&[("LC_TERMINAL", "iTerm2")])).starts_with("iTerm2:"));
        assert!(option_fix(env(&[("TERM", "xterm-ghostty")])).starts_with("Ghostty:"));
        assert!(option_fix(env(&[("TERM", "xterm-256color")])).contains("README"));
    }

    #[test]
    fn a_key_moves_to_its_new_action() {
        let mut map = Keymap::default();
        let had = map.bind(Action::Home, Some(Key::Alt(b'p')));
        assert_eq!(had, Some(Action::Last));
        assert_eq!(map.get(Action::Last), None);
        assert_eq!(map.at(b"\x1bp"), Some((Action::Home, 2)));
        assert_eq!(map.at(b"\x1bh"), None, "home's old key is free");
        map.reset(Action::Last);
        assert_eq!(map.get(Action::Last), Some(Key::Alt(b'p')));
        assert_eq!(map.get(Action::Home), None, "taken back by Last");
    }

    #[test]
    fn only_changes_are_written_and_read_back() {
        let mut map = Keymap::default();
        assert_eq!(map.to_toml(), "");
        map.bind(Action::NewTab, Some(Key::Ctrl(0x14)));
        map.bind(Action::Close, None);
        let text = map.to_toml();
        assert_eq!(
            text,
            "keys.new_tab = \"ctrl+t\"\nkeys.close_tab = \"none\"\n"
        );
        let mut back = Keymap::default();
        for line in text.lines() {
            let (k, v) = line.split_once(" = ").unwrap();
            back.set(k.strip_prefix("keys.").unwrap(), v.trim_matches('"'));
        }
        assert_eq!(back, map);
    }
}

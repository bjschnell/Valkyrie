//! Color themes for Valkyrie's own chrome (home screen, bars) and, unless turned
//! off, for sessions: a program's default colors and its 16 ANSI colors come from
//! the theme. Colors it names exactly (256-color or RGB) are drawn as they are.

use ratatui::style::Color;
use valkyrie_proto::AgentState;

pub struct Theme {
    pub name: &'static str,
    /// Page background, and the slightly lifted panel and selection backgrounds.
    pub bg: Color,
    pub panel: Color,
    /// Behind the tab strip, darker than anything on it.
    pub strip: Color,
    /// A clickable card on the strip (tab, home, new), a step lighter than `bg`.
    pub card: Color,
    pub selection: Color,
    pub border: Color,
    pub fg: Color,
    pub muted: Color,
    /// Brand color: the logo, focused borders, the selection marker, key chips.
    pub accent: Color,
    /// Secondary highlight: names, status messages.
    pub accent2: Color,
    pub needs: Color,
    pub blocked: Color,
    pub done: Color,
    pub working: Color,
    pub interrupted: Color,
    pub idle: Color,
    /// Digital rain in these colors falls through the home screen's empty rows,
    /// and the header glitches: the effects of matrix, violet, cyberpunk, valkyrie and netrunner.
    pub rain: Option<Rain>,
    /// The 16 ANSI colors a session's programs draw with: black, red, green,
    /// yellow, blue, magenta, cyan, white, then their bright versions.
    pub ansi: [Color; 16],
}

/// Digital rain: what it's made of, then a drop's head color and its trail's,
/// brightest first.
pub struct Rain {
    pub glyphs: &'static str,
    pub head: Color,
    pub trail: [Color; 3],
}

/// Code rain: one column wide in every coding font.
const CODE: &str = "0123456789ABCDEFXZ$#@%&*+=<>:;|";
/// Breach protocol's codes.
const HEX: &str = "0123456789ABCDEF";
/// The Elder Futhark, falling. One column wide too, from the font's own runes or
/// the terminal's fallback.
const RUNES: &str = "ᚠᚢᚦᚨᚱᚲᚷᚹᚺᚾᛁᛃᛇᛈᛉᛊᛏᛒᛖᛗᛚᛜᛞᛟ";

const fn hex(rgb: u32) -> Color {
    Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// The official Dracula palette (draculatheme.com).
pub const DRACULA: Theme = Theme {
    name: "dracula",
    bg: hex(0x282a36),
    panel: hex(0x21222c),
    strip: hex(0x191a21),
    card: hex(0x343746),
    selection: hex(0x44475a),
    border: hex(0x44475a),
    fg: hex(0xf8f8f2),
    muted: hex(0x6272a4),
    accent: hex(0xbd93f9),
    accent2: hex(0xff79c6),
    needs: hex(0xf1fa8c),
    blocked: hex(0xff5555),
    done: hex(0x50fa7b),
    working: hex(0x8be9fd),
    interrupted: hex(0xffb86c),
    idle: hex(0x6272a4),
    rain: None,
    ansi: [
        hex(0x21222c),
        hex(0xff5555),
        hex(0x50fa7b),
        hex(0xf1fa8c),
        hex(0xbd93f9),
        hex(0xff79c6),
        hex(0x8be9fd),
        hex(0xf8f8f2),
        hex(0x6272a4),
        hex(0xff6e6e),
        hex(0x69ff94),
        hex(0xffffa5),
        hex(0xd6acff),
        hex(0xff92df),
        hex(0xa4ffff),
        hex(0xffffff),
    ],
};

/// Neon on deep navy, with runes falling through the home screen.
pub const CYBERPUNK: Theme = Theme {
    name: "cyberpunk",
    bg: hex(0x0b0c1a),
    panel: hex(0x11132a),
    strip: hex(0x05060d),
    card: hex(0x1b1e3d),
    selection: hex(0x2b1748),
    border: hex(0x3a2a66),
    fg: hex(0xe6e9ff),
    muted: hex(0x6a6f9e),
    accent: hex(0xff2a6d),
    accent2: hex(0x05d9e8),
    needs: hex(0xf9f002),
    blocked: hex(0xff003c),
    done: hex(0x00ff9f),
    working: hex(0x05d9e8),
    interrupted: hex(0xff9e00),
    idle: hex(0x5a5f8a),
    rain: Some(Rain {
        glyphs: RUNES,
        head: hex(0xe6fdff),
        trail: [hex(0x05d9e8), hex(0xff2a6d), hex(0x5a1a4a)],
    }),
    ansi: [
        hex(0x1b1e3d),
        hex(0xff003c),
        hex(0x00ff9f),
        hex(0xf9f002),
        hex(0x4f6bff),
        hex(0xff2a6d),
        hex(0x05d9e8),
        hex(0xd0d4f5),
        hex(0x6a6f9e),
        hex(0xff4d6d),
        hex(0x5affc0),
        hex(0xfff76b),
        hex(0x7f9cff),
        hex(0xff6ea0),
        hex(0x6af0f8),
        hex(0xffffff),
    ],
};

/// Pure black with pastel neon: mint, hot pink, lavender.
pub const BLACKOUT: Theme = Theme {
    name: "blackout",
    bg: hex(0x000000),
    panel: hex(0x0a0a0c),
    strip: hex(0x000000),
    card: hex(0x141418),
    selection: hex(0x221a2c),
    border: hex(0x2c2833),
    fg: hex(0xf0eef5),
    muted: hex(0x6f6a7d),
    accent: hex(0xc9a7ff),
    accent2: hex(0xff8fd0),
    needs: hex(0xfff3a0),
    blocked: hex(0xff8a9e),
    done: hex(0x8dffbf),
    working: hex(0x9ae6ff),
    interrupted: hex(0xffc39e),
    idle: hex(0x5a5666),
    rain: None,
    ansi: [
        hex(0x1a1a1f),
        hex(0xff8a9e),
        hex(0x8dffbf),
        hex(0xfff3a0),
        hex(0xa0b8ff),
        hex(0xff8fd0),
        hex(0x9ae6ff),
        hex(0xe6e3ee),
        hex(0x6f6a7d),
        hex(0xffadbb),
        hex(0xb5ffd6),
        hex(0xfff8c4),
        hex(0xc9a7ff),
        hex(0xffb3e0),
        hex(0xc2f1ff),
        hex(0xffffff),
    ],
};

/// Catppuccin Mocha (catppuccin.com).
pub const CATPPUCCIN: Theme = Theme {
    name: "catppuccin",
    bg: hex(0x1e1e2e),
    panel: hex(0x181825),
    strip: hex(0x11111b),
    card: hex(0x313244),
    selection: hex(0x45475a),
    border: hex(0x45475a),
    fg: hex(0xcdd6f4),
    muted: hex(0x7f849c),
    accent: hex(0xcba6f7),
    accent2: hex(0xf5c2e7),
    needs: hex(0xf9e2af),
    blocked: hex(0xf38ba8),
    done: hex(0xa6e3a1),
    working: hex(0x89dceb),
    interrupted: hex(0xfab387),
    idle: hex(0x6c7086),
    rain: None,
    ansi: [
        hex(0x45475a),
        hex(0xf38ba8),
        hex(0xa6e3a1),
        hex(0xf9e2af),
        hex(0x89b4fa),
        hex(0xf5c2e7),
        hex(0x94e2d5),
        hex(0xbac2de),
        hex(0x585b70),
        hex(0xf38ba8),
        hex(0xa6e3a1),
        hex(0xf9e2af),
        hex(0x89b4fa),
        hex(0xf5c2e7),
        hex(0x94e2d5),
        hex(0xa6adc8),
    ],
};

/// Nord (nordtheme.com).
pub const NORD: Theme = Theme {
    name: "nord",
    bg: hex(0x2e3440),
    panel: hex(0x292e39),
    strip: hex(0x242933),
    card: hex(0x3b4252),
    selection: hex(0x434c5e),
    border: hex(0x4c566a),
    fg: hex(0xeceff4),
    muted: hex(0x7b88a1),
    accent: hex(0x88c0d0),
    accent2: hex(0xb48ead),
    needs: hex(0xebcb8b),
    blocked: hex(0xbf616a),
    done: hex(0xa3be8c),
    working: hex(0x81a1c1),
    interrupted: hex(0xd08770),
    idle: hex(0x616e88),
    rain: None,
    ansi: [
        hex(0x3b4252),
        hex(0xbf616a),
        hex(0xa3be8c),
        hex(0xebcb8b),
        hex(0x81a1c1),
        hex(0xb48ead),
        hex(0x88c0d0),
        hex(0xe5e9f0),
        hex(0x4c566a),
        hex(0xbf616a),
        hex(0xa3be8c),
        hex(0xebcb8b),
        hex(0x81a1c1),
        hex(0xb48ead),
        hex(0x8fbcbb),
        hex(0xeceff4),
    ],
};

/// Gruvbox dark (github.com/morhetz/gruvbox).
pub const GRUVBOX: Theme = Theme {
    name: "gruvbox",
    bg: hex(0x282828),
    panel: hex(0x1d2021),
    strip: hex(0x141617),
    card: hex(0x32302f),
    selection: hex(0x3c3836),
    border: hex(0x504945),
    fg: hex(0xebdbb2),
    muted: hex(0x928374),
    accent: hex(0xfe8019),
    accent2: hex(0xd3869b),
    needs: hex(0xfabd2f),
    blocked: hex(0xfb4934),
    done: hex(0xb8bb26),
    working: hex(0x83a598),
    interrupted: hex(0x8ec07c),
    idle: hex(0x7c6f64),
    rain: None,
    ansi: [
        hex(0x282828),
        hex(0xcc241d),
        hex(0x98971a),
        hex(0xd79921),
        hex(0x458588),
        hex(0xb16286),
        hex(0x689d6a),
        hex(0xa89984),
        hex(0x928374),
        hex(0xfb4934),
        hex(0xb8bb26),
        hex(0xfabd2f),
        hex(0x83a598),
        hex(0xd3869b),
        hex(0x8ec07c),
        hex(0xebdbb2),
    ],
};

/// Tokyo Night (github.com/folke/tokyonight.nvim).
pub const TOKYONIGHT: Theme = Theme {
    name: "tokyonight",
    bg: hex(0x1a1b26),
    panel: hex(0x16161e),
    strip: hex(0x101014),
    card: hex(0x24283b),
    selection: hex(0x283457),
    border: hex(0x3b4261),
    fg: hex(0xc0caf5),
    muted: hex(0x737aa2),
    accent: hex(0x7aa2f7),
    accent2: hex(0xbb9af7),
    needs: hex(0xe0af68),
    blocked: hex(0xf7768e),
    done: hex(0x9ece6a),
    working: hex(0x7dcfff),
    interrupted: hex(0xff9e64),
    idle: hex(0x565f89),
    rain: None,
    ansi: [
        hex(0x15161e),
        hex(0xf7768e),
        hex(0x9ece6a),
        hex(0xe0af68),
        hex(0x7aa2f7),
        hex(0xbb9af7),
        hex(0x7dcfff),
        hex(0xa9b1d6),
        hex(0x414868),
        hex(0xf7768e),
        hex(0x9ece6a),
        hex(0xe0af68),
        hex(0x7aa2f7),
        hex(0xbb9af7),
        hex(0x7dcfff),
        hex(0xc0caf5),
    ],
};

/// Phosphor green on black, with digital rain on the home screen.
pub const MATRIX: Theme = Theme {
    name: "matrix",
    bg: hex(0x000000),
    panel: hex(0x020a04),
    strip: hex(0x000000),
    card: hex(0x08190d),
    selection: hex(0x0c3318),
    border: hex(0x0f4d22),
    fg: hex(0xb4ffc4),
    muted: hex(0x2f8f4c),
    accent: hex(0x00ff41),
    accent2: hex(0x7dffa0),
    needs: hex(0xd8ff3e),
    blocked: hex(0xff3b3b),
    done: hex(0x00ff41),
    working: hex(0x3dffd2),
    interrupted: hex(0xffb000),
    idle: hex(0x1f6e36),
    rain: Some(Rain {
        glyphs: CODE,
        head: hex(0xe8ffec),
        trail: [hex(0x00ff41), hex(0x00b02c), hex(0x005c18)],
    }),
    ansi: [
        hex(0x08190d),
        hex(0xff3b3b),
        hex(0x00ff41),
        hex(0xd8ff3e),
        hex(0x00b386),
        hex(0x7dffa0),
        hex(0x3dffd2),
        hex(0xb4ffc4),
        hex(0x2f8f4c),
        hex(0xff6b6b),
        hex(0x5cff7f),
        hex(0xe8ff80),
        hex(0x33d9a8),
        hex(0xa8ffbf),
        hex(0x8affe6),
        hex(0xe8ffec),
    ],
};

/// Pale lavender on near-black violet, with lavender digital rain.
pub const VIOLET: Theme = Theme {
    name: "violet",
    bg: hex(0x07050c),
    panel: hex(0x0c0914),
    strip: hex(0x040308),
    card: hex(0x161127),
    selection: hex(0x261d45),
    border: hex(0x3b2e68),
    fg: hex(0xebe4ff),
    muted: hex(0x8a7cb8),
    accent: hex(0xc8b4ff),
    accent2: hex(0xe6ccff),
    needs: hex(0xffe8a6),
    blocked: hex(0xff7d9c),
    done: hex(0xa9f0d1),
    working: hex(0xa3d4ff),
    interrupted: hex(0xffc59c),
    idle: hex(0x5f5388),
    rain: Some(Rain {
        glyphs: CODE,
        head: hex(0xf6f2ff),
        trail: [hex(0xc8b4ff), hex(0x8b70d8), hex(0x463a7d)],
    }),
    ansi: [
        hex(0x161127),
        hex(0xff7d9c),
        hex(0xa9f0d1),
        hex(0xffe8a6),
        hex(0xa3b4ff),
        hex(0xc8b4ff),
        hex(0xa3d4ff),
        hex(0xdcd3f5),
        hex(0x8a7cb8),
        hex(0xff9fb6),
        hex(0xc4f7e1),
        hex(0xfff0c4),
        hex(0xc0ccff),
        hex(0xdccfff),
        hex(0xc4e4ff),
        hex(0xf6f2ff),
    ],
};

/// The app icon's colors: violet and magenta neon on near-black, the brow rune's
/// green, and runes falling with green heads.
pub const VALKYRIE: Theme = Theme {
    name: "valkyrie",
    bg: hex(0x0b0a14),
    panel: hex(0x100e1c),
    strip: hex(0x06050c),
    card: hex(0x1a1a24),
    selection: hex(0x2a1858),
    border: hex(0x3d2a7a),
    fg: hex(0xece6ff),
    muted: hex(0x8577b3),
    accent: hex(0x9d6bff),
    accent2: hex(0xff3ed8),
    needs: hex(0xffe36e),
    blocked: hex(0xff4d6d),
    done: hex(0x39ff7a),
    working: hex(0xc77dff),
    interrupted: hex(0xff9e5e),
    idle: hex(0x5a4f86),
    rain: Some(Rain {
        glyphs: RUNES,
        head: hex(0x8dffb0),
        trail: [hex(0xff3ed8), hex(0x6b3aff), hex(0x33206e)],
    }),
    ansi: [
        hex(0x1a1a24),
        hex(0xff4d6d),
        hex(0x39ff7a),
        hex(0xffe36e),
        hex(0x8b6bff),
        hex(0xff3ed8),
        hex(0x5ee7ff),
        hex(0xd9d2f0),
        hex(0x4a4566),
        hex(0xff7a92),
        hex(0x7dffa6),
        hex(0xfff09e),
        hex(0xa98cff),
        hex(0xff7ae6),
        hex(0x9cf0ff),
        hex(0xffffff),
    ],
};

/// An 80s hacker's heads-up display: pure black, neon cyan for what you can act on,
/// neon pink for the chrome, yellow and red for what needs you. Working and idle
/// agents stay dim so only what needs you lights up. Hex codes fall, cyan to pink.
pub const NETRUNNER: Theme = Theme {
    name: "netrunner",
    bg: hex(0x000000),
    panel: hex(0x07070b),
    strip: hex(0x000000),
    card: hex(0x0d0d14),
    selection: hex(0x1f0a2e),
    border: hex(0x2a2a3d),
    fg: hex(0xf4f4fa),
    muted: hex(0x6e6e88),
    accent: hex(0x00f0ff),
    accent2: hex(0xff2bd6),
    needs: hex(0xfff700),
    blocked: hex(0xff1e3c),
    done: hex(0x39ff14),
    working: hex(0x8a5cff),
    interrupted: hex(0xff8a00),
    idle: hex(0x45455a),
    rain: Some(Rain {
        glyphs: HEX,
        head: hex(0xc8fdff),
        trail: [hex(0x00f0ff), hex(0xff2bd6), hex(0x5e0c52)],
    }),
    ansi: [
        hex(0x0d0d14),
        hex(0xff1e3c),
        hex(0x39ff14),
        hex(0xfff700),
        hex(0x3d5cff),
        hex(0xff2bd6),
        hex(0x00f0ff),
        hex(0xd4d4e0),
        hex(0x6e6e88),
        hex(0xff5c75),
        hex(0x7aff5c),
        hex(0xfffa7a),
        hex(0x7a8cff),
        hex(0xff7ae6),
        hex(0x7af7ff),
        hex(0xffffff),
    ],
};

pub const THEMES: &[&Theme] = &[
    &DRACULA,
    &CYBERPUNK,
    &BLACKOUT,
    &CATPPUCCIN,
    &NORD,
    &GRUVBOX,
    &TOKYONIGHT,
    &MATRIX,
    &VIOLET,
    &VALKYRIE,
    &NETRUNNER,
];

impl Theme {
    pub fn state(&self, state: AgentState) -> Color {
        match state {
            AgentState::NeedsInput => self.needs,
            AgentState::Blocked => self.blocked,
            AgentState::ReviewReady => self.done,
            AgentState::Interrupted | AgentState::Stale => self.interrupted,
            AgentState::Working => self.working,
            AgentState::Idle | AgentState::Exited => self.idle,
        }
    }

    pub fn by_name(name: &str) -> Option<&'static Theme> {
        THEMES
            .iter()
            .copied()
            .find(|t| t.name.eq_ignore_ascii_case(name.trim()))
    }
}

/// The glyph in front of a state: a spinner frame while working. All are in common
/// coding fonts (JetBrains Mono lacks ✔ and ✖, for one).
pub fn state_icon(state: AgentState, frame: usize) -> &'static str {
    const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    match state {
        AgentState::NeedsInput => "●",
        AgentState::Blocked => "×",
        AgentState::ReviewReady => "✓",
        AgentState::Interrupted => "◆",
        AgentState::Stale => "◇",
        AgentState::Working => SPIN[frame % SPIN.len()],
        AgentState::Idle => "○",
        AgentState::Exited => "·",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for theme in THEMES {
            assert_eq!(Theme::by_name(theme.name).unwrap().name, theme.name);
        }
        assert_eq!(Theme::by_name(" Cyberpunk\n").unwrap().name, "cyberpunk");
        assert!(Theme::by_name("nope").is_none());
    }

    #[test]
    fn rain_glyphs_are_one_column_wide() {
        for glyphs in [CODE, RUNES] {
            for c in glyphs.chars() {
                let span = ratatui::text::Span::raw(c.to_string());
                assert_eq!(span.width(), 1, "{c}");
            }
        }
    }

    #[test]
    fn icons_are_one_column_wide() {
        for frame in 0..10 {
            for state in [
                AgentState::NeedsInput,
                AgentState::Blocked,
                AgentState::ReviewReady,
                AgentState::Interrupted,
                AgentState::Stale,
                AgentState::Working,
                AgentState::Idle,
                AgentState::Exited,
            ] {
                let span = ratatui::text::Span::raw(state_icon(state, frame));
                assert_eq!(span.width(), 1, "{state:?}");
            }
        }
    }
}

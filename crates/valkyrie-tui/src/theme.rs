//! Color themes for Valkyrie's own chrome (home screen, bars) and, unless turned
//! off, for sessions: a program's default colors and its 16 ANSI colors come from
//! the theme. Colors it names exactly (256-color or RGB) are drawn as they are.

use ratatui::style::Color;
use std::sync::OnceLock;
use valkyrie_proto::themes::{self, Pack};
use valkyrie_proto::{AgentState, TermColors};

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
    /// The faint text level: kinds, priorities, repos, ids, ages.
    pub muted: Color,
    // A theme is a skin: these nine tokens (spec: Marks and color) are all that
    // carry meaning. `needs`, `blocked`, `done` and `working` are the ask, stuck,
    // finished and working states; `accent` is decisions, selection, keys in hints
    // and the active section; `fg`, `secondary()`, `dim()` and `muted` (faint) are
    // the four text levels. Nothing else changes with state.
    /// Brand color: the logo, the selection marker, keys in hints, decisions.
    pub accent: Color,
    pub needs: Color,
    pub blocked: Color,
    pub done: Color,
    pub working: Color,
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
    needs: hex(0xf1fa8c),
    blocked: hex(0xff5555),
    done: hex(0x50fa7b),
    working: hex(0x8be9fd),
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
    needs: hex(0xf9f002),
    blocked: hex(0xff003c),
    done: hex(0x00ff9f),
    working: hex(0x05d9e8),
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
    needs: hex(0xfff3a0),
    blocked: hex(0xff8a9e),
    done: hex(0x8dffbf),
    working: hex(0x9ae6ff),
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
    needs: hex(0xf9e2af),
    blocked: hex(0xf38ba8),
    done: hex(0xa6e3a1),
    working: hex(0x89dceb),
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
    needs: hex(0xebcb8b),
    blocked: hex(0xbf616a),
    done: hex(0xa3be8c),
    working: hex(0x81a1c1),
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
    needs: hex(0xfabd2f),
    blocked: hex(0xfb4934),
    done: hex(0xb8bb26),
    working: hex(0x83a598),
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
    needs: hex(0xe0af68),
    blocked: hex(0xf7768e),
    done: hex(0x9ece6a),
    working: hex(0x7dcfff),
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
    needs: hex(0xd8ff3e),
    blocked: hex(0xff3b3b),
    done: hex(0x00ff41),
    working: hex(0x3dffd2),
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
    needs: hex(0xffe8a6),
    blocked: hex(0xff7d9c),
    done: hex(0xa9f0d1),
    working: hex(0xa3d4ff),
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
    needs: hex(0xffe36e),
    blocked: hex(0xff4d6d),
    done: hex(0x39ff7a),
    working: hex(0xc77dff),
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
    needs: hex(0xfff700),
    blocked: hex(0xff1e3c),
    done: hex(0x39ff14),
    working: hex(0x8a5cff),
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

/// Solarized Dark (ethanschoonover.com/solarized).
pub const SOLARIZED: Theme = Theme {
    name: "solarized",
    bg: hex(0x002b36),
    panel: hex(0x00252f),
    strip: hex(0x001e26),
    card: hex(0x073642),
    selection: hex(0x0b4552),
    border: hex(0x18505e),
    fg: hex(0x93a1a1),
    muted: hex(0x657b83),
    accent: hex(0x268bd2),
    needs: hex(0xb58900),
    blocked: hex(0xdc322f),
    done: hex(0x859900),
    working: hex(0x2aa198),
    rain: None,
    ansi: [
        hex(0x073642),
        hex(0xdc322f),
        hex(0x859900),
        hex(0xb58900),
        hex(0x268bd2),
        hex(0xd33682),
        hex(0x2aa198),
        hex(0xeee8d5),
        hex(0x002b36),
        hex(0xcb4b16),
        hex(0x586e75),
        hex(0x657b83),
        hex(0x839496),
        hex(0x6c71c4),
        hex(0x93a1a1),
        hex(0xfdf6e3),
    ],
};

/// One Dark (Atom).
pub const ONEDARK: Theme = Theme {
    name: "onedark",
    bg: hex(0x282c34),
    panel: hex(0x21252b),
    strip: hex(0x181a1f),
    card: hex(0x2c313a),
    selection: hex(0x3e4451),
    border: hex(0x4b5263),
    fg: hex(0xabb2bf),
    muted: hex(0x5c6370),
    accent: hex(0x61afef),
    needs: hex(0xe5c07b),
    blocked: hex(0xe06c75),
    done: hex(0x98c379),
    working: hex(0x56b6c2),
    rain: None,
    ansi: [
        hex(0x282c34),
        hex(0xe06c75),
        hex(0x98c379),
        hex(0xe5c07b),
        hex(0x61afef),
        hex(0xc678dd),
        hex(0x56b6c2),
        hex(0xabb2bf),
        hex(0x5c6370),
        hex(0xe06c75),
        hex(0x98c379),
        hex(0xe5c07b),
        hex(0x61afef),
        hex(0xc678dd),
        hex(0x56b6c2),
        hex(0xffffff),
    ],
};

/// Rosé Pine (rosepinetheme.com).
pub const ROSEPINE: Theme = Theme {
    name: "rosepine",
    bg: hex(0x191724),
    panel: hex(0x1f1d2e),
    strip: hex(0x12101c),
    card: hex(0x26233a),
    selection: hex(0x403d52),
    border: hex(0x524f67),
    fg: hex(0xe0def4),
    muted: hex(0x6e6a86),
    accent: hex(0xc4a7e7),
    needs: hex(0xf6c177),
    blocked: hex(0xeb6f92),
    done: hex(0x9ccfd8),
    working: hex(0x3e8fb0),
    rain: None,
    ansi: [
        hex(0x26233a),
        hex(0xeb6f92),
        hex(0x31748f),
        hex(0xf6c177),
        hex(0x9ccfd8),
        hex(0xc4a7e7),
        hex(0xebbcba),
        hex(0xe0def4),
        hex(0x6e6a86),
        hex(0xeb6f92),
        hex(0x31748f),
        hex(0xf6c177),
        hex(0x9ccfd8),
        hex(0xc4a7e7),
        hex(0xebbcba),
        hex(0xe0def4),
    ],
};

/// Monokai.
pub const MONOKAI: Theme = Theme {
    name: "monokai",
    bg: hex(0x272822),
    panel: hex(0x1e1f1c),
    strip: hex(0x171814),
    card: hex(0x3e3d32),
    selection: hex(0x49483e),
    border: hex(0x5b5a4d),
    fg: hex(0xf8f8f2),
    muted: hex(0x75715e),
    accent: hex(0x66d9ef),
    needs: hex(0xe6db74),
    blocked: hex(0xf92672),
    done: hex(0xa6e22e),
    working: hex(0xae81ff),
    rain: None,
    ansi: [
        hex(0x272822),
        hex(0xf92672),
        hex(0xa6e22e),
        hex(0xf4bf75),
        hex(0x66d9ef),
        hex(0xae81ff),
        hex(0xa1efe4),
        hex(0xf8f8f2),
        hex(0x75715e),
        hex(0xf92672),
        hex(0xa6e22e),
        hex(0xf4bf75),
        hex(0x66d9ef),
        hex(0xae81ff),
        hex(0xa1efe4),
        hex(0xf9f8f5),
    ],
};

/// Solarized Light, the first light theme.
pub const SOLARIZEDLIGHT: Theme = Theme {
    name: "solarizedlight",
    bg: hex(0xfdf6e3),
    panel: hex(0xf7f0dc),
    strip: hex(0xeee8d5),
    card: hex(0xf5eedb),
    selection: hex(0xe4ddc6),
    border: hex(0xd3cbb7),
    fg: hex(0x586e75),
    muted: hex(0x93a1a1),
    accent: hex(0x268bd2),
    needs: hex(0xb58900),
    blocked: hex(0xdc322f),
    done: hex(0x859900),
    working: hex(0x2aa198),
    rain: None,
    ansi: [
        hex(0x073642),
        hex(0xdc322f),
        hex(0x859900),
        hex(0xb58900),
        hex(0x268bd2),
        hex(0xd33682),
        hex(0x2aa198),
        hex(0xeee8d5),
        hex(0x002b36),
        hex(0xcb4b16),
        hex(0x586e75),
        hex(0x657b83),
        hex(0x839496),
        hex(0x6c71c4),
        hex(0x93a1a1),
        hex(0xfdf6e3),
    ],
};

/// Catppuccin Latte (catppuccin.com), light.
pub const CATPPUCCINLATTE: Theme = Theme {
    name: "catppuccinlatte",
    bg: hex(0xeff1f5),
    panel: hex(0xe6e9ef),
    strip: hex(0xccd0da),
    card: hex(0xdce0e8),
    selection: hex(0xbcc0cc),
    border: hex(0xacb0be),
    fg: hex(0x4c4f69),
    muted: hex(0x7c7f93),
    accent: hex(0x8839ef),
    needs: hex(0xdf8e1d),
    blocked: hex(0xd20f39),
    done: hex(0x40a02b),
    working: hex(0x04a5e5),
    rain: None,
    ansi: [
        hex(0x5c5f77),
        hex(0xd20f39),
        hex(0x40a02b),
        hex(0xdf8e1d),
        hex(0x1e66f5),
        hex(0xea76cb),
        hex(0x179299),
        hex(0xacb0be),
        hex(0x6c6f85),
        hex(0xd20f39),
        hex(0x40a02b),
        hex(0xdf8e1d),
        hex(0x1e66f5),
        hex(0xea76cb),
        hex(0x179299),
        hex(0xbcc0cc),
    ],
};

/// Gruvbox light (github.com/morhetz/gruvbox).
pub const GRUVBOXLIGHT: Theme = Theme {
    name: "gruvboxlight",
    bg: hex(0xfbf1c7),
    panel: hex(0xf9f5d7),
    strip: hex(0xebdbb2),
    card: hex(0xf2e5bc),
    selection: hex(0xd5c4a1),
    border: hex(0xbdae93),
    fg: hex(0x3c3836),
    muted: hex(0x7c6f64),
    accent: hex(0xaf3a03),
    needs: hex(0xb57614),
    blocked: hex(0x9d0006),
    done: hex(0x79740e),
    working: hex(0x076678),
    rain: None,
    ansi: [
        hex(0xfbf1c7),
        hex(0xcc241d),
        hex(0x98971a),
        hex(0xd79921),
        hex(0x458588),
        hex(0xb16286),
        hex(0x689d6a),
        hex(0x7c6f64),
        hex(0x928374),
        hex(0x9d0006),
        hex(0x79740e),
        hex(0xb57614),
        hex(0x076678),
        hex(0x8f3f71),
        hex(0x427b58),
        hex(0x3c3836),
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
    &SOLARIZED,
    &ONEDARK,
    &ROSEPINE,
    &MONOKAI,
    &SOLARIZEDLIGHT,
    &CATPPUCCINLATTE,
    &GRUVBOXLIGHT,
];

impl Theme {
    pub fn state(&self, state: AgentState) -> Color {
        match state {
            AgentState::NeedsInput => self.needs,
            AgentState::Blocked => self.blocked,
            AgentState::ReviewReady => self.done,
            AgentState::Interrupted | AgentState::Stale => self.blocked,
            AgentState::Working => self.working,
            AgentState::Idle | AgentState::Exited => self.muted,
        }
    }

    /// The secondary text level: a row's one line, a body.
    pub fn secondary(&self) -> Color {
        mix(self.fg, self.muted, 0.3)
    }

    /// The muted text level, between secondary and faint: group labels, hint
    /// labels.
    pub fn dim(&self) -> Color {
        mix(self.fg, self.muted, 0.6)
    }

    /// `line` for the selected row: its faint words lifted to the muted level, so
    /// they stay readable on the selection background in the light themes.
    pub fn lift_faint(&self, line: ratatui::text::Line<'static>) -> ratatui::text::Line<'static> {
        let dim = self.dim();
        let spans = line
            .spans
            .into_iter()
            .map(|mut s| {
                if s.style.fg == Some(self.muted) {
                    s.style.fg = Some(dim);
                }
                s
            })
            .collect::<Vec<_>>();
        ratatui::text::Line { spans, ..line }
    }

    /// The accent at 70%: the marker of items agents filed.
    pub fn accent_dim(&self) -> Color {
        mix(self.accent, self.bg, 0.3)
    }

    /// What a session's programs are told their terminal looks like.
    pub fn term_colors(&self) -> TermColors {
        let rgb = |c: Color| match c {
            Color::Rgb(r, g, b) => [r, g, b],
            _ => [0, 0, 0],
        };
        TermColors {
            fg: rgb(self.fg),
            bg: rgb(self.bg),
            ansi: self.ansi.iter().map(|&c| rgb(c)).collect(),
        }
    }

    pub fn by_name(name: &str) -> Option<&'static Theme> {
        all()
            .iter()
            .copied()
            .find(|t| t.name.eq_ignore_ascii_case(name.trim()))
    }

    /// A theme from a pack's theme.toml (see `valkyrie_proto::themes`). It needs
    /// `bg`, `fg` and `accent`; the rest default from those, and `rain.glyphs`
    /// gives it rain. `None` when a required color is missing or isn't `#rrggbb`.
    pub fn from_pack(pack: &Pack) -> Option<Theme> {
        let color = |key: &str| {
            pack.get(key)
                .and_then(themes::rgb)
                .map(|[r, g, b]| Color::Rgb(r, g, b))
        };
        let bg = color("bg")?;
        let fg = color("fg")?;
        let accent = color("accent")?;
        let or = |key: &str, default: Color| color(key).unwrap_or(default);
        let muted = or("muted", mix(fg, bg, 0.55));
        let border = or("border", mix(bg, fg, 0.2));
        let needs = or("needs", DRACULA.needs);
        let blocked = or("blocked", DRACULA.blocked);
        let done = or("done", DRACULA.done);
        let working = or("working", accent);
        let base = [
            or("panel", mix(bg, fg, 0.04)),
            blocked,
            done,
            needs,
            accent,
            accent,
            working,
            mix(fg, bg, 0.1),
        ];
        let mut ansi = [Color::Reset; 16];
        let given: Vec<Color> = pack
            .get("ansi")
            .map(themes::list)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| themes::rgb(c).map(|[r, g, b]| Color::Rgb(r, g, b)))
            .collect();
        for i in 0..16 {
            ansi[i] = given.get(i).copied().unwrap_or(if i < 8 {
                base[i]
            } else if i == 8 {
                muted
            } else if i == 15 {
                fg
            } else {
                mix(base[i - 8], Color::Rgb(255, 255, 255), 0.25)
            });
        }
        let rain = pack.get("rain.glyphs").map(|glyphs| {
            let trail: Vec<Color> = pack
                .get("rain.trail")
                .map(themes::list)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|c| themes::rgb(c).map(|[r, g, b]| Color::Rgb(r, g, b)))
                .collect();
            let first = trail.first().copied().unwrap_or(accent);
            Rain {
                glyphs: leak(rain_glyphs(glyphs)),
                head: or("rain.head", fg),
                trail: [
                    first,
                    trail.get(1).copied().unwrap_or(mix(first, bg, 0.45)),
                    trail.get(2).copied().unwrap_or(mix(first, bg, 0.75)),
                ],
            }
        });
        Some(Theme {
            name: leak(pack.name.clone()),
            bg,
            panel: or("panel", mix(bg, fg, 0.04)),
            strip: or("strip", mix(bg, Color::Rgb(0, 0, 0), 0.4)),
            card: or("card", mix(bg, fg, 0.08)),
            selection: or("selection", mix(bg, accent, 0.25)),
            border,
            fg,
            muted,
            accent,
            needs,
            blocked,
            done,
            working,
            rain,
            ansi,
        })
    }
}

/// The built-in themes, then the packs on this machine whose names aren't taken.
/// Read once: a new pack shows up the next time valk starts.
pub fn all() -> &'static [&'static Theme] {
    // Tests see the built-ins alone, whatever is installed where they run.
    if cfg!(test) {
        return THEMES;
    }
    static ALL: OnceLock<Vec<&'static Theme>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut all = THEMES.to_vec();
        for pack in themes::load() {
            if all.iter().any(|t| t.name.eq_ignore_ascii_case(&pack.name)) {
                continue;
            }
            if let Some(theme) = Theme::from_pack(&pack) {
                all.push(Box::leak(Box::new(theme)));
            }
        }
        all
    })
}

/// A pack's rain: one of the built-in sets by name, or its own glyphs, keeping
/// only those one column wide so a column of rain stays a column.
fn rain_glyphs(value: &str) -> String {
    match value {
        "code" => CODE.to_owned(),
        "hex" => HEX.to_owned(),
        "runes" => RUNES.to_owned(),
        own => {
            let kept: String = own
                .chars()
                .filter(|&c| unicode_width::UnicodeWidthChar::width(c) == Some(1))
                .collect();
            if kept.is_empty() {
                CODE.to_owned()
            } else {
                kept
            }
        }
    }
}

/// Packs are read once and live as long as valk does, like the built-in themes.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// The one word for a state, everywhere the TUI says it (spec: Marks and color).
pub fn state_word(state: AgentState) -> &'static str {
    match state {
        AgentState::NeedsInput => "asks",
        AgentState::Blocked | AgentState::Interrupted | AgentState::Stale => "stuck",
        AgentState::ReviewReady => "finished",
        AgentState::Working => "working",
        AgentState::Idle | AgentState::Exited => "quiet",
    }
}

/// `a` blended toward `b` by `t` (0 is `a`, 1 is `b`). Named colors stay `a`.
fn mix(a: Color, b: Color, t: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => Color::Rgb(
            (r1 as f32 + (r2 as f32 - r1 as f32) * t) as u8,
            (g1 as f32 + (g2 as f32 - g1 as f32) * t) as u8,
            (b1 as f32 + (b2 as f32 - b1 as f32) * t) as u8,
        ),
        _ => a,
    }
}

/// The mark in front of a state (spec: Marks and color): a dot in the state's
/// color for what waits on you, a spinner while working, a ring when quiet. All
/// are in common coding fonts.
pub fn state_icon(state: AgentState, frame: usize) -> &'static str {
    const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    match state {
        AgentState::NeedsInput
        | AgentState::Blocked
        | AgentState::ReviewReady
        | AgentState::Interrupted
        | AgentState::Stale => "●",
        AgentState::Working => SPIN[frame % SPIN.len()],
        AgentState::Idle | AgentState::Exited => "○",
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

    fn pack(toml: &[(&str, &str)]) -> Pack {
        Pack {
            name: "mine".into(),
            dir: std::path::PathBuf::new(),
            values: toml
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn a_pack_needs_its_three_colors_and_defaults_the_rest() {
        assert!(Theme::from_pack(&pack(&[("bg", "#000000"), ("fg", "#ffffff")])).is_none());
        assert!(
            Theme::from_pack(&pack(&[
                ("bg", "#000000"),
                ("fg", "white"),
                ("accent", "#ff00ff")
            ]))
            .is_none()
        );
        let t = Theme::from_pack(&pack(&[
            ("bg", "#07050d"),
            ("fg", "#efe6ff"),
            ("accent", "#b46bff"),
        ]))
        .unwrap();
        assert_eq!(t.name, "mine");
        assert_eq!(t.accent, hex(0xb46bff));
        assert_eq!(t.needs, DRACULA.needs);
        assert_ne!(t.muted, t.fg);
        assert!(t.rain.is_none());
        assert_eq!(t.ansi[15], t.fg);
    }

    #[test]
    fn a_pack_brings_its_rain_and_colors() {
        let t = Theme::from_pack(&pack(&[
            ("bg", "#07050d"),
            ("fg", "#efe6ff"),
            ("accent", "#b46bff"),
            ("needs", "#ffe36e"),
            ("ansi", "#000000 #ff0000"),
            ("rain.glyphs", "ｱｲ01あ"),
            ("rain.trail", "#c03ae0"),
        ]))
        .unwrap();
        assert_eq!(t.needs, hex(0xffe36e));
        assert_eq!((t.ansi[0], t.ansi[1]), (hex(0x000000), hex(0xff0000)));
        let rain = t.rain.unwrap();
        // The wide あ is dropped: a column of rain stays one column.
        assert_eq!(rain.glyphs, "ｱｲ01");
        assert_eq!(rain.trail[0], hex(0xc03ae0));
        assert_eq!(rain.head, t.fg);
        assert_eq!(rain_glyphs("runes"), RUNES);
    }

    #[test]
    fn the_selected_row_lifts_faint_words_and_leaves_the_rest() {
        use ratatui::style::Stylize;
        for t in THEMES {
            let line = ratatui::text::Line::from(vec!["api".fg(t.muted), "name".fg(t.fg)]);
            let lifted = t.lift_faint(line);
            assert_eq!(lifted.spans[0].style.fg, Some(t.dim()), "{}", t.name);
            assert_eq!(lifted.spans[1].style.fg, Some(t.fg));
            assert_ne!(t.dim(), t.muted, "{}: muted sits above faint", t.name);
        }
    }

    #[test]
    fn term_colors_carry_the_theme() {
        let c = CATPPUCCINLATTE.term_colors();
        assert_eq!(c.bg, [0xef, 0xf1, 0xf5]);
        assert_eq!(c.ansi.len(), 16);
        assert_eq!(c.ansi[1], [0xd2, 0x0f, 0x39]);
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

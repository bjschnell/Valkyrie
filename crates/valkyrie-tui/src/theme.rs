//! Color themes for Valkyrie's own chrome (home screen, bars). A hosted program's
//! screen is always drawn in its own colors.

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
}

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
};

/// Neon on deep navy.
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
};

pub const THEMES: &[&Theme] = &[
    &DRACULA,
    &CYBERPUNK,
    &BLACKOUT,
    &CATPPUCCIN,
    &NORD,
    &GRUVBOX,
    &TOKYONIGHT,
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

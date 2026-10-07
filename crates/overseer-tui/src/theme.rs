//! Color themes for overseer's own chrome (home screen, bars). A hosted program's
//! screen is always drawn in its own colors.

use overseer_proto::AgentState;
use ratatui::style::Color;
use std::path::PathBuf;

pub struct Theme {
    pub name: &'static str,
    /// Page background, and the slightly lifted panel and selection backgrounds.
    pub bg: Color,
    pub panel: Color,
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

pub const THEMES: &[&Theme] = &[&DRACULA, &CYBERPUNK];

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

    pub fn next(&self) -> &'static Theme {
        let i = THEMES.iter().position(|t| t.name == self.name).unwrap_or(0);
        THEMES[(i + 1) % THEMES.len()]
    }

    /// `$OVERSEER_THEME`, else the last one picked with `t`, else Dracula.
    pub fn load() -> &'static Theme {
        std::env::var("OVERSEER_THEME")
            .ok()
            .and_then(|n| Theme::by_name(&n))
            .or_else(|| {
                let saved = std::fs::read_to_string(saved_path()).ok()?;
                Theme::by_name(&saved)
            })
            .unwrap_or(&DRACULA)
    }

    /// Remembers the pick for the next start; failing to is not worth an error.
    pub fn save(&self) {
        let path = saved_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.name);
    }
}

fn saved_path() -> PathBuf {
    overseer_proto::state_dir().join("tui-theme")
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
    fn names_round_trip_and_cycle() {
        for theme in THEMES {
            assert_eq!(Theme::by_name(theme.name).unwrap().name, theme.name);
        }
        assert_eq!(Theme::by_name(" Cyberpunk\n").unwrap().name, "cyberpunk");
        assert!(Theme::by_name("nope").is_none());
        assert_eq!(DRACULA.next().name, "cyberpunk");
        assert_eq!(CYBERPUNK.next().name, "dracula");
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

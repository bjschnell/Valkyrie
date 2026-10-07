//! Row-granular screen updates (ADR-0003).
//!
//! A row update replaces the whole row: cells not covered by a span are blank with
//! default style. Spans start at column `x`; clients advance by each character's
//! display width (wide characters occupy two columns, combining marks zero).

use crate::Size;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenUpdate {
    /// True when `rows` holds every row and the client should discard its old state.
    pub full: bool,
    pub size: Size,
    pub rows: Vec<Row>,
    pub cursor: Cursor,
    pub modes: Modes,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub y: u16,
    pub spans: Vec<Span>,
    /// The line continues on the next row (soft wrap), so copied text joins them.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wrapped: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub x: u16,
    pub text: String,
    #[serde(default, skip_serializing_if = "Style::is_default")]
    pub style: Style,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Style {
    #[serde(default, skip_serializing_if = "Color::is_default")]
    pub fg: Color,
    #[serde(default, skip_serializing_if = "Color::is_default")]
    pub bg: Color,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub flags: u8,
}

impl Style {
    pub const BOLD: u8 = 1;
    pub const ITALIC: u8 = 1 << 1;
    pub const UNDERLINE: u8 = 1 << 2;
    pub const INVERSE: u8 = 1 << 3;
    pub const DIM: u8 = 1 << 4;
    pub const HIDDEN: u8 = 1 << 5;
    pub const STRIKEOUT: u8 = 1 << 6;

    pub fn is_default(&self) -> bool {
        *self == Style::default()
    }
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    pub fn is_default(&self) -> bool {
        *self == Color::Default
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub x: u16,
    pub y: u16,
    pub visible: bool,
    pub shape: CursorShape,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    #[default]
    Block,
    Underline,
    Beam,
}

/// Terminal modes a client must mirror on its own terminal so that the raw input
/// bytes it forwards are encoded the way the program expects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Modes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    pub alt_screen: bool,
    /// Mouse reporting the program asked for (1000/1002/1003) and its encoding
    /// (1006 SGR, 1005 UTF-8). With none, the client may use the mouse itself.
    pub mouse_click: bool,
    pub mouse_drag: bool,
    pub mouse_motion: bool,
    pub mouse_sgr: bool,
    pub mouse_utf8: bool,
    /// 1007: the wheel scrolls by sending arrow keys in the alternate screen.
    pub alt_scroll: bool,
}

impl Modes {
    pub fn wants_mouse(&self) -> bool {
        self.mouse_click || self.mouse_drag || self.mouse_motion
    }
}

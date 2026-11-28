//! The web server page (backlog #321): Enter on the settings panel's Web server
//! row. A dialog, as the keys page is: whether the daemon keeps the web app
//! running, where it listens (this machine, or every interface after a second
//! press), its port, the address phones open, and what it's doing. The daemon
//! keeps these in `settings.toml` (`ClientMsg::WebServerConfig`).

use crate::theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::{self, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use valkyrie_proto::{WebRun, WebServerState};

const LOCAL: &str = "127.0.0.1";
const EVERY: &str = "0.0.0.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    Start,
    Listen,
    Port,
    Url,
}

impl Row {
    pub(crate) const ALL: [Row; 4] = [Row::Start, Row::Listen, Row::Port, Row::Url];

    fn label(self) -> &'static str {
        match self {
            Row::Start => "Start",
            Row::Listen => "Listen on",
            Row::Port => "Port",
            Row::Url => "Phone URL",
        }
    }

    /// Typed in, not picked with ←→.
    fn typed(self) -> bool {
        matches!(self, Row::Port | Row::Url)
    }
}

/// A change that takes a second press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Arm {
    /// Turning it off: the web app this may be serving stops.
    Off,
    /// Every interface: anyone who reaches the port can try to pair.
    Every,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Page {
    pub cursor: usize,
    /// What the daemon said last; none until it answers.
    pub state: Option<WebServerState>,
    /// Why the last change was refused, until the next one.
    pub error: Option<String>,
    /// The port or URL being typed, on the cursor's row.
    pub editing: Option<String>,
    pub armed: Option<Arm>,
}

/// What a key on the page asks the app to do.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Do {
    Nothing,
    /// Back to the settings panel.
    Close,
    Set {
        on: Option<bool>,
        listen: Option<String>,
        url: Option<String>,
    },
}

/// `127.0.0.1:8790` → (`127.0.0.1`, `8790`); `[::1]:9000` → (`::1`, `9000`).
fn split(listen: &str) -> (&str, &str) {
    let (host, port) = listen.rsplit_once(':').unwrap_or((listen, ""));
    (host.trim_start_matches('[').trim_end_matches(']'), port)
}

fn join(host: &str, port: &str) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

impl Page {
    fn row(&self) -> Row {
        Row::ALL[self.cursor.min(Row::ALL.len() - 1)]
    }

    fn host(&self) -> Option<&str> {
        self.state.as_ref().map(|s| split(&s.listen).0)
    }

    /// The daemon answered: a change went through, or it's polled.
    pub(crate) fn loaded(&mut self, state: Result<WebServerState, String>) {
        match state {
            Ok(state) => {
                self.state = Some(state);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// One key. Mouse reports and pastes are the caller's to drop.
    pub(crate) fn key(&mut self, key: &[u8], whole: bool) -> Do {
        let row = self.row();
        if let Some(text) = &mut self.editing {
            match key {
                b"\r" => {
                    let text = self.editing.take().unwrap_or_default();
                    return self.save(row, text.trim());
                }
                b"\x1b" if whole => self.editing = None,
                [0x7f] | [0x08] => {
                    text.pop();
                }
                [c] if (0x20..0x7f).contains(c)
                    && (row == Row::Url || (c.is_ascii_digit() && text.len() < 5)) =>
                {
                    text.push(*c as char);
                }
                _ => {}
            }
            return Do::Nothing;
        }
        let Some(state) = &self.state else {
            return match key {
                b"\x1b" if whole => Do::Close,
                b"q" | [0x03] => Do::Close,
                _ => Do::Nothing,
            };
        };
        let last = Row::ALL.len() - 1;
        let armed = self.armed.take();
        match key {
            b"k" | b"\x1b[A" | b"\x1bOA" => self.cursor = self.cursor.saturating_sub(1),
            b"j" | b"\x1b[B" | b"\x1bOB" => self.cursor = (self.cursor + 1).min(last),
            b"\x1b" if whole => return Do::Close,
            b"q" | [0x03] => return Do::Close,
            b"\r" | b" " | b"h" | b"l" | b"\x1b[D" | b"\x1bOD" | b"\x1b[C" | b"\x1bOC" => {
                if row.typed() {
                    if matches!(key, b"\r" | b" ") {
                        self.editing = Some(match row {
                            Row::Port => split(&state.listen).1.to_owned(),
                            _ => state.url.clone().unwrap_or_default(),
                        });
                    }
                    return Do::Nothing;
                }
                let toward = match key {
                    b"h" | b"\x1b[D" | b"\x1bOD" => Some(false),
                    b"l" | b"\x1b[C" | b"\x1bOC" => Some(true),
                    _ => None,
                };
                return self.flip(row, toward, armed);
            }
            _ => {}
        }
        Do::Nothing
    }

    /// ←→ (or Enter) on Start or Listen on. On Listen on, ← is this machine, →
    /// every interface, and Enter the other one; only every interface takes a
    /// second press.
    fn flip(&mut self, row: Row, toward: Option<bool>, armed: Option<Arm>) -> Do {
        let Some(state) = &self.state else {
            return Do::Nothing;
        };
        let (host, port) = split(&state.listen);
        match row {
            Row::Start if state.on && armed != Some(Arm::Off) => {
                self.armed = Some(Arm::Off);
                Do::Nothing
            }
            Row::Start => Do::Set {
                on: Some(!state.on),
                listen: None,
                url: None,
            },
            Row::Listen => {
                let every = toward.unwrap_or(host != EVERY);
                let to = if every { EVERY } else { LOCAL };
                if to == host {
                    Do::Nothing
                } else if every && armed != Some(Arm::Every) {
                    self.armed = Some(Arm::Every);
                    Do::Nothing
                } else {
                    Do::Set {
                        on: None,
                        listen: Some(join(to, port)),
                        url: None,
                    }
                }
            }
            Row::Port | Row::Url => Do::Nothing,
        }
    }

    fn save(&mut self, row: Row, text: &str) -> Do {
        let Some(state) = &self.state else {
            return Do::Nothing;
        };
        let (host, port) = split(&state.listen);
        match row {
            Row::Port if text != port && !text.is_empty() => Do::Set {
                on: None,
                listen: Some(join(host, text)),
                url: None,
            },
            Row::Url if text != state.url.as_deref().unwrap_or("") => Do::Set {
                on: None,
                listen: None,
                url: Some(text.to_owned()),
            },
            _ => Do::Nothing,
        }
    }

    /// What the server is doing, as a word in its state color and a line.
    fn run(&self, t: &Theme) -> (Span<'static>, String) {
        let Some(s) = &self.state else {
            return ("…".fg(t.muted), "asking the daemon".into());
        };
        let at = format!("http://{}", s.listen);
        match s.run {
            WebRun::Running => ("running".fg(t.done), at),
            WebRun::Starting => ("starting".fg(t.working), at),
            WebRun::Off => (
                "off".fg(t.muted),
                "valk web or valk setup web still start it".into(),
            ),
            WebRun::Service => (
                "stood aside".fg(t.muted),
                "the valk setup web service runs it".into(),
            ),
            WebRun::Taken => (
                "port taken".fg(t.blocked),
                format!("something else listens on {}", s.listen),
            ),
            WebRun::Unmanaged => (
                "not kept".fg(t.muted),
                "this daemon doesn't start one".into(),
            ),
        }
    }

    fn value(&self, row: Row) -> String {
        let Some(s) = &self.state else {
            return String::new();
        };
        let (host, port) = split(&s.listen);
        match row {
            Row::Start if self.armed == Some(Arm::Off) => "off? press again".into(),
            Row::Start => if s.on { "on" } else { "off" }.into(),
            Row::Listen if self.armed == Some(Arm::Every) => "all interfaces? press again".into(),
            Row::Listen => match host {
                LOCAL => "this machine".into(),
                EVERY => "all interfaces".into(),
                other => format!("{other} (settings.toml)"),
            },
            Row::Port => port.into(),
            Row::Url => match &s.url {
                Some(url) => url.clone(),
                None => format!("{} (default)", s.default_url),
            },
        }
    }

    /// The line under the rows: why a change was refused, the pairing warning,
    /// or what a press does here.
    fn note(&self) -> Option<String> {
        if let Some(e) = &self.error {
            return Some(e.clone());
        }
        let state = self.state.as_ref()?;
        if self.armed == Some(Arm::Every) || self.host() == Some(EVERY) {
            return Some(format!(
                "anyone who can reach port {} can try to pair",
                split(&state.listen).1
            ));
        }
        if self.armed == Some(Arm::Off) {
            return Some("the web app stops, this page in a browser too".into());
        }
        if self.editing.is_some() && self.row() == Row::Url {
            return Some(format!("empty is the default, {}", state.default_url));
        }
        None
    }

    /// The dialog, in the middle of `area`, over what the caller dimmed.
    pub(crate) fn draw(&self, frame: &mut ratatui::Frame, t: &Theme) {
        let area = frame.area();
        let width = 64.min(area.width);
        let inner = (width as usize).saturating_sub(2);
        let (word, at) = self.run(t);
        let mut lines = vec![
            Line::default(),
            Line::from(vec![
                "  ".into(),
                word.bold(),
                "  ".into(),
                crate::fit(&at, inner.saturating_sub(16)).fg(t.muted),
            ]),
            Line::default(),
        ];
        for (i, &row) in Row::ALL.iter().enumerate() {
            let here = i == self.cursor;
            let typing = here && self.editing.is_some();
            let arrow = |a: &'static str| {
                if here && !row.typed() {
                    a.fg(t.accent)
                } else {
                    "  ".into()
                }
            };
            let value = match &self.editing {
                Some(text) if typing => format!("{text}▏"),
                _ => self.value(row),
            };
            let room = inner.saturating_sub(18);
            let value = crate::fit(&value, room);
            let mut line = Line::from(vec![
                format!("  {:<12}", row.label()).fg(if here { t.fg } else { t.muted }),
                arrow("‹ "),
                if here {
                    value.fg(t.accent).bold()
                } else {
                    value.fg(t.fg)
                },
                arrow(" ›"),
            ]);
            let pad = inner.saturating_sub(line.width());
            line.spans.push(" ".repeat(pad).into());
            lines.push(if here {
                line.style(style::Style::new().bg(t.selection))
            } else {
                line
            });
        }
        lines.push(Line::default());
        if let Some(note) = self.note() {
            let color = if self.error.is_some() {
                t.blocked
            } else {
                t.needs
            };
            lines.push(Line::from(
                format!("  {}", crate::fit(&note, inner.saturating_sub(2))).fg(color),
            ));
        }
        lines.push(Line::default());
        let height = lines.len() as u16 + 2;
        let rect = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        )
        .intersection(area);
        let hint = |k: &'static str, w: &'static str| [k.fg(t.accent).bold(), w.fg(t.muted)];
        let hints: Vec<Span> = if self.editing.is_some() {
            [hint(" ↩ ", "save "), hint(" esc ", "cancel ")].concat()
        } else if self.row().typed() {
            [
                hint(" ↑↓ ", "choose "),
                hint(" ↩ ", "edit "),
                hint(" esc ", "back "),
            ]
            .concat()
        } else {
            [
                hint(" ↑↓ ", "choose "),
                hint(" ←→ ", "change "),
                hint(" esc ", "back "),
            ]
            .concat()
        };
        frame.render_widget(Clear, rect);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(style::Style::new().fg(t.accent))
            .title(" Web server ".fg(t.fg).bold())
            .title_bottom(Line::from(hints).centered())
            .style(style::Style::new().bg(t.panel).fg(t.fg));
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(listen: &str, on: bool) -> Page {
        Page {
            state: Some(WebServerState {
                on,
                listen: listen.into(),
                url: None,
                default_url: "https://thor.tail1.ts.net".into(),
                run: WebRun::Running,
            }),
            ..Page::default()
        }
    }

    fn set_listen(to: &str) -> Do {
        Do::Set {
            on: None,
            listen: Some(to.into()),
            url: None,
        }
    }

    #[test]
    fn every_interface_and_off_take_a_second_press() {
        let mut p = page("127.0.0.1:8790", true);
        // Start: off asks first.
        assert_eq!(p.key(b"l", true), Do::Nothing);
        assert_eq!(p.armed, Some(Arm::Off));
        assert_eq!(
            p.key(b"l", true),
            Do::Set {
                on: Some(false),
                listen: None,
                url: None
            }
        );
        // Moving away disarms.
        p.key(b"l", true);
        p.key(b"j", true);
        assert_eq!(p.armed, None);
        assert_eq!(p.key(b"l", true), Do::Nothing);
        assert!(p.note().unwrap().contains("can try to pair"));
        assert_eq!(p.key(b"l", true), set_listen("0.0.0.0:8790"));
        // Back to this machine at once, from every interface or a tailnet IP.
        let mut p = page("0.0.0.0:9000", true);
        p.cursor = 1;
        assert_eq!(p.key(b"h", true), set_listen("127.0.0.1:9000"));
        let mut p = page("100.101.102.103:8790", true);
        p.cursor = 1;
        assert_eq!(p.value(Row::Listen), "100.101.102.103 (settings.toml)");
        assert_eq!(p.key(b"h", true), set_listen("127.0.0.1:8790"));
        assert_eq!(p.key(b"\r", true), Do::Nothing);
        assert_eq!(p.key(b"\r", true), set_listen("0.0.0.0:8790"));
        // Already there: nothing to do.
        let mut p = page("127.0.0.1:8790", true);
        p.cursor = 1;
        assert_eq!(p.key(b"h", true), Do::Nothing);
    }

    #[test]
    fn the_port_and_url_are_typed_and_saved_with_enter() {
        let mut p = page("[::1]:8790", true);
        p.cursor = 2;
        p.key(b"\r", true);
        assert_eq!(p.editing.as_deref(), Some("8790"));
        for k in [
            &[0x7f][..],
            &[0x7f],
            &[0x7f],
            &[0x7f],
            b"9",
            b"x",
            b"1",
            b"0",
            b"0",
        ] {
            p.key(k, true);
        }
        assert_eq!(p.editing.as_deref(), Some("9100"));
        assert_eq!(p.key(b"\r", true), set_listen("[::1]:9100"));
        // Esc gives up; Enter on what's there already sends nothing.
        p.key(b"\r", true);
        assert_eq!(p.key(b"\x1b", true), Do::Nothing);
        assert_eq!(p.editing, None);
        p.key(b"\r", true);
        assert_eq!(p.key(b"\r", true), Do::Nothing);
        // The URL: empty is the default.
        p.cursor = 3;
        assert_eq!(p.value(Row::Url), "https://thor.tail1.ts.net (default)");
        p.key(b"\r", true);
        for c in "https://v.example".bytes() {
            p.key(&[c], true);
        }
        assert_eq!(
            p.key(b"\r", true),
            Do::Set {
                on: None,
                listen: None,
                url: Some("https://v.example".into())
            }
        );
    }

    #[test]
    fn esc_goes_back_even_before_the_daemon_answers() {
        let mut p = Page::default();
        assert_eq!(p.key(b"j", true), Do::Nothing);
        assert_eq!(p.key(b"\x1b", true), Do::Close);
        assert_eq!(page("127.0.0.1:8790", true).key(b"q", true), Do::Close);
    }
}

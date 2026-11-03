//! The mini terminal: a small shell in a box over whatever is on screen, for the
//! side command an agent asked for. The key opens it, the same key puts it away,
//! and the shell keeps running while hidden. Its session is hidden from the daemon's
//! lists and ends with this TUI's connection (`SpawnSpec::hidden`).
//!
//! It is watched only while the box is open: showing it attaches it beside the
//! tab's panes, hiding it attaches the panes alone. While it is open every key but
//! its own goes to the shell, and the mouse is ignored.

use crate::keys::Action;
use crate::mouse::{self, Input};
use crate::{App, Attached, write_modes};
use anyhow::Result;
use ratatui::layout::Rect;
use ratatui::style;
use ratatui::text::Line;
use valkyrie_proto::{ServerMsg, SessionId, Size, SpawnSpec};

/// The least a box is, so a small terminal still gets a usable one.
const MIN_COLS: u16 = 40;
const MIN_ROWS: u16 = 10;

pub(crate) struct Mini {
    pub(crate) pane: Attached,
    /// The box is on screen (and the daemon is sending its screen).
    pub(crate) open: bool,
}

/// The box within `body` (the terminal minus the bar): four fifths of the width and
/// three fifths of the height, centered.
pub(crate) fn mini_rect(body: Rect) -> Rect {
    let width = (body.width * 4 / 5).max(MIN_COLS.min(body.width));
    let height = (body.height * 3 / 5).max(MIN_ROWS.min(body.height));
    Rect::new(
        body.x + (body.width - width) / 2,
        body.y + (body.height - height) / 2,
        width,
        height,
    )
}

impl App {
    pub(crate) fn mini_open(&self) -> bool {
        self.mini.as_ref().is_some_and(|m| m.open)
    }

    /// What the shell gets: the box minus its border.
    fn mini_size(&self) -> Option<Size> {
        let full = self.screen().ok()?;
        let r = mini_rect(Rect::new(0, 0, full.cols, full.rows));
        Some(Size {
            cols: r.width.saturating_sub(4).max(1),
            rows: r.height.saturating_sub(2).max(1),
        })
    }

    /// Watches the tab's panes, and the mini terminal when it is open.
    pub(crate) async fn attach_set(&self, mut sizes: Vec<(SessionId, Size)>) -> Result<()> {
        if let Some(m) = self.mini.as_ref().filter(|m| m.open) {
            sizes.push((m.pane.id, m.pane.size));
        }
        self.client.attach_panes(sizes).await
    }

    pub(crate) async fn toggle_mini(&mut self) {
        if self.mini_open() {
            self.hide_mini().await;
        } else {
            self.show_mini().await;
        }
    }

    async fn show_mini(&mut self) {
        let Some(size) = self.mini_size() else {
            return self.say("no terminal size".into());
        };
        let alive = self.mini.as_ref().is_some_and(|m| m.pane.exited.is_none());
        if !alive {
            let spec = SpawnSpec {
                command: vec![valkyrie_proto::default_shell()],
                cwd: self.attached_cwd(),
                name: Some("mini".into()),
                size,
                env: valkyrie_proto::login_env(),
                worktree: None,
                hidden: true,
            };
            match self.client.spawn(spec).await {
                Ok(info) => {
                    let pane = Attached::new(info.id, "mini".into(), None);
                    self.mini = Some(Mini { pane, open: false });
                    self.report_cell(Some(info.id));
                }
                Err(e) => return self.say(format!("mini terminal failed: {e:#}")),
            }
        }
        let Some(m) = self.mini.as_mut() else { return };
        m.open = true;
        m.pane.size = size;
        self.tab_mode = false;
        let sizes = self.pane_sizes();
        if let Err(e) = self.attach_set(sizes).await {
            // Gone from under us (the daemon restarted): the next press starts afresh.
            self.mini = None;
            self.say(format!("mini terminal not attached: {e:#}"));
        }
    }

    pub(crate) async fn hide_mini(&mut self) {
        let Some(m) = self.mini.as_mut().filter(|m| m.open) else {
            return;
        };
        m.open = false;
        m.pane.selection = None;
        let sizes = self.pane_sizes();
        if let Err(e) = self.attach_set(sizes).await {
            self.say(format!("panes not attached: {e:#}"));
        }
        // The program underneath gets its terminal modes back.
        match self.view.as_ref().map(|v| (v.modes, v.shape)) {
            Some((modes, shape)) => {
                let _ = write_modes(self.outer_modes(modes), Some(shape));
            }
            None => self.reset_outer_modes(),
        }
    }

    /// The shell ended (`exit`): put the box away; the next press starts a new one.
    pub(crate) async fn mini_reap(&mut self) {
        if self.mini.as_ref().is_some_and(|m| m.pane.exited.is_some()) {
            self.hide_mini().await;
            self.mini = None;
        }
    }

    /// Keys while the box is open: all the shell's but its own, which puts it away.
    pub(crate) async fn mini_input(&mut self, mut bytes: Vec<u8>) {
        if let Some(cut) = mouse::unfinished(&bytes) {
            self.partial = bytes.split_off(cut);
        }
        for input in mouse::split(&bytes) {
            let Input::Bytes(keys) = input else { continue };
            let chord = (0..keys.len()).find_map(|i| match self.settings.keys.at(&keys[i..]) {
                Some((Action::Mini, n)) => Some((i, n)),
                _ => None,
            });
            let Some(id) = self.mini.as_ref().map(|m| m.pane.id) else {
                return;
            };
            match chord {
                Some((i, n)) => {
                    if i > 0 {
                        let _ = self.client.input(id, keys[..i].to_vec());
                    }
                    self.hide_mini().await;
                    let rest = keys[i + n..].to_vec();
                    if !rest.is_empty() {
                        return Box::pin(self.on_input(rest)).await;
                    }
                }
                None => {
                    let _ = self.client.input(id, keys);
                }
            }
        }
    }

    /// A push for the mini terminal's session: handled here, else handed back.
    pub(crate) fn mini_push(&mut self, msg: ServerMsg) -> Result<Option<ServerMsg>> {
        let session = match &msg {
            ServerMsg::Screen { session, .. }
            | ServerMsg::Graphics { session, .. }
            | ServerMsg::Clipboard { session, .. }
            | ServerMsg::Exited { session, .. } => *session,
            _ => return Ok(Some(msg)),
        };
        let Some(m) = self.mini.as_mut().filter(|m| m.pane.id == session) else {
            return Ok(Some(msg));
        };
        match msg {
            ServerMsg::Screen { update, .. } => {
                let before = (m.pane.modes, m.pane.shape);
                m.pane.apply(update);
                if m.open && before != (m.pane.modes, m.pane.shape) {
                    write_modes(m.pane.modes, Some(m.pane.shape))?;
                }
            }
            ServerMsg::Exited { code, .. } => m.pane.exited = Some(code),
            // Images and the clipboard are for the tab's programs.
            _ => {}
        }
        Ok(None)
    }

    /// Resizes the shell after the window changed.
    pub(crate) fn resize_mini(&mut self) {
        let Some(size) = self.mini_size() else { return };
        if let Some(m) = self.mini.as_mut().filter(|m| m.open)
            && m.pane.size != size
        {
            m.pane.size = size;
            let _ = self.client.resize(m.pane.id, size);
        }
    }

    pub(crate) fn draw_mini(&self, frame: &mut ratatui::Frame) {
        let Some(m) = self.mini.as_ref().filter(|m| m.open) else {
            return;
        };
        let full = frame.area();
        let body = Rect::new(0, 0, full.width, full.height.saturating_sub(1));
        let r = mini_rect(body);
        let t = self.settings.theme;
        let hide = self
            .settings
            .keys
            .get(Action::Mini)
            .map_or("unbound".into(), |k| k.label());
        let title = Line::from(format!(" mini terminal · {hide} puts it away "));
        let block = crate::panel(title, t, true);
        let inner = block.inner(r);
        frame.render_widget(ratatui::widgets::Clear, r);
        frame.render_widget(block, r);
        let themed = self.settings.themed_sessions.then_some(t);
        crate::render_rows(m.pane.shown(), inner, frame.buffer_mut(), themed);
        if let Some(code) = m.pane.exited {
            let text = match code {
                Some(c) => format!(" exited {c} "),
                None => " exited ".into(),
            };
            let line = Line::from(text).style(style::Style::new().fg(t.blocked).bg(t.panel));
            frame.render_widget(
                line,
                Rect::new(inner.x, inner.bottom().saturating_sub(1), inner.width, 1),
            );
        }
        if let Some((x, y)) = m.pane.cursor.filter(|_| m.pane.scroll.is_none())
            && x < inner.width
            && y < inner.height
        {
            frame.set_cursor_position(ratatui::layout::Position::new(inner.x + x, inner.y + y));
        }
    }
}

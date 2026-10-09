//! Split panes inside a tab (DESIGN §8.9). The daemon keeps each split tab's layout;
//! the TUI watches every pane of the attached tab and draws each in its own area.
//! `App::view` is the focused pane, which gets the keyboard and the bar; the others
//! are in `App::others`.

use crate::mouse::{self, Mouse, MouseKind};
use crate::{App, Attached, MENU_WIDTH, Menu, MenuKind, reset_terminal_modes, write_modes};
use ratatui::layout::{Position, Rect};
use ratatui::style;
use valkyrie_proto::layout::{Area, Divider};
use valkyrie_proto::{SessionId, Side, Size, SpawnSpec};

/// Sent to a program that asked for focus events (1004) as its pane gains or loses
/// the keyboard.
const FOCUS_IN: &[u8] = b"\x1b[I";
const FOCUS_OUT: &[u8] = b"\x1b[O";

fn area(r: Rect) -> Area {
    Area {
        x: r.x,
        y: r.y,
        cols: r.width,
        rows: r.height,
    }
}

fn rect(a: Area) -> Rect {
    Rect::new(a.x, a.y, a.cols, a.rows)
}

impl App {
    /// The tabs in order, each the places in `sessions` of its panes: one, unless the
    /// tab is split. A tab is where its first pane is.
    pub(crate) fn tabs(&self) -> Vec<Vec<usize>> {
        let mut placed = vec![false; self.sessions.len()];
        let mut tabs = Vec::new();
        for i in 0..self.sessions.len() {
            if placed[i] {
                continue;
            }
            let id = self.sessions[i].id;
            let tab: Vec<usize> = match self.layouts.iter().find(|p| p.contains(id)) {
                Some(p) => (i..self.sessions.len())
                    .filter(|&j| p.contains(self.sessions[j].id))
                    .collect(),
                None => vec![i],
            };
            for &j in &tab {
                placed[j] = true;
            }
            tabs.push(tab);
        }
        tabs
    }

    /// The session a tab shows: its focused pane when attached, else the pane that
    /// needs you first, else its first.
    pub(crate) fn face(&self, tab: &[usize]) -> usize {
        let is = |i: &&usize, id: SessionId| self.sessions[**i].id == id;
        let mut ids = self
            .view
            .iter()
            .map(|v| v.id)
            .chain(self.queue.iter().map(|q| q.session));
        ids.find_map(|id| tab.iter().find(|i| is(i, id)).copied())
            .unwrap_or(tab[0])
    }

    /// The tab holding session `id`, as an index into `tabs()`.
    pub(crate) fn tab_of(&self, id: SessionId) -> Option<usize> {
        self.tabs()
            .iter()
            .position(|t| t.iter().any(|&i| self.sessions[i].id == id))
    }

    /// Every pane on screen, the focused one first.
    pub(crate) fn pane_ids(&self) -> Vec<SessionId> {
        self.view.iter().chain(&self.others).map(|p| p.id).collect()
    }

    pub(crate) fn pane_mut(&mut self, id: SessionId) -> Option<&mut Attached> {
        self.view
            .iter_mut()
            .chain(self.others.iter_mut())
            .find(|p| p.id == id)
    }

    fn pane(&self, id: SessionId) -> Option<&Attached> {
        self.view.iter().chain(&self.others).find(|p| p.id == id)
    }

    /// Where the panes go now: the terminal minus the bar and the tab strip.
    pub(crate) fn main_area(&self) -> Option<Rect> {
        let full = self.screen().ok()?;
        Some(self.split_body(Rect::new(0, 0, full.cols, full.rows)).0)
    }

    /// Each pane's area within `main`, and the dividers between them.
    pub(crate) fn pane_layout(&self, main: Rect) -> (Vec<(SessionId, Rect)>, Vec<Divider>) {
        match (&self.layout, &self.view) {
            (Some(layout), _) => {
                let (panes, dividers) = layout.layout(area(main));
                (
                    panes.into_iter().map(|(id, a)| (id, rect(a))).collect(),
                    dividers,
                )
            }
            (None, Some(view)) => (vec![(view.id, main)], Vec::new()),
            (None, None) => (Vec::new(), Vec::new()),
        }
    }

    /// Each pane's size, for the daemon.
    fn pane_sizes(&self) -> Vec<(SessionId, Size)> {
        let Some(main) = self.main_area() else {
            return Vec::new();
        };
        self.pane_layout(main)
            .0
            .into_iter()
            .map(|(id, r)| {
                let size = Size {
                    cols: r.width,
                    rows: r.height,
                };
                (id, size.clamped())
            })
            .collect()
    }

    pub(crate) fn new_pane(&self, id: SessionId) -> Attached {
        let name = self
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| id.to_string());
        Attached::new(id, name, self.sessions_status(id))
    }

    /// Attaches to the tab holding `id`, every pane of it, with `id` focused.
    pub(crate) async fn attach_tab(&mut self, id: SessionId) {
        let layout = self.layouts.iter().find(|p| p.contains(id)).cloned();
        let ids = layout.as_ref().map_or_else(|| vec![id], |p| p.sessions());
        // A TUI running inside a session would feed its own output back to itself.
        if let Ok(own) = std::env::var("VALK_SESSION")
            && let Some(own) = ids.iter().find(|i| i.to_string() == own)
        {
            let text = format!("session {own} is this TUI");
            match &mut self.view {
                Some(view) => view.notice(text),
                None => self.status = text,
            }
            return;
        }
        if self.main_area().is_none() {
            self.status = "no terminal size".into();
            return;
        }
        // Switching from another tab: its modes and images must not carry over.
        if self.view.is_some() {
            let _ = reset_terminal_modes();
        }
        // Set the panes before the request so the snapshots pushed right after the
        // reply land in them.
        self.layout = layout;
        // Panes this connection already watches get no new snapshot: keep their screens.
        let mut kept: Vec<Attached> = self.view.take().into_iter().collect();
        kept.append(&mut self.others);
        let mut take = |o: SessionId, app: &App| match kept.iter().position(|p| p.id == o) {
            Some(i) => kept.swap_remove(i),
            None => app.new_pane(o),
        };
        let others = ids
            .iter()
            .filter(|&&o| o != id)
            .map(|&o| take(o, self))
            .collect();
        let view = take(id, self);
        self.others = others;
        self.view = Some(view);
        self.mouse_owner = None;
        self.divider_drag = None;
        // The snapshots only rewrite modes that differ from the defaults; the mouse
        // capture has to start now.
        self.captured = None;
        self.update_capture();
        for &pane in &ids {
            self.report_cell(Some(pane));
        }
        let sizes = self.pane_sizes();
        for &(pane, size) in &sizes {
            if let Some(p) = self.pane_mut(pane) {
                p.size = size;
            }
        }
        if let Err(e) = self.client.attach_panes(sizes).await {
            self.view = None;
            self.others.clear();
            self.layout = None;
            let _ = reset_terminal_modes();
            self.status = format!("attach {id} failed: {e:#}");
        }
    }

    /// Brings the attached tab's panes in line with its layout, after the session
    /// list came back: a pane split off here or by another client, one that left,
    /// a divider moved.
    pub(crate) async fn sync_panes(&mut self) {
        let Some(focused) = self.view.as_ref().map(|v| v.id) else {
            return;
        };
        let current = self.pane_ids();
        let listed = |id: SessionId| self.sessions.iter().any(|s| s.id == id);
        let layout = current
            .iter()
            .find_map(|&id| self.layouts.iter().find(|p| p.contains(id)))
            .cloned();
        let wanted = match &layout {
            Some(p) => p.sessions(),
            // Down to one pane; a tab whose only pane left is `on_ended`'s.
            None => vec![
                current
                    .iter()
                    .copied()
                    .find(|&id| listed(id))
                    .unwrap_or(focused),
            ],
        };
        let same = wanted.len() == current.len() && wanted.iter().all(|id| current.contains(id));
        // Mid-drag the local ratio is newer than the daemon's, which hears on release.
        if same && self.divider_drag.is_some() {
            return;
        }
        let old = self.main_area().map(|main| self.pane_layout(main).0);
        self.layout = layout;
        if same {
            return self.resize_panes();
        }
        // The pane that took a closed one's room gets the keyboard.
        let focus = if wanted.contains(&focused) {
            focused
        } else {
            // Where it was is now inside that pane. `layout` is already the new
            // one, and a tab down to one pane has only `wanted[0]`.
            let was = old
                .unwrap_or_default()
                .into_iter()
                .find(|p| p.0 == focused)
                .map(|(_, r)| Position::new(r.x, r.y));
            let now = self
                .main_area()
                .filter(|_| self.layout.is_some())
                .map(|main| self.pane_layout(main).0);
            was.zip(now)
                .and_then(|(at, now)| now.into_iter().find(|(_, r)| r.contains(at)))
                .map_or(wanted[0], |(id, _)| id)
        };
        let mut old: Vec<Attached> = self.view.take().into_iter().collect();
        old.append(&mut self.others);
        let mut panes = Vec::new();
        for &id in &wanted {
            panes.push(match old.iter().position(|p| p.id == id) {
                Some(i) => old.swap_remove(i),
                None => self.new_pane(id),
            });
        }
        let i = panes.iter().position(|p| p.id == focus).unwrap_or(0);
        let view = panes.remove(i);
        if view.id != focused {
            let _ = write_modes(view.modes, Some(view.shape));
        }
        self.view = Some(view);
        self.others = panes;
        self.mouse_owner = None;
        self.divider_drag = None;
        self.update_capture();
        for &id in &wanted {
            if !current.contains(&id) {
                self.report_cell(Some(id));
            }
        }
        let sizes = self.pane_sizes();
        for &(id, size) in &sizes {
            if let Some(p) = self.pane_mut(id) {
                p.size = size;
            }
        }
        if let Err(e) = self.client.attach_panes(sizes).await {
            self.say(format!("panes not attached: {e:#}"));
        }
    }

    /// Tells each pane whose area changed its new size.
    pub(crate) fn resize_panes(&mut self) {
        for (id, size) in self.pane_sizes() {
            if let Some(p) = self.pane_mut(id)
                && p.size != size
            {
                p.size = size;
                let _ = self.client.resize(id, size);
            }
        }
    }

    /// Gives pane `id` the keyboard.
    pub(crate) fn focus(&mut self, id: SessionId) {
        let Some(i) = self.others.iter().position(|p| p.id == id) else {
            return;
        };
        let Some(mut old) = self.view.replace(self.others.remove(i)) else {
            return;
        };
        old.selection = None;
        old.selecting = false;
        if old.modes.focus_events {
            let _ = self.client.input(old.id, FOCUS_OUT.to_vec());
        }
        self.others.push(old);
        if let Some(view) = &self.view {
            if view.modes.focus_events {
                let _ = self.client.input(view.id, FOCUS_IN.to_vec());
            }
            let _ = write_modes(view.modes, Some(view.shape));
        }
    }

    /// Moves the keyboard `step` panes on, in layout order.
    pub(crate) fn cycle_focus(&mut self, step: isize) {
        let (Some(layout), Some(view)) = (&self.layout, &self.view) else {
            return;
        };
        let order = layout.sessions();
        let Some(i) = order.iter().position(|&id| id == view.id) else {
            return;
        };
        let n = order.len() as isize;
        let next = order[(i as isize + step).rem_euclid(n) as usize];
        self.focus(next);
    }

    /// Splits the focused pane: a shell in its directory goes on `side`.
    pub(crate) async fn split(&mut self, side: Side) {
        let Some(at) = self.view.as_ref().map(|v| v.id) else {
            return;
        };
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        // About half the pane; attaching sets the real size.
        let size = self
            .pane(at)
            .map(|p| p.size)
            .filter(|s| s.cols > 0)
            .map_or(Size { cols: 80, rows: 24 }, |s| match side {
                Side::Left | Side::Right => Size {
                    cols: s.cols / 2,
                    ..s
                },
                Side::Up | Side::Down => Size {
                    rows: s.rows / 2,
                    ..s
                },
            })
            .clamped();
        let spec = SpawnSpec {
            command: vec![shell],
            cwd: self.attached_cwd(),
            name: None,
            size,
            env: valkyrie_proto::login_env(),
        };
        match self.client.split(at, side, spec).await {
            Ok(info) => {
                self.refresh().await;
                self.focus(info.id);
            }
            Err(e) => self.say(format!("split failed: {e:#}")),
        }
    }

    /// The mouse wants the program's or Valkyrie's mode: both use SGR reports, and
    /// plain motion only while some pane asked for it.
    pub(crate) fn update_capture(&mut self) {
        let motion = self
            .view
            .iter()
            .chain(&self.others)
            .any(|p| p.modes.mouse_motion);
        if self.captured != Some(motion) {
            self.captured = Some(motion);
            let mut out = std::io::stdout();
            let _ = std::io::Write::write_all(
                &mut out,
                format!(
                    "{}{}{}",
                    mouse::CAPTURE_OFF,
                    mouse::CAPTURE_ON,
                    if motion { mouse::MOTION_ON } else { "" }
                )
                .as_bytes(),
            );
            let _ = std::io::Write::flush(&mut out);
        }
    }

    /// A report below the tab strip. Returns it moved into the cells of the focused
    /// pane when Valkyrie's own handling (scrollback, selection) is next; `None` when
    /// it was used here: on a divider, the pane menu, or passed to the program.
    pub(crate) async fn pane_mouse(&mut self, m: Mouse) -> Option<Mouse> {
        let main = self.main_area()?;
        let wheel = m.code & 64 != 0;
        // A press while a drag is still on means its release got lost (let go
        // outside the window): that gesture is over, this one starts afresh.
        if m.is_press() && !wheel {
            if let Some((d, ratio)) = self.divider_drag.take() {
                self.commit_ratio(d, ratio).await;
            }
            self.mouse_owner = None;
        }
        if let Some((d, _)) = self.divider_drag {
            let pos = match d.axis {
                valkyrie_proto::Axis::Columns => m.x,
                valkyrie_proto::Axis::Rows => m.y,
            };
            let ratio = d.ratio_at(pos);
            if let Some(layout) = &mut self.layout {
                layout.set_ratio(d.a, d.b, ratio);
            }
            self.divider_drag = Some((d, ratio));
            self.resize_panes();
            if m.release {
                self.divider_drag = None;
                self.commit_ratio(d, ratio).await;
            }
            return None;
        }
        let (panes, dividers) = self.pane_layout(main);
        let at = Position::new(m.x, m.y);
        let on_divider = dividers.iter().find(|d| rect(d.area).contains(at));
        if m.kind == MouseKind::Press
            && self.mouse_owner.is_none()
            && let Some(d) = on_divider
        {
            let ratio = d.ratio_at(match d.axis {
                valkyrie_proto::Axis::Columns => d.area.x,
                valkyrie_proto::Axis::Rows => d.area.y,
            });
            self.divider_drag = Some((*d, ratio));
            return None;
        }
        let under = panes.iter().find(|(_, r)| r.contains(at)).map(|p| p.0);
        let target = self.mouse_owner.or(under);
        if m.release {
            self.mouse_owner = None;
        }
        let target = target?;
        let r = panes.iter().find(|p| p.0 == target)?.1;
        // Right clicks are the pane menu's; with Ctrl or Alt held, a program that
        // wants the mouse gets them (vim's and htop's own menus).
        let right = !wheel && m.code & 0b11 == 2;
        let modified = m.code & (8 | 16) != 0;
        let program = self.pane(target).is_some_and(|p| p.modes.wants_mouse());
        if right && !(modified && program) {
            if m.kind == MouseKind::RightPress {
                self.focus(target);
                self.open_pane_menu(target, m.x, m.y);
            }
            return None;
        }
        if m.is_press() && !wheel {
            self.focus(target);
            self.mouse_owner = Some(target);
        }
        let local = Mouse {
            x: m.x.saturating_sub(r.x).min(r.width.saturating_sub(1)),
            y: m.y.saturating_sub(r.y).min(r.height.saturating_sub(1)),
            ..m
        };
        let pane = self.pane(target)?;
        if pane.modes.wants_mouse() {
            if let Some(bytes) = mouse::encode(&m, local.x, local.y, pane.modes) {
                let _ = self.client.input(target, bytes);
            }
            // A selection the program took the mouse in the middle of is over.
            if m.release
                && let Some(p) = self.pane_mut(target)
            {
                p.selecting = false;
            }
            return None;
        }
        if self.view.as_ref().is_some_and(|v| v.id == target) {
            return Some(local);
        }
        // The wheel over another pane scrolls that one, as in tmux.
        if wheel {
            self.wheel(target, local.kind).await;
        }
        None
    }

    /// Tells the daemon where a divider was dragged to, so every client draws it so.
    async fn commit_ratio(&mut self, d: Divider, ratio: u16) {
        if let Err(e) = self.client.ratio(d.a, d.b, ratio).await {
            self.say(format!("resize failed: {e:#}"));
        }
    }

    /// Opens the pane menu at the pointer, kept on screen.
    fn open_pane_menu(&mut self, id: SessionId, x: u16, y: u16) {
        let mut menu = Menu {
            kind: MenuKind::Pane,
            session: id,
            x,
            y,
            cursor: 0,
            confirm: false,
        };
        let (cols, rows) = self.screen().map_or((80, 24), |s| (s.cols, s.rows + 1));
        let r = menu.rect();
        menu.x = x.min(cols.saturating_sub(MENU_WIDTH));
        menu.y = y.min(rows.saturating_sub(r.height));
        self.menu = Some(menu);
    }

    /// The pane menu from the keyboard, at the focused pane's corner.
    pub(crate) fn open_focused_pane_menu(&mut self) {
        let Some(id) = self.view.as_ref().map(|v| v.id) else {
            return;
        };
        let at = self
            .main_area()
            .map(|main| self.pane_layout(main).0)
            .and_then(|panes| panes.into_iter().find(|p| p.0 == id))
            .map_or((0, 0), |(_, r)| (r.x, r.y));
        self.open_pane_menu(id, at.0, at.1);
    }

    /// Does a pane menu item.
    pub(crate) async fn pick_pane(&mut self, id: SessionId, item: usize) {
        let side = match item {
            0 => Side::Right,
            1 => Side::Down,
            2 => Side::Left,
            3 => Side::Up,
            _ => {
                self.menu = None;
                return self.close_session(id).await;
            }
        };
        self.menu = None;
        self.focus(id);
        self.split(side).await;
    }

    /// Draws every pane in `main`, the dividers between them, and the focused pane's
    /// cursor. The dividers along the focused pane are lit.
    pub(crate) fn draw_panes(&self, frame: &mut ratatui::Frame, main: Rect, cursor: bool) {
        let (panes, dividers) = self.pane_layout(main);
        let focused = self.view.as_ref().map(|v| v.id);
        let themed = self.settings.themed_sessions.then_some(self.settings.theme);
        let buf = frame.buffer_mut();
        for (id, r) in &panes {
            let Some(pane) = self.pane(*id) else { continue };
            crate::render_rows(pane.shown(), *r, buf, themed);
            if let Some(sel) = &pane.selection {
                crate::highlight(sel, *r, buf);
            }
        }
        let t = self.settings.theme;
        let lit = panes.iter().find(|p| Some(p.0) == focused).map(|p| p.1);
        for d in &dividers {
            let r = rect(d.area);
            let symbol = match d.axis {
                valkyrie_proto::Axis::Columns => "│",
                valkyrie_proto::Axis::Rows => "─",
            };
            for y in r.top()..r.bottom() {
                for x in r.left()..r.right() {
                    // Next to the focused pane: on its edge, within its span.
                    let near = lit.is_some_and(|f| {
                        let across = Rect::new(
                            f.x.saturating_sub(1),
                            f.y.saturating_sub(1),
                            f.width + 2,
                            f.height + 2,
                        );
                        across.contains(Position::new(x, y))
                    });
                    let color = if near && self.layout.is_some() {
                        t.accent
                    } else {
                        t.border
                    };
                    buf[(x, y)]
                        .set_symbol(symbol)
                        .set_style(style::Style::new().fg(color).bg(self.page_bg()));
                }
            }
        }
        if let Some(view) = &self.view
            && let Some((x, y)) = view.cursor.filter(|_| view.scroll.is_none())
            && let Some((_, r)) = panes.iter().find(|p| p.0 == view.id)
            && x < r.width
            && y < r.height
            && cursor
        {
            frame.set_cursor_position(Position::new(r.x + x, r.y + y));
        }
    }
}

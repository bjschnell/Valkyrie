//! A backend that writes nothing when a frame changes nothing.
//!
//! ratatui ends every frame with style resets and the cursor's visibility and
//! position, even when no cell changed, so a TUI redrawn once a second is never
//! quiet. Over SSH that matters: a write the far end can't acknowledge (a laptop
//! asleep) starts TCP's retransmit backoff, and on wake the output then waits for the
//! next retry, up to two minutes later. With this, an unchanged screen sends nothing.

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

pub struct Quiet<B> {
    inner: B,
    /// What the terminal was last told; `None` when unknown.
    visible: Option<bool>,
    position: Option<Position>,
}

impl<B> Quiet<B> {
    pub fn new(inner: B) -> Self {
        Self {
            inner,
            visible: None,
            position: None,
        }
    }

    /// Something ratatui doesn't see may have moved or shown the cursor (a resize
    /// the emulator clamped it in, the outer terminal's own clear): say it again.
    pub fn forget(&mut self) {
        self.visible = None;
        self.position = None;
    }
}

impl<B: Backend> Backend for Quiet<B> {
    type Error = B::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut content = content.peekable();
        if content.peek().is_none() {
            return Ok(());
        }
        // Writing cells moves the cursor.
        self.position = None;
        self.inner.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.position = None;
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        if self.visible == Some(false) {
            return Ok(());
        }
        self.visible = None;
        self.inner.hide_cursor()?;
        self.visible = Some(false);
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        if self.visible == Some(true) {
            return Ok(());
        }
        self.visible = None;
        self.inner.show_cursor()?;
        self.visible = Some(true);
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        let position = position.into();
        if self.position == Some(position) {
            return Ok(());
        }
        self.position = None;
        self.inner.set_cursor_position(position)?;
        self.position = Some(position);
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.position = None;
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.position = None;
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;
    use ratatui::widgets::Paragraph;
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    /// Counts what reaches the "terminal".
    #[derive(Clone, Default)]
    struct Wire(Rc<RefCell<Vec<u8>>>);

    impl Write for Wire {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn an_unchanged_frame_writes_nothing() {
        let wire = Wire::default();
        let backend = Quiet::new(CrosstermBackend::new(wire.clone()));
        let mut terminal = Terminal::with_options(
            backend,
            ratatui::TerminalOptions {
                viewport: ratatui::Viewport::Fixed(ratatui::layout::Rect::new(0, 0, 20, 2)),
            },
        )
        .unwrap();
        let mut frame = |text: &'static str, cursor: Option<(u16, u16)>| {
            terminal
                .draw(|f| {
                    f.render_widget(Paragraph::new(text), f.area());
                    if let Some(c) = cursor {
                        f.set_cursor_position(c);
                    }
                })
                .unwrap();
            std::mem::take(&mut *wire.0.borrow_mut())
        };
        assert!(!frame("hello", Some((1, 0))).is_empty());
        assert!(frame("hello", Some((1, 0))).is_empty(), "same frame");
        let moved = String::from_utf8(frame("hello", Some((2, 0)))).unwrap();
        assert_eq!(moved, "\x1b[1;3H", "only the cursor moved");
        let changed = String::from_utf8(frame("help!", Some((2, 0)))).unwrap();
        assert!(changed.contains("p!"));
        assert!(
            changed.ends_with("\x1b[1;3H"),
            "cursor put back after the cells"
        );
        let hidden = String::from_utf8(frame("help!", None)).unwrap();
        assert_eq!(hidden, "\x1b[?25l");
        assert!(frame("help!", None).is_empty());
    }
}

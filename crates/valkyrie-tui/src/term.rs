//! The outer terminal, for the escapes Valkyrie writes around ratatui: mouse
//! capture, mirrored input modes, images, the clipboard, the bell.
//!
//! Tests get a buffer of their own thread instead, so a test that attaches
//! doesn't flip the modes of the terminal running `cargo test`, and can read
//! back what it would have written (`take_written`).

/// Where those escapes go: stdout.
#[cfg(not(test))]
pub fn outer() -> std::io::Stdout {
    std::io::stdout()
}

#[cfg(test)]
pub fn outer() -> Written {
    Written
}

#[cfg(test)]
thread_local! {
    static WRITTEN: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// This thread's stand-in for the terminal.
#[cfg(test)]
pub struct Written;

#[cfg(test)]
impl std::io::Write for Written {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        WRITTEN.with(|w| w.borrow_mut().extend_from_slice(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// What this thread wrote to the terminal since the last call.
#[cfg(test)]
pub fn take_written() -> String {
    WRITTEN.with(|w| String::from_utf8_lossy(&std::mem::take(&mut *w.borrow_mut())).into_owned())
}

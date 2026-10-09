//! The terminal a session's program runs in, per platform. On unix the PTY master
//! is a plain fd, so it can cross an upgrade exec (ADR-0006); on Windows it is a
//! ConPTY, and the program and everything it starts share a job object, the
//! nearest thing to a process group.

use valkyrie_proto::Size;

/// The size portable-pty wants: `cell` in pixels fills the pixel fields, which
/// image programs read.
pub fn pty_size(size: Size, cell: (u16, u16)) -> portable_pty::PtySize {
    portable_pty::PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: size.cols.saturating_mul(cell.0),
        pixel_height: size.rows.saturating_mul(cell.1),
    }
}

#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
pub use windows::*;

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
    use valkyrie_proto::Size;

    /// Readers poll this alongside their PTY and return, before reading another
    /// byte, once it is set: unread output stays in the kernel for the next daemon
    /// image.
    pub struct StopPipe {
        pub read: OwnedFd,
        write: OwnedFd,
    }

    impl StopPipe {
        pub fn new() -> std::io::Result<Self> {
            // Close-on-exec, on every platform.
            let (read, write) = std::io::pipe()?;
            Ok(Self {
                read: read.into(),
                write: write.into(),
            })
        }

        /// Level-triggered and never drained, so every poller sees it, now and later.
        pub fn set(&self) {
            // SAFETY: writes one byte from a valid buffer to our own pipe.
            unsafe { libc::write(self.write.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        }
    }

    /// The PTY master, held as a plain fd so it can survive an exec (ADR-0006).
    pub struct Pty(pub OwnedFd);

    impl Pty {
        /// Takes the master's fd; `pair` (and portable-pty's own copy) may drop after.
        pub fn new(
            master: Box<dyn portable_pty::MasterPty + Send>,
            _child: &dyn portable_pty::Child,
        ) -> anyhow::Result<Pty> {
            use anyhow::Context;
            let fd = master.as_raw_fd().context("pty master has no fd")?;
            // SAFETY: the master is open until `master` drops, after this dup.
            let pty = Pty(unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()?);
            drop(master);
            Ok(pty)
        }

        /// `cell` in pixels fills the winsize pixel fields, which image programs read.
        pub fn resize(&self, size: Size, cell: (u16, u16)) -> std::io::Result<()> {
            let ws = libc::winsize {
                ws_row: size.rows,
                ws_col: size.cols,
                ws_xpixel: size.cols.saturating_mul(cell.0),
                ws_ypixel: size.rows.saturating_mul(cell.1),
            };
            // SAFETY: TIOCSWINSZ reads a winsize from a valid pointer.
            if unsafe { libc::ioctl(self.0.as_raw_fd(), libc::TIOCSWINSZ, &ws) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        /// The foreground process group, as the shell's job control set it.
        pub fn foreground(&self) -> Option<i32> {
            // SAFETY: tcgetpgrp only reads the terminal's state.
            let group = unsafe { libc::tcgetpgrp(self.0.as_raw_fd()) };
            (group > 0).then_some(group)
        }

        /// An independent handle for a reader or writer thread.
        pub fn file(&self) -> std::io::Result<File> {
            Ok(File::from(self.0.try_clone()?))
        }

        pub fn fd(&self) -> RawFd {
            self.0.as_raw_fd()
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::io::{Read, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::sync::Mutex;
    use valkyrie_proto::Size;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    /// CreateProcess cannot execute npm's .cmd launchers. Run the known package
    /// entry point with node directly, preserving argument boundaries (including
    /// Claude's JSON settings) without passing them through cmd.exe.
    pub fn resolve_agent_shim(
        command: &mut portable_pty::CommandBuilder,
        agent: &str,
    ) -> anyhow::Result<()> {
        use std::path::{Path, PathBuf};
        let program = Path::new(&command.get_argv()[0]);
        let paths = command.get_env("PATH").map(std::env::split_paths);
        let extensions = command
            .get_env("PATHEXT")
            .and_then(|s| s.to_str())
            .unwrap_or(".COM;.EXE;.BAT;.CMD");
        let candidates = if program.components().count() > 1 || program.is_absolute() {
            vec![program.to_path_buf()]
        } else {
            paths
                .into_iter()
                .flatten()
                .map(|dir| dir.join(program))
                .collect::<Vec<_>>()
        };
        let resolved = candidates.into_iter().find_map(|base| {
            if base.is_file() {
                return Some(base);
            }
            if base.extension().is_some() {
                return None;
            }
            extensions
                .split(';')
                .filter_map(|ext| ext.strip_prefix('.'))
                .map(|ext| base.with_extension(ext))
                .find(|p| p.is_file())
        });
        let Some(program) = resolved else {
            return Ok(());
        };
        if !program
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
        {
            // Use the resolved path: portable-pty replaces existing extensions
            // while searching, which can choose a different program.
            command.get_argv_mut()[0] = program.into_os_string();
            return Ok(());
        }
        let entry = match agent {
            "codex" => r"node_modules\@openai\codex\bin\codex.js",
            "claude" => r"node_modules\@anthropic-ai\claude-code\cli.js",
            _ => anyhow::bail!(
                "start {} from a shell tab; ConPTY needs an executable",
                program.display()
            ),
        };
        let dir = program.parent().unwrap_or(Path::new("."));
        let script = dir.join(entry);
        let shim = std::fs::read_to_string(&program)?;
        anyhow::ensure!(
            script.is_file() && shim.contains(entry),
            "{} is not a recognized npm agent launcher; start it from a shell tab",
            program.display()
        );
        let node = dir.join("node.exe");
        let node = if node.is_file() {
            node
        } else {
            PathBuf::from("node.exe")
        };
        command
            .get_argv_mut()
            .splice(0..1, [node.into_os_string(), script.into_os_string()]);
        Ok(())
    }

    /// Upgrades restart the daemon on Windows instead of handing off (no exec), so
    /// there is nothing to stop.
    pub struct StopPipe;

    impl StopPipe {
        pub fn new() -> std::io::Result<Self> {
            Ok(StopPipe)
        }
    }

    pub struct Pty {
        master: Mutex<Option<Box<dyn portable_pty::MasterPty + Send>>>,
        /// portable-pty hands the writer out once.
        writer: Mutex<Option<Box<dyn Write + Send>>>,
        /// The session's program and everything it starts. Killing the session
        /// ends the job; so does the daemon exiting, as a PTY's hangup would.
        job: OwnedHandle,
        pid: Option<u32>,
    }

    impl Pty {
        pub fn new(
            master: Box<dyn portable_pty::MasterPty + Send>,
            child: &dyn portable_pty::Child,
        ) -> anyhow::Result<Pty> {
            use anyhow::Context;
            let writer = master.take_writer()?;
            let process = child
                .as_raw_handle()
                .context("session has no process handle")?;
            let job = job_for(process).context("assign session to a job object")?;
            Ok(Pty {
                master: Mutex::new(Some(master)),
                writer: Mutex::new(Some(writer)),
                job,
                pid: child.process_id(),
            })
        }

        pub fn resize(&self, size: Size, cell: (u16, u16)) -> std::io::Result<()> {
            self.master
                .lock()
                .unwrap()
                .as_ref()
                .ok_or_else(|| std::io::Error::other("pty closed"))?
                .resize(super::pty_size(size, cell))
                .map_err(std::io::Error::other)
        }

        /// No process groups here: the program itself, whose descendants are
        /// searched for an agent (`foreground::agent`).
        pub fn foreground(&self) -> Option<i32> {
            self.pid.map(|pid| pid as i32)
        }

        pub fn reader(&self) -> std::io::Result<Box<dyn Read + Send>> {
            self.master
                .lock()
                .unwrap()
                .as_ref()
                .ok_or_else(|| std::io::Error::other("pty closed"))?
                .try_clone_reader()
                .map_err(std::io::Error::other)
        }

        pub fn writer(&self) -> std::io::Result<Box<dyn Write + Send>> {
            self.writer
                .lock()
                .unwrap()
                .take()
                .ok_or_else(|| std::io::Error::other("pty writer already taken"))
        }

        /// Ends the program and everything it started. Returns whether it could.
        pub fn kill(&self) -> bool {
            // SAFETY: a job handle we own.
            unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) != 0 }
        }

        /// Close the host so the reader gets EOF. It holds a session reference,
        /// so waiting for Session::drop would otherwise keep both alive forever.
        /// Drop outside the mutex: ClosePseudoConsole may wait for output to drain.
        pub fn close(&self) {
            let master = self.master.lock().unwrap().take();
            drop(master);
        }
    }

    /// A job holding `process` (and, from now on, whatever it starts) that ends
    /// with its last handle, ours.
    fn job_for(process: std::os::windows::io::RawHandle) -> std::io::Result<OwnedHandle> {
        // SAFETY: plain calls on handles we own or were given; the job handle is
        // owned from here on.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let job = OwnedHandle::from_raw_handle(job);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if AssignProcessToJobObject(job.as_raw_handle(), process) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(job)
        }
    }
}

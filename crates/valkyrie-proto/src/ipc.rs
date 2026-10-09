//! The daemon's socket. A unix socket on Linux and macOS; on Windows a named pipe,
//! named after the socket path, so `--socket` and `VALK_SOCKET` mean the same on
//! every platform.

use std::io;
use std::path::Path;
use std::time::Duration;

#[cfg(unix)]
pub use unix::*;
#[cfg(windows)]
pub use windows::*;

/// Sends `frame` and, with `reply`, reads one frame back, giving up after
/// `timeout`. For hooks, which must never hold their agent up.
pub fn exchange(path: &Path, frame: &[u8], reply: bool, timeout: Duration) -> io::Result<Vec<u8>> {
    #[cfg(unix)]
    {
        exchange_now(path, frame, reply, timeout)
    }
    // A pipe opened as a file has no timeouts: run it aside and stop waiting.
    #[cfg(windows)]
    {
        let (path, frame) = (path.to_owned(), frame.to_vec());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(exchange_now(&path, &frame, reply, timeout));
        });
        rx.recv_timeout(timeout)
            .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
    }
}

fn exchange_now(path: &Path, frame: &[u8], reply: bool, timeout: Duration) -> io::Result<Vec<u8>> {
    use std::io::{Read, Write};
    let mut stream = connect_blocking(path, timeout)?;
    stream.write_all(frame)?;
    if !reply {
        return Ok(Vec::new());
    }
    let mut len = [0u8; 4];
    stream.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > 1 << 20 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "reply too large",
        ));
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body)?;
    Ok(body)
}

#[cfg(unix)]
mod unix {
    use std::io;
    use std::path::Path;
    use std::time::Duration;

    pub type Stream = tokio::net::UnixStream;
    pub type Listener = tokio::net::UnixListener;

    pub async fn connect(path: &Path) -> io::Result<Stream> {
        Stream::connect(path).await
    }

    pub(super) fn connect_blocking(
        path: &Path,
        timeout: Duration,
    ) -> io::Result<std::os::unix::net::UnixStream> {
        let stream = std::os::unix::net::UnixStream::connect(path)?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_read_timeout(Some(timeout))?;
        Ok(stream)
    }

    /// The process at the other end.
    pub fn peer_pid(stream: &Stream) -> Option<i32> {
        stream.peer_cred().ok()?.pid()
    }
}

#[cfg(windows)]
mod windows {
    use std::ffi::OsStr;
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::sync::OnceLock;
    use std::time::{Duration, Instant};
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_PIPE_BUSY, HANDLE, HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::Pipes::{
        GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// Either end of a connection: the daemon's accepted ones are servers.
    pub enum Stream {
        Server(NamedPipeServer),
        Client(NamedPipeClient),
    }

    /// Accepts connections on one pipe name: each client takes the instance that
    /// was waiting, and a new one waits for the next.
    pub struct Listener {
        name: String,
        next: NamedPipeServer,
    }

    impl Listener {
        /// Fails if any process, this user's or another's, already has the name.
        pub fn bind(path: &Path) -> io::Result<Listener> {
            let name = pipe_name(path);
            let next = create(&name, true)?;
            Ok(Listener { name, next })
        }

        pub async fn accept(&mut self) -> io::Result<(Stream, ())> {
            self.next.connect().await?;
            let fresh = create(&self.name, false)?;
            let accepted = std::mem::replace(&mut self.next, fresh);
            Ok((Stream::Server(accepted), ()))
        }
    }

    fn create(name: &str, first: bool) -> io::Result<NamedPipeServer> {
        let mut attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: user_only()?,
            bInheritHandle: 0,
        };
        // SAFETY: `attrs` is a valid SECURITY_ATTRIBUTES whose descriptor lives for
        // the whole process.
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(name, (&raw mut attrs).cast())
        }
    }

    /// `\\.\pipe\valkyrie-<hash of the path>`: the path is under the user's own
    /// profile, so each user (and each `--socket`) gets its own name.
    pub fn pipe_name(path: &Path) -> String {
        let path = path.to_string_lossy().to_lowercase();
        // FNV-1a: stable across builds, unlike the std hasher.
        let hash = path.bytes().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ b as u64).wrapping_mul(0x100000001b3)
        });
        format!(r"\\.\pipe\valkyrie-{hash:016x}")
    }

    pub async fn connect(path: &Path) -> io::Result<Stream> {
        let name = pipe_name(path);
        let deadline = Instant::now() + Duration::from_secs(2);
        let client = loop {
            match ClientOptions::new().open(&name) {
                Ok(client) => break client,
                // Every instance taken for a moment: the daemon makes another.
                Err(e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                        && Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(e) => return Err(e),
            }
        };
        check_server(client.as_raw_handle())?;
        Ok(Stream::Client(client))
    }

    pub(super) fn connect_blocking(path: &Path, timeout: Duration) -> io::Result<std::fs::File> {
        let name = pipe_name(path);
        let deadline = Instant::now() + timeout;
        let file = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(OsStr::new(&name))
            {
                Ok(file) => break file,
                Err(e)
                    if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(e),
            }
        };
        check_server(file.as_raw_handle())?;
        Ok(file)
    }

    /// The process at the other end.
    pub fn peer_pid(stream: &Stream) -> Option<i32> {
        let Stream::Server(server) = stream else {
            return None;
        };
        let mut pid = 0u32;
        // SAFETY: a valid pipe handle and out pointer.
        let ok = unsafe { GetNamedPipeClientProcessId(server.as_raw_handle(), &mut pid) };
        (ok != 0 && pid != 0).then_some(pid as i32)
    }

    /// Refuses a pipe another user's process serves: whoever holds the name gets
    /// our keystrokes (the unix socket's private directory guards against this).
    fn check_server(pipe: std::os::windows::io::RawHandle) -> io::Result<()> {
        let mut pid = 0u32;
        // SAFETY: a valid pipe handle and out pointer.
        if unsafe { GetNamedPipeServerProcessId(pipe, &mut pid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: plain call; the handle is closed below.
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return Err(io::Error::last_os_error());
        }
        let theirs = token_user(process);
        // SAFETY: we opened it.
        unsafe { CloseHandle(process) };
        let (theirs, ours) = (theirs?, token_user(current_process())?);
        // SAFETY: both buffers hold a TOKEN_USER whose SID points into them.
        let same = unsafe {
            let a = &*(theirs.as_ptr() as *const TOKEN_USER);
            let b = &*(ours.as_ptr() as *const TOKEN_USER);
            EqualSid(a.User.Sid, b.User.Sid) != 0
        };
        if !same {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the daemon's pipe belongs to another user",
            ));
        }
        Ok(())
    }

    fn current_process() -> HANDLE {
        // SAFETY: a pseudo handle; never fails.
        unsafe { GetCurrentProcess() }
    }

    /// `process`'s TOKEN_USER, in a buffer aligned for it.
    fn token_user(process: HANDLE) -> io::Result<Vec<u64>> {
        let mut token: HANDLE = std::ptr::null_mut();
        // SAFETY: a valid process handle and out pointer.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = vec![0u64; 64];
        let mut len = 0u32;
        // SAFETY: the buffer holds the size given.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                &mut len,
            )
        };
        let error = io::Error::last_os_error();
        // SAFETY: we opened it.
        unsafe { CloseHandle(token) };
        if ok == 0 {
            return Err(error);
        }
        Ok(buf)
    }

    /// A security descriptor that lets only this user open the pipe (the default
    /// one lets everyone read it). Made once and kept for the process.
    fn user_only() -> io::Result<PSECURITY_DESCRIPTOR> {
        static DESCRIPTOR: OnceLock<usize> = OnceLock::new();
        if let Some(&sd) = DESCRIPTOR.get() {
            return Ok(sd as PSECURITY_DESCRIPTOR);
        }
        let user = token_user(current_process())?;
        // SAFETY: `user` holds a TOKEN_USER; the string is freed below.
        let sid = unsafe {
            let user = &*(user.as_ptr() as *const TOKEN_USER);
            let mut text = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
                return Err(io::Error::last_os_error());
            }
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text as HLOCAL);
            sid
        };
        // Protected (no inherited entries), full access for this user alone.
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain([0])
            .collect();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        // SAFETY: a NUL-terminated string and an out pointer; never freed.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(*DESCRIPTOR.get_or_init(|| sd as usize) as PSECURITY_DESCRIPTOR)
    }

    macro_rules! both {
        ($self:ident, $s:ident => $e:expr) => {
            match $self.get_mut() {
                Stream::Server($s) => $e,
                Stream::Client($s) => $e,
            }
        };
    }

    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    impl AsyncRead for Stream {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            both!(self, s => Pin::new(s).poll_read(cx, buf))
        }
    }

    impl AsyncWrite for Stream {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            both!(self, s => Pin::new(s).poll_write(cx, buf))
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            both!(self, s => Pin::new(s).poll_flush(cx))
        }

        fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            both!(self, s => Pin::new(s).poll_shutdown(cx))
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn socket() -> std::path::PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        std::env::temp_dir().join(format!(
            "valkyrie-pipe-test-{}-{}.sock",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[tokio::test]
    async fn blocking_exchange_and_peer_identity() {
        let path = socket();
        let mut listener = Listener::bind(&path).unwrap();
        assert!(Listener::bind(&path).is_err());
        let task = tokio::task::spawn_blocking(move || {
            exchange(&path, b"request", true, Duration::from_secs(2))
        });
        let (mut stream, _) = listener.accept().await.unwrap();
        assert_eq!(peer_pid(&stream), Some(std::process::id() as i32));
        let mut request = [0; 7];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"request");
        stream.write_all(b"\0\0\0\x05reply").await.unwrap();
        assert_eq!(task.await.unwrap().unwrap(), b"reply");
    }

    #[tokio::test]
    async fn a_hook_does_not_wait_forever_for_a_reply() {
        let path = socket();
        let mut listener = Listener::bind(&path).unwrap();
        let task = tokio::task::spawn_blocking(move || {
            exchange(&path, b"request", true, Duration::from_millis(100))
        });
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 7];
        stream.read_exact(&mut request).await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        // Dropping the server releases the blocking read's worker as well.
        drop(stream);
    }
}

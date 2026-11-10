//! Proof that what answers on the local port is this user's `valk web`, not
//! whatever took the port first (ADR-0020 §4). The asker writes a random nonce to
//! a file only the user can read, under the web state directory, and asks the
//! server for the hash of it: only a process that can read the file can answer.
//! Such a process could write `devices.json` anyway, so nothing is given away.
//!
//! The desktop app asks before it loads the address and pairs itself with it.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// How long a nonce is answered for: a check takes a moment, and a file left by an
/// asker that died shouldn't be answerable forever.
const NONCE_TTL: Duration = Duration::from_secs(60);
/// How long the asker waits on the port.
const ASK_TIMEOUT: Duration = Duration::from_secs(3);
/// More than any answer; a stranger on the port may send anything.
const MAX_REPLY: u64 = 16 * 1024;

/// Where nonces are written: beside `devices.json`.
pub fn dir() -> PathBuf {
    valkyrie_proto::state_dir().join("web").join("proof")
}

fn random_hex(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| anyhow::anyhow!("random: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// What a server that read `nonce` answers for `id`.
fn answer(id: &str, nonce: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(b"valk-web-proof\n");
    hash.update(id.as_bytes());
    hash.update(b"\n");
    hash.update(nonce.as_bytes());
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A nonce's name: 32 hex digits, so a request can't name any other file.
fn is_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The server's side: the answer for the nonce called `id` in `dir`, if there is a
/// fresh one.
pub fn respond(dir: &Path, id: &str) -> Option<String> {
    if !is_id(id) {
        return None;
    }
    let file = dir.join(id);
    // A plain file, not a link to something else worth hashing.
    let meta = std::fs::symlink_metadata(&file).ok()?;
    if !meta.is_file() || meta.len() > 256 {
        return None;
    }
    let age = meta.modified().ok()?.elapsed().unwrap_or_default();
    if age > NONCE_TTL {
        return None;
    }
    let nonce = std::fs::read_to_string(&file).ok()?;
    Some(answer(id, nonce.trim()))
}

/// A nonce on disk, removed when the check is over.
struct Challenge {
    id: String,
    nonce: String,
    file: PathBuf,
}

impl Challenge {
    fn new(dir: &Path) -> Result<Self> {
        valkyrie_proto::ensure_private_dir(dir)?;
        let id = random_hex(16)?;
        let nonce = random_hex(32)?;
        let file = dir.join(&id);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut out = options
            .open(&file)
            .with_context(|| format!("write {}", file.display()))?;
        std::io::Write::write_all(&mut out, nonce.as_bytes())?;
        Ok(Self { id, nonce, file })
    }
}

impl Drop for Challenge {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
    }
}

/// What a check of the local port found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Local {
    /// This user's `valk web`: it read the nonce.
    Ours,
    /// Something answers, and it couldn't prove it's this user's `valk web`:
    /// another program, another user's server, or a `valk web` from before this
    /// check existed.
    Stranger,
    /// Nothing listens there.
    Nothing,
}

/// Asks whatever listens on `addr` to prove it's this user's `valk web`.
pub async fn check(addr: SocketAddr) -> Result<Local> {
    check_in(&dir(), addr).await
}

async fn check_in(dir: &Path, addr: SocketAddr) -> Result<Local> {
    let challenge = Challenge::new(dir)?;
    let request = format!(
        "GET /api/proof/{} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n",
        challenge.id
    );
    let asked = tokio::time::timeout(ASK_TIMEOUT, async {
        let mut stream = match tokio::net::TcpStream::connect(addr).await {
            Ok(stream) => stream,
            Err(_) => return None,
        };
        let mut reply = Vec::new();
        // A stranger may hang up, stall or send rubbish: all the same answer.
        let _ = stream.write_all(request.as_bytes()).await;
        let _ = stream.take(MAX_REPLY).read_to_end(&mut reply).await;
        Some(reply)
    })
    .await;
    let reply = match asked {
        Ok(None) => return Ok(Local::Nothing),
        Ok(Some(reply)) => reply,
        // It took the connection and never answered.
        Err(_) => return Ok(Local::Stranger),
    };
    let reply = String::from_utf8_lossy(&reply);
    let ok = reply.starts_with("HTTP/1.1 200")
        && reply
            .split_once("\r\n\r\n")
            .is_some_and(|(_, body)| body.trim() == answer(&challenge.id, &challenge.nonce));
    Ok(if ok { Local::Ours } else { Local::Stranger })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("valk-proof-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn answers_only_for_a_fresh_nonce_by_its_name() {
        let dir = scratch("respond");
        let challenge = Challenge::new(&dir).unwrap();
        assert_eq!(
            respond(&dir, &challenge.id),
            Some(answer(&challenge.id, &challenge.nonce))
        );
        // Not a nonce's name: nothing else in the directory, or outside it, is read.
        std::fs::write(dir.join("notes"), "x").unwrap();
        assert_eq!(respond(&dir, "notes"), None);
        assert_eq!(respond(&dir, "../devices.json"), None);
        assert_eq!(respond(&dir, &"0".repeat(32)), None);
        // Gone once the check is over.
        let id = challenge.id.clone();
        drop(challenge);
        assert_eq!(respond(&dir, &id), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_nonce_is_the_users_alone_and_links_are_not_followed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("private");
        let challenge = Challenge::new(&dir).unwrap();
        let mode = std::fs::metadata(&challenge.file)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let link = "a".repeat(32);
        std::os::unix::fs::symlink(&challenge.file, dir.join(&link)).unwrap();
        assert_eq!(respond(&dir, &link), None);
        drop(challenge);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Serves `reply` to one connection on a local port.
    async fn serve_once(reply: impl Fn(&str) -> String + Send + 'static) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = stream.read(&mut buf).await.unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            let _ = stream.write_all(reply(&request).as_bytes()).await;
        });
        addr
    }

    #[tokio::test]
    async fn a_server_that_reads_the_nonce_is_ours() {
        let dir = scratch("ours");
        let served = dir.clone();
        let addr = serve_once(move |request| {
            let id = request
                .strip_prefix("GET /api/proof/")
                .and_then(|rest| rest.split(' ').next())
                .unwrap();
            let body = respond(&served, id).unwrap();
            format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            )
        })
        .await;
        assert_eq!(check_in(&dir, addr).await.unwrap(), Local::Ours);
        // The nonce doesn't outlive the check.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn anything_else_on_the_port_is_a_stranger() {
        let dir = scratch("stranger");
        // An older valk web serves the app for any path it doesn't know.
        let addr =
            serve_once(|_| "HTTP/1.1 200 OK\r\ncontent-length: 15\r\n\r\n<!doctype html>".into())
                .await;
        assert_eq!(check_in(&dir, addr).await.unwrap(), Local::Stranger);
        let addr = serve_once(|_| "HTTP/1.1 404 Not Found\r\n\r\n".into()).await;
        assert_eq!(check_in(&dir, addr).await.unwrap(), Local::Stranger);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_closed_port_is_nothing() {
        let dir = scratch("nothing");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        assert_eq!(check_in(&dir, addr).await.unwrap(), Local::Nothing);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

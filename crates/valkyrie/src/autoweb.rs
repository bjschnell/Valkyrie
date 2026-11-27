//! The web app started with the daemon (backlog #316): the daemon serving this
//! machine keeps a `valk web` running, bound to localhost, while the
//! `web_server` setting is on (the default). `valk web server off`, the TUI's
//! settings panel or the web app's Settings turn it off. The server stops by
//! itself once the setting goes off (`valkyrie_web::Options::managed`); this side
//! only starts one when nothing answers on the address.
//!
//! It stands aside for a `valk setup web` service (phones over the tailnet, the
//! desktop app), which owns the server then, and for anything else on the port.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use valkyrie_proto::NoWindow as _;

/// The setting, and the environment variable that overrides it for one run.
pub const KEY: &str = "web_server";
pub const ENV: &str = "VALK_WEB_SERVER";

/// Where it listens unless `web_listen` in `settings.toml` names another port on
/// localhost: the same address `valk web` and `valk setup web` default to.
pub const LISTEN: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 8790);

/// How often the daemon looks whether one should be started.
const EVERY: Duration = Duration::from_secs(3);

pub fn on() -> bool {
    valkyrie_proto::setting_on(ENV, KEY)
}

/// The address: `web_listen` when it's on this machine only, else [`LISTEN`].
/// Anything beyond localhost is what `valk setup web` is for, with its warning.
pub fn listen() -> SocketAddr {
    parse_listen(valkyrie_proto::setting("web_listen").as_deref())
}

fn parse_listen(value: Option<&str>) -> SocketAddr {
    match value.and_then(|v| v.parse::<SocketAddr>().ok()) {
        Some(addr) if addr.ip().is_loopback() => addr,
        Some(addr) => {
            tracing::warn!("web_listen = {addr} isn't on localhost; using {LISTEN}");
            LISTEN
        }
        None => LISTEN,
    }
}

/// Whether the daemon on `socket` should keep a web server: only the one serving
/// this machine (not `--dev`, not a test's or a verify run's on a socket of its
/// own), and only while no service has been set up to do it.
pub fn applies(socket: &Path, dev: bool) -> bool {
    !dev && socket == valkyrie_proto::default_socket_path() && !crate::service::installed()
}

/// Runs for as long as the daemon does: starts `exe web` whenever the setting is
/// on and nothing listens on the address, and reaps it when it stops.
pub async fn keep(exe: PathBuf, socket: PathBuf) {
    let mut child: Option<tokio::process::Child> = None;
    loop {
        // First, so the daemon is listening before its server connects to it.
        tokio::time::sleep(EVERY).await;
        if let Some(c) = &mut child
            && !matches!(c.try_wait(), Ok(None))
        {
            child = None;
        }
        if child.is_none() && on() {
            let addr = listen();
            // Free right now: nothing else (a `valk web` in a terminal) has it.
            if std::net::TcpListener::bind(addr).is_ok() {
                match start(&exe, &socket, addr) {
                    Ok(c) => {
                        tracing::info!("started valk web on {addr}");
                        child = Some(c);
                    }
                    Err(e) => tracing::warn!("couldn't start valk web: {e:#}"),
                }
            }
        }
    }
}

fn start(exe: &Path, socket: &Path, addr: SocketAddr) -> anyhow::Result<tokio::process::Child> {
    // The web app's own directory, private as its store requires.
    let dir = valkyrie_proto::state_dir().join("web");
    valkyrie_proto::ensure_private_dir(&dir)?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("server.log"))?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("--socket")
        .arg(socket)
        .args(["web", "--managed", "--listen", &addr.to_string()])
        // As `valk setup web --local` does: nothing names the tailnet, so this
        // one is for this machine's browsers only.
        .args(["--url", &format!("http://{addr}")])
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        // Nothing the daemon's sessions set for themselves.
        .env_remove("VALK_SESSION")
        .no_window();
    Ok(cmd.spawn()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_listens_on_localhost_only() {
        assert_eq!(parse_listen(None), LISTEN);
        assert_eq!(parse_listen(Some("nonsense")), LISTEN);
        assert_eq!(
            parse_listen(Some("127.0.0.1:9000")),
            "127.0.0.1:9000".parse().unwrap()
        );
        assert_eq!(
            parse_listen(Some("[::1]:9000")),
            "[::1]:9000".parse().unwrap()
        );
        assert_eq!(parse_listen(Some("0.0.0.0:8790")), LISTEN);
        assert_eq!(parse_listen(Some("192.168.1.5:8790")), LISTEN);
    }
}

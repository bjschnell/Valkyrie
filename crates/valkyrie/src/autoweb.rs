//! The web app started with the daemon (backlog #316): the daemon serving this
//! machine keeps a `valk web` running, bound to localhost unless `web_listen`
//! names another address, while the `web_server` setting is on (the default).
//! `valk web server off`, the TUI's settings panel or the web app's Settings
//! turn it off. The server stops by
//! itself once the setting goes off (`valkyrie_web::Options::managed`); this side
//! only starts one when nothing answers on the address.
//!
//! It stands aside for a `valk setup web` service (phones over the tailnet, the
//! desktop app), which owns the server then, and for anything else on the port.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use valkyrie_daemon::web::{self, Report};
use valkyrie_proto::NoWindow as _;
use valkyrie_proto::WebRun;

/// The setting, and the environment variable that overrides it for one run.
pub const KEY: &str = "web_server";
pub const ENV: &str = "VALK_WEB_SERVER";

/// Where it listens unless `web_listen` in `settings.toml` names another address:
/// the same one `valk web` and `valk setup web` default to.
pub const LISTEN: SocketAddr = valkyrie_proto::WEB_LISTEN;

/// How often the daemon looks whether one should be started.
const EVERY: Duration = Duration::from_secs(3);

pub fn on() -> bool {
    valkyrie_proto::setting_on(ENV, KEY)
}

/// Where the server listens and the address phones open, from `settings.toml`.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    /// `web_listen`: localhost by default; a tailnet IP or `0.0.0.0` puts it
    /// beyond this machine, which `valk web` warns about in its log.
    pub listen: SocketAddr,
    /// `web_url`: the name phones open (a proxy, a tunnel, a MagicDNS alias).
    /// None leaves it to `valk web`: the tailnet name, else the listen address.
    pub url: Option<String>,
}

impl Place {
    pub fn now() -> Place {
        Place::from(
            valkyrie_proto::setting(web::LISTEN).as_deref(),
            valkyrie_proto::setting(web::URL).as_deref(),
        )
    }

    fn from(listen: Option<&str>, url: Option<&str>) -> Place {
        let listen = match listen.map(str::trim).filter(|v| !v.is_empty()) {
            None => LISTEN,
            Some(v) => valkyrie_proto::parse_web_listen(v).unwrap_or_else(|| {
                tracing::warn!(
                    "web_listen = {v:?} isn't an address like 127.0.0.1:8790; using {LISTEN}"
                );
                LISTEN
            }),
        };
        let url = url
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string);
        Place { listen, url }
    }

    /// `valk web`'s arguments after `web`.
    fn args(&self) -> Vec<String> {
        let mut args = vec![
            "--managed".into(),
            "--listen".into(),
            self.listen.to_string(),
        ];
        if let Some(url) = &self.url {
            args.extend(["--url".into(), url.clone()]);
        }
        args
    }
}

/// Whether the daemon on `socket` should keep a web server: only the one serving
/// this machine (not `--dev`, not a test's or a verify run's on a socket of its
/// own). While a `valk setup web` service is set up, `keep` stands aside for it.
pub fn applies(socket: &Path, dev: bool) -> bool {
    !dev && socket == valkyrie_proto::default_socket_path()
}

/// How long the default phone URL (the tailnet name) is trusted before
/// `tailscale status` is asked again.
const DEFAULT_URL_EVERY: Duration = Duration::from_secs(60);

/// Runs for as long as the daemon does: starts `exe web` whenever the setting is
/// on and nothing listens on the address, restarts it when `web_listen` or
/// `web_url` changes, and reaps it when it stops. Says what it's doing to the
/// daemon's web server page (`valkyrie_daemon::web`) each time it looks.
pub async fn keep(exe: PathBuf, socket: PathBuf) {
    let mut child: Option<(tokio::process::Child, Place)> = None;
    let mut default_url: Option<(SocketAddr, String, Instant)> = None;
    loop {
        // First, so the daemon is listening before its server connects to it.
        tokio::time::sleep(EVERY).await;
        let place = Place::now();
        if let Some((c, started)) = &mut child {
            if !matches!(c.try_wait(), Ok(None)) {
                child = None;
            } else if *started != place {
                tracing::info!("web_listen or web_url changed; restarting valk web");
                let _ = c.kill().await;
                child = None;
            }
        }
        let run = if child.is_some() {
            WebRun::Running
        } else if crate::service::installed() {
            WebRun::Service
        } else if !on() {
            WebRun::Off
        } else if std::net::TcpListener::bind(place.listen).is_err() {
            // Something else has it: a `valk web` in a terminal, another program.
            WebRun::Taken
        } else {
            match start(&exe, &socket, &place) {
                Ok(c) => {
                    tracing::info!("started valk web on {}", place.listen);
                    child = Some((c, place.clone()));
                    WebRun::Running
                }
                Err(e) => {
                    tracing::warn!("couldn't start valk web: {e:#}");
                    WebRun::Taken
                }
            }
        };
        let fresh = default_url
            .as_ref()
            .is_some_and(|(at, _, when)| *at == place.listen && when.elapsed() < DEFAULT_URL_EVERY);
        if !fresh {
            let listen = place.listen;
            if let Ok(url) =
                tokio::task::spawn_blocking(move || valkyrie_web::public_url(None, listen)).await
            {
                default_url = Some((listen, url, Instant::now()));
            }
        }
        web::report(Report {
            run,
            listen: place.listen,
            url: place.url.clone(),
            default_url: default_url.as_ref().map(|(_, url, _)| url.clone()),
        });
    }
}

fn start(exe: &Path, socket: &Path, place: &Place) -> anyhow::Result<tokio::process::Child> {
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
        .arg("web")
        .args(place.args())
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
    fn it_listens_on_localhost_unless_told_otherwise() {
        let place = |l, u| Place::from(l, u);
        assert_eq!(place(None, None).listen, LISTEN);
        assert_eq!(place(Some(""), None).listen, LISTEN);
        assert_eq!(place(Some("nonsense"), None).listen, LISTEN);
        assert_eq!(
            place(Some("127.0.0.1:9000"), None).listen,
            "127.0.0.1:9000".parse().unwrap()
        );
        assert_eq!(
            place(Some("[::1]:9000"), None).listen,
            "[::1]:9000".parse().unwrap()
        );
        // A tailnet IP or every interface, as `valk web --listen` takes them.
        assert_eq!(
            place(Some("100.101.102.103:8790"), None).listen,
            "100.101.102.103:8790".parse().unwrap()
        );
        assert_eq!(
            place(Some("0.0.0.0:8790"), None).listen,
            "0.0.0.0:8790".parse().unwrap()
        );
    }

    #[test]
    fn web_url_names_the_address_phones_open() {
        let default = Place::from(None, None);
        assert_eq!(default.url, None);
        assert_eq!(default.args(), ["--managed", "--listen", "127.0.0.1:8790"]);
        let tailnet = Place::from(
            Some("100.101.102.103:9000"),
            Some(" https://thor.tail1.ts.net "),
        );
        assert_eq!(
            tailnet.args(),
            [
                "--managed",
                "--listen",
                "100.101.102.103:9000",
                "--url",
                "https://thor.tail1.ts.net"
            ]
        );
        assert_eq!(Place::from(None, Some("")).url, None);
    }
}

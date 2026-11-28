//! The daemon's web server page (backlog #321): `ClientMsg::WebServerConfig`
//! reads and writes `web_server`, `web_listen` and `web_url` in `settings.toml`,
//! and says what the server is doing. The server itself is kept by `valk
//! daemon` (`autoweb` in the `valkyrie` crate), which reports here with
//! [`report`] as it starts, stands aside or finds the port taken.

use crate::Registry;
use crate::context::Peer;
use anyhow::{Context, Result, bail};
use std::net::SocketAddr;
use std::sync::Mutex;
use valkyrie_proto::{Reply, WebRun, WebServerState};

/// The settings, and the environment variable that overrides `web_server`.
pub const ON: &str = "web_server";
pub const ON_ENV: &str = "VALK_WEB_SERVER";
pub const LISTEN: &str = "web_listen";
pub const URL: &str = "web_url";

/// What the keeper last said: what it's doing, for which address and URL, and
/// the URL phones open by default.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub run: WebRun,
    pub listen: SocketAddr,
    pub url: Option<String>,
    pub default_url: Option<String>,
}

static REPORTED: Mutex<Option<Report>> = Mutex::new(None);

/// The keeper says what the server is doing. None reported means this daemon
/// keeps no server (`--dev`, a socket of its own).
pub fn report(report: Report) {
    *REPORTED.lock().unwrap() = Some(report);
}

/// `web_listen` as the keeper reads it: the default when unset or not an
/// address with a port.
pub fn listen_now() -> SocketAddr {
    valkyrie_proto::setting(LISTEN)
        .as_deref()
        .and_then(valkyrie_proto::parse_web_listen)
        .unwrap_or(valkyrie_proto::WEB_LISTEN)
}

/// `web_url`, when one is set.
pub fn url_now() -> Option<String> {
    valkyrie_proto::setting(URL)
        .map(|u| u.trim().to_owned())
        .filter(|u| !u.is_empty())
}

/// A `web_url` the file can keep and a phone can open: `http(s)://` and no
/// spaces, quotes or `#` (which starts a comment in `settings.toml`).
fn check_url(url: &str) -> Result<()> {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        bail!("the phone URL starts with https:// or http://");
    };
    if rest.is_empty()
        || url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '\'' | '#'))
    {
        bail!("{url:?} isn't a URL a phone can open");
    }
    Ok(())
}

/// What the server is doing, from what's in the file now and what the keeper
/// last said.
fn state(reported: Option<Report>) -> WebServerState {
    decide(
        valkyrie_proto::setting_on(ON_ENV, ON),
        listen_now(),
        url_now(),
        reported,
    )
}

/// `state` once the file is read: a change the keeper hasn't acted on yet reads
/// as starting.
fn decide(
    on: bool,
    listen: SocketAddr,
    url: Option<String>,
    reported: Option<Report>,
) -> WebServerState {
    let run = match &reported {
        _ if !on => WebRun::Off,
        None => WebRun::Unmanaged,
        Some(r) if r.run == WebRun::Service => WebRun::Service,
        Some(r) if r.listen != listen || r.url != url => WebRun::Starting,
        Some(r) => r.run,
    };
    let default_url = reported
        .and_then(|r| r.default_url)
        .unwrap_or_else(|| format!("http://{listen}"));
    WebServerState {
        on,
        listen: listen.to_string(),
        url,
        default_url,
        run,
    }
}

impl Registry {
    /// `ClientMsg::WebServerConfig`.
    pub(crate) fn web_server_config(
        &self,
        on: Option<bool>,
        listen: Option<String>,
        url: Option<String>,
        peer: Option<Peer>,
    ) -> Result<Reply> {
        if on.is_some() || listen.is_some() || url.is_some() {
            self.caller(peer).vouch("changing the web server")?;
            let mut pairs: Vec<(&str, String)> = Vec::new();
            if let Some(on) = on {
                pairs.push((ON, if on { "on" } else { "off" }.into()));
            }
            if let Some(listen) = &listen {
                let addr = valkyrie_proto::parse_web_listen(listen).with_context(|| {
                    format!("{:?} isn't an address like 127.0.0.1:8790", listen.trim())
                })?;
                pairs.push((LISTEN, format!("\"{addr}\"")));
            }
            if let Some(url) = &url {
                let url = url.trim();
                if !url.is_empty() {
                    check_url(url)?;
                }
                pairs.push((URL, format!("\"{url}\"")));
            }
            let pairs: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (*k, v.as_str())).collect();
            valkyrie_proto::set_settings(&pairs)
                .context("couldn't save the settings in settings.toml")?;
        }
        let reported = REPORTED.lock().unwrap().clone();
        Ok(Reply::WebServerState {
            state: state(reported),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_the_keeper_has_not_seen_reads_as_starting() {
        let local = valkyrie_proto::WEB_LISTEN;
        let every: SocketAddr = "0.0.0.0:8790".parse().unwrap();
        let reported = |run, listen| {
            Some(Report {
                run,
                listen,
                url: None,
                default_url: Some("https://thor.tail1.ts.net".into()),
            })
        };
        let now = decide(true, local, None, reported(WebRun::Running, local));
        assert_eq!(now.run, WebRun::Running);
        assert_eq!(now.listen, "127.0.0.1:8790");
        assert_eq!(now.default_url, "https://thor.tail1.ts.net");
        // Just moved: starting until the keeper restarts it there.
        let moved = decide(true, every, None, reported(WebRun::Running, local));
        assert_eq!(moved.run, WebRun::Starting);
        let named = decide(
            true,
            local,
            Some("https://v.example".into()),
            reported(WebRun::Running, local),
        );
        assert_eq!(named.run, WebRun::Starting);
        // Off wins; a service stands in wherever it listens; no keeper, no server.
        assert_eq!(
            decide(false, local, None, reported(WebRun::Running, local)).run,
            WebRun::Off
        );
        assert_eq!(
            decide(true, every, None, reported(WebRun::Service, local)).run,
            WebRun::Service
        );
        assert_eq!(
            decide(true, local, None, reported(WebRun::Taken, local)).run,
            WebRun::Taken
        );
        let none = decide(true, local, None, None);
        assert_eq!(none.run, WebRun::Unmanaged);
        assert_eq!(none.default_url, "http://127.0.0.1:8790");
    }

    #[tokio::test]
    async fn only_the_user_changes_the_web_server() {
        use crate::context::{AS_CALLER, Caller};
        let dir = std::env::temp_dir().join(format!("valkyrie-webcfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (registry, _) = crate::new_registry(
            dir.join("v.sock"),
            &dir.join("state"),
            std::path::Path::new("/bin/true"),
            1,
            0,
            7,
            false,
        )
        .unwrap();
        AS_CALLER.with(|cell| {
            *cell.borrow_mut() = Some(Box::new(|| Caller {
                agent: Some("claude".into()),
                session: None,
                bridge: false,
                desktop: false,
            }))
        });
        // Refused before anything is written, so this machine's file is safe.
        for (on, listen, url) in [
            (Some(false), None, None),
            (None, Some("0.0.0.0:8790".to_string()), None),
            (None, None, Some(String::new())),
        ] {
            let e = registry
                .web_server_config(on, listen, url, None)
                .unwrap_err();
            assert!(format!("{e:#}").contains("is for the user"), "{e:#}");
        }
        // Asking is anyone's.
        assert!(matches!(
            registry.web_server_config(None, None, None, None),
            Ok(Reply::WebServerState { .. })
        ));
        AS_CALLER.with(|cell| *cell.borrow_mut() = None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_phone_url_is_one_the_file_keeps_and_a_phone_opens() {
        assert!(check_url("https://thor.tail1.ts.net").is_ok());
        assert!(check_url("http://192.168.1.4:8790/").is_ok());
        assert!(check_url("thor.tail1.ts.net").is_err());
        assert!(check_url("https://").is_err());
        assert!(check_url("https://a b").is_err());
        assert!(check_url("https://x/#y").is_err());
        assert!(check_url("https://x\"\nweb_server = off").is_err());
    }
}

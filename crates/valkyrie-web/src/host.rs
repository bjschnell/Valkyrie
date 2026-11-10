//! Which `Host` names the server answers to, against DNS rebinding.
//!
//! A page on a name its owner points at 127.0.0.1 is same-origin with this server
//! as far as the browser knows, so Origin matching Host proves nothing. The browser
//! always sets Host itself, so the defence is to answer only names the server is
//! known by: an IP address (a rebinding needs a name), `localhost` and
//! `*.localhost` (browsers resolve those to loopback themselves), the public
//! address given with `--url`, and this machine's tailnet name. `X-Forwarded-Host`
//! isn't consulted, since a page can set it; a proxy that rewrites Host sends
//! `127.0.0.1`, which is an IP address.

use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How often an unknown name may make the server ask Tailscale again: it may
/// have come up after `valk web` started.
const RECHECK: Duration = Duration::from_secs(60);

pub(crate) struct Allowed {
    /// From `--url`; fixed.
    given: Option<String>,
    tailnet: Mutex<Tailnet>,
}

struct Tailnet {
    name: Option<String>,
    checked: Option<Instant>,
}

impl Allowed {
    pub(crate) fn new(given: Option<&str>, tailnet: Option<&str>) -> Allowed {
        Allowed {
            given: given.and_then(url_host),
            tailnet: Mutex::new(Tailnet {
                name: tailnet.and_then(url_host),
                checked: Some(Instant::now()),
            }),
        }
    }

    /// Whether `host` (a Host header) names this server without asking anything.
    pub(crate) fn known(&self, host: &str) -> bool {
        let name = name(host);
        if name.parse::<IpAddr>().is_ok() || name == "localhost" || name.ends_with(".localhost") {
            return true;
        }
        self.given.as_deref() == Some(name.as_str())
            || self.tailnet.lock().unwrap().name.as_deref() == Some(name.as_str())
    }

    /// Whether an unknown name is due another look at Tailscale; marks it done.
    pub(crate) fn recheck_due(&self) -> bool {
        let mut t = self.tailnet.lock().unwrap();
        if t.checked.is_some_and(|at| at.elapsed() < RECHECK) {
            return false;
        }
        t.checked = Some(Instant::now());
        true
    }

    pub(crate) fn set_tailnet(&self, url: Option<&str>) {
        if let Some(name) = url.and_then(url_host) {
            self.tailnet.lock().unwrap().name = Some(name);
        }
    }
}

/// A Host header's name: no port, no IPv6 brackets, no trailing dot, lowercase.
fn name(host: &str) -> String {
    let host = host.trim();
    let bare = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => match host.rsplit_once(':') {
            // One colon is a port; more is a bare IPv6 address.
            Some((h, _)) if !h.contains(':') => h,
            _ => host,
        },
    };
    bare.trim_end_matches('.').to_ascii_lowercase()
}

/// The host name in a URL like `https://valk.example.com:8443/path`.
fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let name = name(authority);
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_lose_ports_brackets_and_case() {
        assert_eq!(name("Thor.Example.ts.net:443"), "thor.example.ts.net");
        assert_eq!(name("[::1]:8790"), "::1");
        assert_eq!(name("::1"), "::1");
        assert_eq!(name("valk.example.com."), "valk.example.com");
        assert_eq!(
            url_host("https://u@valk.example.com:8443/x?y").as_deref(),
            Some("valk.example.com")
        );
    }

    #[test]
    fn only_names_the_server_is_known_by() {
        let a = Allowed::new(
            Some("https://valk.example.com"),
            Some("https://thor.tail1.ts.net"),
        );
        for ok in [
            "127.0.0.1:8790",
            "192.168.1.5:8790",
            "[::1]:8790",
            "localhost:8790",
            "app.localhost",
            "valk.example.com",
            "VALK.example.com:443",
            "thor.tail1.ts.net:8443",
        ] {
            assert!(a.known(ok), "{ok}");
        }
        for bad in [
            "evil.example:8790",
            "localhost.evil.example",
            "valk.example.com.evil.example",
            "other.tail1.ts.net",
            "",
        ] {
            assert!(!a.known(bad), "{bad}");
        }
    }

    #[test]
    fn a_tailnet_that_came_up_later_is_learned_at_most_each_minute() {
        let a = Allowed::new(None, None);
        assert!(!a.known("thor.tail1.ts.net"));
        assert!(!a.recheck_due(), "just looked, at start");
        a.tailnet.lock().unwrap().checked = Some(Instant::now() - RECHECK);
        assert!(a.recheck_due());
        assert!(!a.recheck_due(), "once per minute");
        a.set_tailnet(Some("https://thor.tail1.ts.net"));
        assert!(a.known("thor.tail1.ts.net"));
    }
}

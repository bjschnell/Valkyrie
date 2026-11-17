//! Other machines' web apps in windows of their own (backlog #179).
//!
//! Each host's `valk web` runs there (`valk setup web --local`, started over
//! SSH) and is forwarded to a local port of its own (`remote::tunnel`), so its
//! window has an origin of its own, and with it a device token of its own. The
//! page asks for a host by navigating to `valk-desktop://host/NAME`, as it asks
//! to pair: the window hears it, and the page can't say anything else.

use anyhow::{Context, Result, bail};
use std::process::Stdio;
use valkyrie_proto::remote::{self, Auth, Host, Known};

/// What a page said by navigating to `valk-desktop://…`.
#[derive(Debug, PartialEq, Eq)]
pub enum Signal {
    /// `valk-desktop://pair`: the page isn't paired.
    Pair,
    /// `valk-desktop://host/NAME`: show that host's window, connecting to it
    /// first if need be; `valk-desktop://local` is this computer's.
    Host(Option<Host>),
}

/// The signal in a `valk-desktop:` address, if it is one this app knows.
pub fn signal(url: &str) -> Option<Signal> {
    let rest = url.strip_prefix("valk-desktop://")?;
    let rest = rest.trim_end_matches('/');
    match rest {
        "pair" => Some(Signal::Pair),
        "local" => Some(Signal::Host(None)),
        _ => {
            let name = percent_decode(rest.strip_prefix("host/")?)?;
            Host::parse(&name).ok().map(|h| Signal::Host(Some(h)))
        }
    }
}

/// `%XX` escapes, as `encodeURIComponent` writes them; `None` for a broken one.
fn percent_decode(text: &str) -> Option<String> {
    let mut out = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

/// What a window's page is told about hosts, before its own scripts run:
/// `window.__VALK_HOSTS__`, read by the web app's host switcher.
pub fn page_script(current: Option<&Host>, known: &[Known]) -> String {
    let hosts: Vec<&str> = known.iter().map(|k| k.host.as_str()).collect();
    // Host names need no escaping (`Host::parse`), but JSON is what's safe to
    // put in a script whatever they hold.
    let json = format!(
        "{{\"current\":{},\"hosts\":[{}]}}",
        current.map_or("null".into(), |h| format!("\"{h}\"")),
        hosts
            .iter()
            .map(|h| format!("\"{h}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    format!("window.__VALK_HOSTS__ = {json}; window.dispatchEvent(new Event(\"valk-hosts\"));")
}

/// A host's web app, forwarded to `port` here while this lives.
pub struct Tunnel {
    pub host: Host,
    pub port: u16,
    /// Ends with the tunnel: killed when dropped.
    ssh: tokio::process::Child,
}

impl Tunnel {
    /// Starts the host's `valk web` if it isn't running, and forwards its port
    /// here. Never asks for a password: there's no terminal to ask on, so a key,
    /// the SSH agent, or a connection `ssh` already shares must do.
    pub async fn open(host: &Host) -> Result<Tunnel> {
        let known = remote::add_host(&remote::hosts_file(), host)?;
        remote::start_web(host, Auth::Never).await?;
        let mut ssh = tokio::process::Command::from(remote::tunnel(host, Auth::Never, known.port))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("couldn't run ssh")?;
        tokio::select! {
            answered = remote::web_answers(known.port) => answered
                .with_context(|| format!("{host}'s web app didn't answer through the tunnel"))?,
            status = ssh.wait() => {
                let mut said = String::new();
                if let Some(mut err) = ssh.stderr.take() {
                    use tokio::io::AsyncReadExt;
                    let _ = err.read_to_string(&mut said).await;
                }
                bail!(
                    "the tunnel to {host} closed ({}): {}",
                    status?,
                    said.trim()
                )
            }
        }
        Ok(Tunnel {
            host: host.clone(),
            port: known.port,
            ssh,
        })
    }

    /// Whether `ssh` still holds the tunnel.
    pub fn alive(&mut self) -> bool {
        matches!(self.ssh.try_wait(), Ok(None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_page_can_say() {
        assert_eq!(signal("valk-desktop://pair"), Some(Signal::Pair));
        assert_eq!(signal("valk-desktop://local"), Some(Signal::Host(None)));
        assert_eq!(
            signal("valk-desktop://host/me%40box.ts.net"),
            Some(Signal::Host(Some(Host::parse("me@box.ts.net").unwrap())))
        );
        assert_eq!(
            signal("valk-desktop://host/box/"),
            Some(Signal::Host(Some(Host::parse("box").unwrap())))
        );
        // Nothing that could reach ssh as an option or a second word.
        assert_eq!(signal("valk-desktop://host/-oProxyCommand%3Dsh"), None);
        assert_eq!(signal("valk-desktop://host/box%20x"), None);
        assert_eq!(signal("valk-desktop://host/"), None);
        assert_eq!(signal("valk-desktop://host/%zz"), None);
        assert_eq!(signal("valk-desktop://quit"), None);
        assert_eq!(signal("http://127.0.0.1:8790/"), None);
    }

    #[test]
    fn the_page_learns_the_hosts_and_which_one_it_is() {
        let known = vec![
            Known {
                host: Host::parse("a").unwrap(),
                port: 8791,
            },
            Known {
                host: Host::parse("me@b").unwrap(),
                port: 8792,
            },
        ];
        assert_eq!(
            page_script(None, &known),
            "window.__VALK_HOSTS__ = {\"current\":null,\"hosts\":[\"a\",\"me@b\"]}; \
             window.dispatchEvent(new Event(\"valk-hosts\"));"
        );
        assert!(
            page_script(Some(&known[1].host), &[]).contains("{\"current\":\"me@b\",\"hosts\":[]}")
        );
    }
}

//! What `valk web` needs from Tailscale: this machine's tailnet name, whether the
//! tailnet can issue it an HTTPS certificate, and `tailscale serve` in front of the
//! server. All of it through the `tailscale` CLI.

use anyhow::{Result, bail};
use std::process::{Command, Stdio};

/// This machine on its tailnet.
#[derive(Debug, PartialEq)]
pub struct Tailnet {
    /// `https://<machine>.<tailnet>.ts.net`
    pub url: String,
    /// The tailnet issues HTTPS certificates. Without them `tailscale serve` takes
    /// connections and drops every one: HTTPS has to be turned on in the admin
    /// console first.
    pub certs: bool,
}

/// The CLI: on PATH, or inside the macOS app, whose CLI isn't on PATH by default.
pub fn command() -> Command {
    const MAC_APP: &str = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";
    let on_path = std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| dir.join("tailscale").is_file())
    });
    let program = if !on_path && std::path::Path::new(MAC_APP).is_file() {
        MAC_APP
    } else {
        "tailscale"
    };
    let mut command = Command::new(program);
    command.stdin(Stdio::null());
    command
}

fn json(args: &[&str]) -> Option<serde_json::Value> {
    let out = command().args(args).stderr(Stdio::null()).output().ok()?;
    out.status
        .success()
        .then(|| serde_json::from_slice(&out.stdout).ok())
        .flatten()
}

/// `None` without Tailscale, or while it's logged out.
pub fn tailnet() -> Option<Tailnet> {
    parse_status(&json(&["status", "--json"])?)
}

fn parse_status(status: &serde_json::Value) -> Option<Tailnet> {
    let name = status["Self"]["DNSName"].as_str()?.trim_end_matches('.');
    if name.is_empty() {
        return None;
    }
    let certs = status["CertDomains"]
        .as_array()
        .is_some_and(|domains| !domains.is_empty());
    Some(Tailnet {
        url: format!("https://{name}"),
        certs,
    })
}

/// `tailscale serve` already forwards to `port` on this machine.
pub fn serving(port: u16) -> bool {
    json(&["serve", "status", "--json"]).is_some_and(|config| forwards_to(&config, port))
}

fn forwards_to(config: &serde_json::Value, port: u16) -> bool {
    let text = config.to_string();
    [format!("127.0.0.1:{port}\""), format!("localhost:{port}\"")]
        .iter()
        .any(|target| text.contains(target.as_str()))
}

/// Puts the tailnet's HTTPS in front of `port`, kept across restarts.
pub fn serve(port: u16) -> Result<()> {
    let out = command()
        .args(["serve", "--bg", &port.to_string()])
        .output()?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Where to turn HTTPS certificates on.
pub const CERTS_HELP: &str = "turn on HTTPS certificates in the Tailscale admin console \
    (https://login.tailscale.com/admin/dns, \"HTTPS Certificates\"); until then phones \
    can't open the page";

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_tailnet_name_and_certificates() {
        let on = json!({"Self": {"DNSName": "thor.tail1.ts.net."}, "CertDomains": ["thor.tail1.ts.net"]});
        assert_eq!(
            parse_status(&on),
            Some(Tailnet {
                url: "https://thor.tail1.ts.net".into(),
                certs: true
            })
        );
        let off = json!({"Self": {"DNSName": "thor.tail1.ts.net."}, "CertDomains": null});
        assert!(!parse_status(&off).unwrap().certs);
        let logged_out = json!({"Self": {"DNSName": ""}});
        assert_eq!(parse_status(&logged_out), None);
    }

    #[test]
    fn finds_a_forward_to_the_port() {
        let config = json!({"Web": {"thor.tail1.ts.net:443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:8790"}}}}});
        assert!(forwards_to(&config, 8790));
        assert!(!forwards_to(&config, 879));
        assert!(!forwards_to(&json!({}), 8790));
    }
}

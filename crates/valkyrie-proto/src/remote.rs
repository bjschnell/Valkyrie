//! Another machine's Valkyrie, reached over SSH (DESIGN §8.1, backlog #179).
//!
//! Two clients use it. `valk --remote HOST` runs this machine's TUI against the
//! other machine's daemon: each connection runs `ssh HOST valk bridge`, which
//! pipes the daemon's socket over the SSH channel. The web app can't work that
//! way, since `valk web` reads the disk it runs on (the folder picker, diffs), so
//! the other machine runs its own `valk web` and `valk remote web HOST` forwards
//! a local port to it. Each host gets a port of its own here, and so an origin
//! of its own, which keeps each one's device token apart.
//!
//! SSH is the user's own: their config, keys and agent. Nothing here asks for a
//! password; `ssh` does, on the terminal, before the TUI takes it over.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Where the other machine's `valk web` listens (`valk setup web --local`).
pub const REMOTE_WEB_PORT: u16 = 8790;
/// The first local port a host's web app is forwarded to; this machine's own
/// `valk web` has `REMOTE_WEB_PORT`.
const FIRST_PORT: u16 = 8791;
/// Ports handed out at most, from `FIRST_PORT`.
const MAX_HOSTS: u16 = 200;

/// An SSH destination as the user would type it: `box`, `me@box`, an alias from
/// `~/.ssh/config`. Never anything `ssh` could read as an option, or the shell on
/// the other side as more than one word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Host(String);

impl Host {
    pub fn parse(name: &str) -> Result<Host> {
        let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | ':');
        if name.is_empty() || name.len() > 253 {
            bail!("a host is 1 to 253 characters");
        }
        if name.starts_with('-') {
            bail!("a host can't start with -: {name}");
        }
        if let Some(c) = name.chars().find(|&c| !ok(c)) {
            bail!("a host has letters, digits and . - _ @ : only, not {c:?}");
        }
        Ok(Host(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for Host {
    type Error = anyhow::Error;
    fn try_from(name: String) -> Result<Host> {
        Host::parse(&name)
    }
}

impl From<Host> for String {
    fn from(host: Host) -> String {
        host.0
    }
}

/// How an `ssh` call may get through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    /// It may ask on the terminal: before the TUI starts, or from a terminal.
    Ask,
    /// Never asks: from behind the TUI, or from the desktop app, which has no
    /// terminal. A key, the agent, or the shared connection must do.
    Never,
}

/// `ssh` to `host`, running `valk ARGS` there. Calls share one connection where
/// `ssh` can (`ControlMaster`), so the TUI's reconnects and the desktop app's
/// pairing don't log in again each time.
pub fn ssh(host: &Host, auth: Auth, args: &[&str]) -> std::process::Command {
    let mut cmd = std::process::Command::new("ssh");
    cmd.args(ssh_options(auth));
    cmd.arg(host.as_str());
    cmd.arg(remote_command(args));
    cmd
}

/// `ssh` forwarding local `port` to the other machine's `valk web`, on a
/// connection of its own: one shared with others could go with them, and a
/// forward through a shared one outlives the `ssh` that asked for it. It ends
/// at once if the port is taken here.
pub fn tunnel(host: &Host, auth: Auth, port: u16) -> std::process::Command {
    let mut cmd = std::process::Command::new("ssh");
    if auth == Auth::Never {
        cmd.args(["-o", "BatchMode=yes"]);
    }
    cmd.args(["-o", "ControlPath=none", "-o", "ExitOnForwardFailure=yes"]);
    cmd.args(["-o", "ServerAliveInterval=30", "-N", "-L"]);
    cmd.arg(format!("127.0.0.1:{port}:127.0.0.1:{REMOTE_WEB_PORT}"));
    cmd.arg(host.as_str());
    cmd
}

fn ssh_options(auth: Auth) -> Vec<String> {
    let mut opts = Vec::new();
    if auth == Auth::Never {
        opts.extend(["-o".into(), "BatchMode=yes".into()]);
    }
    // Windows' OpenSSH has no connection sharing.
    #[cfg(unix)]
    if let Some(path) = control_path() {
        opts.extend([
            "-o".into(),
            "ControlMaster=auto".into(),
            "-o".into(),
            format!("ControlPath={}", path.display()),
            "-o".into(),
            "ControlPersist=600".into(),
        ]);
    }
    opts
}

/// The shared connections' sockets: `ssh` puts a hash of the destination for
/// `%C`, so every host has its own and the path stays short.
#[cfg(unix)]
fn control_path() -> Option<PathBuf> {
    let dir = crate::state_dir().join("run");
    crate::ensure_private_dir(&dir).ok()?;
    let path = dir.join("ssh-%C");
    // `%C` is 40 hex digits, so the path grows by 38.
    (path.as_os_str().len() + 38 <= crate::MAX_SOCKET_PATH).then_some(path)
}

/// What `ssh` hands the other machine's login shell, which may be fish, bash or
/// zsh: `sh` finds `valk` on the `PATH`, else where the installer puts it, since a
/// command run over SSH often gets no `~/.local/bin` on its `PATH`. Every word in
/// `args` is checked to need no quoting.
fn remote_command(args: &[&str]) -> String {
    const FIND: &str = r#"sh -c 'v=$(command -v valk 2>/dev/null || echo "$HOME/.local/bin/valk"); exec "$v" "$@"' valk"#;
    let mut line = FIND.to_owned();
    for arg in args {
        assert!(
            !arg.is_empty()
                && arg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':')),
            "a remote argument needs no quoting: {arg:?}"
        );
        line.push(' ');
        line.push_str(arg);
    }
    line
}

/// A host the desktop app and `valk remote` know, with the local port its web
/// app is forwarded to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Known {
    pub host: Host,
    pub port: u16,
}

/// The hosts file: `<state dir>/remote/hosts.json`.
pub fn hosts_file() -> PathBuf {
    crate::state_dir().join("remote").join("hosts.json")
}

/// The hosts in `file`, in the order they were added. A missing file has none.
pub fn hosts(file: &Path) -> Result<Vec<Known>> {
    match std::fs::read(file) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("read {}", file.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", file.display())),
    }
}

/// Adds `host` to `file` with the first free port, or returns it as it is.
pub fn add_host(file: &Path, host: &Host) -> Result<Known> {
    let mut known = hosts(file)?;
    if let Some(k) = known.iter().find(|k| &k.host == host) {
        return Ok(k.clone());
    }
    let Some(port) =
        (FIRST_PORT..FIRST_PORT + MAX_HOSTS).find(|p| known.iter().all(|k| k.port != *p))
    else {
        bail!("{MAX_HOSTS} hosts already; remove one with `valk remote rm`");
    };
    let k = Known {
        host: host.clone(),
        port,
    };
    known.push(k.clone());
    save(file, &known)?;
    Ok(k)
}

/// Takes `host` out of `file`; whether it was there.
pub fn remove_host(file: &Path, host: &Host) -> Result<bool> {
    let mut known = hosts(file)?;
    let before = known.len();
    known.retain(|k| &k.host != host);
    if known.len() == before {
        return Ok(false);
    }
    save(file, &known)?;
    Ok(true)
}

fn save(file: &Path, known: &[Known]) -> Result<()> {
    let dir = file.parent().context("hosts file has no directory")?;
    crate::ensure_private_dir(dir)?;
    let tmp = file.with_extension(format!("json.{}", std::process::id()));
    {
        use std::io::Write;
        let mut f = crate::private_file()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(known)?)?;
    }
    std::fs::rename(&tmp, file).with_context(|| format!("write {}", file.display()))
}

/// The local address a host's web app is forwarded to.
pub fn local_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// The pairing code in a link the other machine's `valk web open --print` made,
/// if the line holds one. Only the code crosses: the link itself names the other
/// machine's port.
pub fn pairing_code(line: &str) -> Option<&str> {
    let code = line.trim().split_once("#pair=")?.1;
    (!code.is_empty()
        && code.len() <= 128
        && code
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
    .then_some(code)
}

/// How long the other machine's web app gets to answer through the tunnel:
/// setting it up waits ten seconds of its own.
const WEB_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Makes sure the other machine's `valk web` runs, as the desktop app does for
/// its own: for that machine only, and one already set up is left as it is.
/// What it says goes to stderr: stdout may be a program's.
pub async fn start_web(host: &Host, auth: Auth) -> Result<()> {
    let setup = tokio::process::Command::from(ssh(host, auth, &["setup", "web", "--local"]))
        .stdin(std::process::Stdio::null())
        .stdout(std::io::stderr())
        .status()
        .await
        .context("couldn't run ssh")?;
    if !setup.success() {
        bail!("couldn't start valk web on {host} (`valk setup web --local` there failed)");
    }
    Ok(())
}

/// Waits until a web app answers on local `port`: an HTTP status line, not just
/// the tunnel's listener, which accepts before the other side is there.
pub async fn web_answers(port: u16) -> Result<()> {
    let deadline = tokio::time::Instant::now() + WEB_WAIT;
    loop {
        if http_answers(port).await {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            bail!("nothing answered for {} s", WEB_WAIT.as_secs());
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

async fn http_answers(port: u16) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let ask = async {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .ok()?;
        stream
            .write_all(b"HEAD / HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .ok()?;
        let mut head = [0u8; 8];
        stream.read_exact(&mut head).await.ok()?;
        Some(head.starts_with(b"HTTP/1."))
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), ask)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

/// A link that pairs a browser at `url` (the tunnel's end here) with the other
/// machine's web app, made there by `valk web open --print`. With a terminal
/// there (`-tt`): it pairs only for a person, as from your own. Ask this
/// machine's daemon first; the other one can't see past `sshd`.
pub async fn pairing_link(host: &Host, auth: Auth, url: &str) -> Result<String> {
    let mut cmd = std::process::Command::new("ssh");
    cmd.args(ssh_options(auth));
    cmd.arg("-tt");
    cmd.arg(host.as_str());
    cmd.arg(remote_command(&["web", "open", "--print"]));
    let out = tokio::process::Command::from(cmd)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .context("couldn't run ssh")?;
    let said = String::from_utf8_lossy(&out.stdout);
    match said.lines().find_map(pairing_code) {
        Some(code) => Ok(format!("{}/#pair={code}", url.trim_end_matches('/'))),
        None => bail!(
            "{host} made no pairing link: {}",
            one_line(&format!("{said} {}", String::from_utf8_lossy(&out.stderr)))
        ),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_is_one_word_ssh_reads_as_a_destination() {
        for ok in [
            "box",
            "me@box",
            "box.tail1234.ts.net",
            "10.0.0.2",
            "fe80::1",
            "my_box-2",
        ] {
            assert_eq!(Host::parse(ok).unwrap().as_str(), ok);
        }
        for bad in [
            "",
            "-oProxyCommand=sh",
            "box; rm -rf ~",
            "box other",
            "box\n",
            "$(id)",
            "'box'",
            "box`id`",
        ] {
            assert!(Host::parse(bad).is_err(), "{bad:?}");
        }
        assert!(Host::parse(&"a".repeat(254)).is_err());
        // Read back from the hosts file the same way.
        assert!(serde_json::from_str::<Host>("\"-oX\"").is_err());
    }

    #[test]
    fn the_remote_command_finds_valk_off_the_path_too() {
        assert_eq!(
            remote_command(&["bridge", "--agent", "claude"]),
            r#"sh -c 'v=$(command -v valk 2>/dev/null || echo "$HOME/.local/bin/valk"); exec "$v" "$@"' valk bridge --agent claude"#
        );
    }

    #[test]
    #[should_panic(expected = "needs no quoting")]
    fn a_remote_argument_never_needs_quoting() {
        remote_command(&["web", "x y"]);
    }

    #[test]
    fn the_tunnel_forwards_one_local_port_on_its_own_connection() {
        let host = Host::parse("box").unwrap();
        let args: Vec<_> = tunnel(&host, Auth::Never, 8791)
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-o",
                "BatchMode=yes",
                "-o",
                "ControlPath=none",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ServerAliveInterval=30",
                "-N",
                "-L",
                "127.0.0.1:8791:127.0.0.1:8790",
                "box"
            ]
        );
    }

    #[test]
    fn never_asking_means_batch_mode() {
        let opts = ssh_options(Auth::Never);
        assert_eq!(opts[..2], ["-o", "BatchMode=yes"]);
        assert!(!ssh_options(Auth::Ask).contains(&"BatchMode=yes".to_string()));
    }

    #[test]
    fn each_host_keeps_its_own_port() {
        let dir = std::env::temp_dir().join(format!("valk-remote-hosts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("hosts.json");
        assert!(hosts(&file).unwrap().is_empty());
        let a = Host::parse("a").unwrap();
        let b = Host::parse("b").unwrap();
        assert_eq!(add_host(&file, &a).unwrap().port, 8791);
        assert_eq!(add_host(&file, &b).unwrap().port, 8792);
        // Again: the same port, so the same origin and device token.
        assert_eq!(add_host(&file, &a).unwrap().port, 8791);
        assert!(remove_host(&file, &a).unwrap());
        assert!(!remove_host(&file, &a).unwrap());
        // A freed port goes to the next host.
        let c = Host::parse("c").unwrap();
        assert_eq!(add_host(&file, &c).unwrap().port, 8791);
        let names: Vec<_> = hosts(&file)
            .unwrap()
            .into_iter()
            .map(|k| k.host.0)
            .collect();
        assert_eq!(names, ["b", "c"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_the_code_comes_from_the_other_machines_link() {
        assert_eq!(
            pairing_code("http://127.0.0.1:8790/#pair=Ab3_x-9\r\n"),
            Some("Ab3_x-9")
        );
        assert_eq!(pairing_code("http://127.0.0.1:8790/"), None);
        assert_eq!(pairing_code("x#pair="), None);
        assert_eq!(pairing_code("x#pair=a\"><script>"), None);
        assert_eq!(pairing_code("x#pair=a b"), None);
    }
}

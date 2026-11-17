//! Another machine's Valkyrie over SSH (DESIGN §8.1, backlog #179).
//!
//! - `valk bridge` runs on the other machine: it starts the daemon if need be,
//!   then pipes its socket to stdin and stdout, for `ssh` to carry.
//! - `valk --remote HOST` runs here: a private socket of its own, where every
//!   connection runs `ssh HOST valk bridge`. The TUI and the commands that make
//!   sense there connect to it as to a daemon, and reconnect through it after an
//!   upgrade or a dropped network as they would locally.
//! - `valk remote web HOST` forwards a local port to the other machine's own
//!   `valk web` and pairs this machine's browser with it. The desktop app does
//!   the same itself, through `valkyrie_proto::remote`.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use valkyrie_proto::remote::{self, Auth, Host};
// The proxy behind `--remote` is a unix socket.
#[cfg(unix)]
use {
    anyhow::anyhow,
    std::path::PathBuf,
    std::sync::{Arc, Mutex},
    std::time::Duration,
    tokio::io::AsyncBufReadExt,
    valkyrie_proto::client::{Client, Pushes},
};

/// What's kept of `ssh`'s complaints, for the message when it can't connect.
#[cfg(unix)]
const MAX_ERROR: usize = 2048;

/// `valk bridge`: the daemon's socket on stdin and stdout. Nothing else may
/// write to stdout, so notes go to stderr, which `ssh` carries back.
pub async fn bridge(socket: &Path) -> Result<()> {
    drop(crate::start_or_connect(socket).await?);
    let stream = valkyrie_proto::ipc::connect(socket)
        .await
        .with_context(|| format!("connect {}", socket.display()))?;
    let (mut rd, mut wr) = tokio::io::split(stream);
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    tokio::select! {
        _ = tokio::io::copy(&mut stdin, &mut wr) => {}
        _ = async {
            let _ = tokio::io::copy(&mut rd, &mut stdout).await;
            let _ = stdout.flush().await;
        } => {}
    }
    // Not back through the runtime: it would wait on the thread blocked reading
    // stdin, which only `ssh` closing ends.
    std::process::exit(0)
}

/// The local end of `valk --remote HOST`: a socket that reaches HOST's daemon.
#[cfg(unix)]
pub struct Proxy {
    pub host: Host,
    pub socket: PathBuf,
    shared: Arc<Shared>,
    accept: tokio::task::JoinHandle<()>,
}

#[cfg(unix)]
struct Shared {
    /// Whether `ssh` may still ask on the terminal: until the TUI takes it.
    ask: std::sync::atomic::AtomicBool,
    /// The last thing `ssh` (or `valk` over there) said on stderr.
    error: Mutex<String>,
}

#[cfg(unix)]
impl Proxy {
    /// Listens on a socket of this process's own. `agent` is what to tell the
    /// other machine runs this, when this machine's daemon says it's an agent.
    pub fn start(host: Host, agent: Option<String>) -> Result<Proxy> {
        let dir = valkyrie_proto::state_dir().join("run");
        valkyrie_proto::ensure_private_dir(&dir)?;
        sweep(&dir);
        // By pid: an exec'd TUI keeps it, and binds the same path again.
        let socket = dir.join(format!("remote-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);
        let listener = tokio::net::UnixListener::bind(&socket)
            .with_context(|| format!("listen on {}", socket.display()))?;
        let shared = Arc::new(Shared {
            ask: true.into(),
            error: Mutex::default(),
        });
        let accept = {
            let (host, shared) = (host.clone(), shared.clone());
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let cmd = bridge_command(&host, agent.as_deref(), &shared);
                    let shared = shared.clone();
                    tokio::spawn(async move { carry(stream, cmd, &shared).await });
                }
            })
        };
        Ok(Proxy {
            host,
            socket,
            shared,
            accept,
        })
    }

    /// What the rest of the process holds to connect through it.
    pub fn handle(&self) -> Handle {
        Handle {
            host: self.host.clone(),
            shared: self.shared.clone(),
        }
    }
}

/// Takes away the sockets of `valk --remote`s that ended without cleaning up
/// (killed, or their terminal closed): a socket named after a pid that's gone.
#[cfg(unix)]
fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("remote-")?.strip_suffix(".sock"))
            .and_then(|pid| pid.parse::<i32>().ok())
            .filter(|&pid| pid > 0)
        else {
            continue;
        };
        // SAFETY: signal 0 only checks that the process exists.
        let gone = unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Connects through a `Proxy`.
#[cfg(unix)]
pub struct Handle {
    host: Host,
    shared: Arc<Shared>,
}

#[cfg(unix)]
impl Handle {
    /// The other machine's daemon through the proxy at `socket`, once it has
    /// said it speaks this protocol. From then on `ssh` never asks on the
    /// terminal, which becomes the TUI's: the connection just made is shared, so
    /// a key or password isn't needed again unless it drops.
    pub async fn connect(&self, socket: &Path) -> Result<(Client, Pushes)> {
        let host = &self.host;
        let conn = Client::connect(socket).await?;
        let protocol = conn
            .0
            .hello()
            .await
            .map_err(|_| anyhow!("couldn't reach valk on {host}: {}", self.why()))?;
        if protocol != valkyrie_proto::PROTOCOL {
            bail!(
                "valk on {host} speaks protocol {protocol}, this one {}; bring both to the \
                 same release (`valk upgrade` on each keeps every session)",
                valkyrie_proto::PROTOCOL
            );
        }
        self.shared
            .ask
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(conn)
    }

    /// What went wrong reaching the other machine, as `ssh` put it.
    fn why(&self) -> String {
        let error = self.shared.error.lock().unwrap();
        let error = error.trim();
        if error.is_empty() {
            format!("ssh {} ended without a word", self.host)
        } else {
            one_line(error)
        }
    }
}

#[cfg(unix)]
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(unix)]
impl Drop for Proxy {
    fn drop(&mut self) {
        self.accept.abort();
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// `ssh HOST valk bridge`, asking on the terminal only while it may.
#[cfg(unix)]
fn bridge_command(host: &Host, agent: Option<&str>, shared: &Shared) -> std::process::Command {
    let auth = if shared.ask.load(std::sync::atomic::Ordering::SeqCst) {
        Auth::Ask
    } else {
        Auth::Never
    };
    let mut args = vec!["bridge"];
    if let Some(agent) = agent {
        args.extend(["--agent", agent]);
    }
    remote::ssh(host, auth, &args)
}

/// One connection, through `cmd` (`ssh HOST valk bridge`), piped both ways
/// until either side goes.
#[cfg(unix)]
async fn carry(stream: tokio::net::UnixStream, cmd: std::process::Command, shared: &Shared) {
    let mut cmd = tokio::process::Command::from(cmd);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            *shared.error.lock().unwrap() = format!("couldn't run ssh: {e}");
            return;
        }
    };
    // What `why` reports is this connection's, not an older one's.
    shared.error.lock().unwrap().clear();
    let (Some(mut to), Some(mut from), Some(err)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return;
    };
    // Line by line, as it comes: when `ssh` fails, its reason and the end of
    // stdout arrive together.
    let error = async {
        let mut lines = tokio::io::BufReader::new(err).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let mut error = shared.error.lock().unwrap();
            if error.len() + line.len() >= MAX_ERROR {
                error.clear();
            }
            error.push_str(&line);
            error.push('\n');
        }
    };
    let (mut rd, mut wr) = stream.into_split();
    let up = tokio::io::copy(&mut rd, &mut to);
    let down = tokio::io::copy(&mut from, &mut wr);
    tokio::pin!(error, up, down);
    let mut said_all = false;
    loop {
        tokio::select! {
            _ = &mut up => break,
            _ = &mut down => break,
            _ = &mut error, if !said_all => said_all = true,
        }
    }
    // What's left of it, but never to the end: a shared connection `ssh` leaves
    // behind holds stderr open for as long as it lives.
    if !said_all {
        let _ = tokio::time::timeout(Duration::from_millis(500), error).await;
    }
}

/// What this machine's daemon makes of this process: `Some` an agent, which
/// the other machine is then told. No daemon here means no session either.
#[cfg(unix)]
pub async fn local_agent() -> Option<String> {
    let socket = valkyrie_proto::daemon_socket(None);
    let ask = async {
        let (client, _) = valkyrie_proto::client::Client::connect(&socket)
            .await
            .ok()?;
        client.vouch("reaching another machine").await.err()
    };
    tokio::time::timeout(Duration::from_secs(2), ask)
        .await
        .ok()
        .flatten()
        .map(|_| "agent".to_owned())
}

/// `valk remote ls`.
pub fn list() -> Result<()> {
    let known = remote::hosts(&remote::hosts_file())?;
    if known.is_empty() {
        println!("no hosts yet; `valk remote add HOST`, or `valk --remote HOST`");
    }
    for k in known {
        println!("{}  (web app on {})", k.host, remote::local_url(k.port));
    }
    Ok(())
}

/// `valk remote add HOST`.
pub fn add(host: &Host) -> Result<()> {
    let k = remote::add_host(&remote::hosts_file(), host)?;
    println!(
        "{}: `valk --remote {}`, or its web app on {}",
        k.host,
        k.host,
        remote::local_url(k.port)
    );
    Ok(())
}

/// `valk remote rm HOST`.
pub fn remove(host: &Host) -> Result<()> {
    if !remote::remove_host(&remote::hosts_file(), host)? {
        bail!("{host} isn't one of your hosts (`valk remote ls`)");
    }
    println!("removed {host}");
    Ok(())
}

/// `valk remote web HOST`: the other machine's web app in this machine's
/// browser, paired, through a tunnel held until Ctrl-C.
pub async fn web(host: &Host) -> Result<()> {
    let known = remote::add_host(&remote::hosts_file(), host)?;
    let url = remote::local_url(known.port);
    let there = remote::start_web(host, Auth::Ask).await?;
    let mut tunnel =
        tokio::process::Command::from(remote::tunnel(host, Auth::Ask, known.port, there))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("couldn't run ssh")?;
    tokio::select! {
        answered = remote::web_answers(known.port) => {
            answered.with_context(|| format!("{host}'s web app didn't answer on {url}"))?
        }
        status = tunnel.wait() => bail!(
            "the tunnel to {host}'s {there} closed ({}); is port {} free here?",
            status?,
            known.port
        ),
    }
    let link = remote::pairing_link(host, Auth::Ask, &url, there).await?;
    if valkyrie_tui::open_link(&link).is_ok() {
        println!("opened {host}'s web app in your browser");
        println!("  (if it didn't open: {link})");
    } else {
        println!("open this in a browser on this machine: {link}");
    }
    println!("  {host} stays on {url} until Ctrl-C");
    tokio::select! {
        status = tunnel.wait() => bail!("the tunnel to {host} closed ({})", status?),
        _ = tokio::signal::ctrl_c() => Ok(()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    /// A stand-in for `ssh HOST valk bridge` that echoes what it reads: the
    /// proxy must carry bytes both ways, and keep what it said on stderr.
    #[tokio::test]
    async fn the_proxy_carries_a_connection_through_ssh() {
        let shared = Shared {
            ask: false.into(),
            error: Mutex::default(),
        };
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "echo 'said on stderr' >&2; head -c 5"]);
        let (mut a, b) = tokio::net::UnixStream::pair().unwrap();
        let carried = tokio::spawn(async move {
            carry(b, cmd, &shared).await;
            shared
        });
        a.write_all(b"hello").await.unwrap();
        let mut back = [0u8; 5];
        a.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"hello");
        // The far end hung up: so does the connection.
        let mut rest = Vec::new();
        a.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
        let shared = carried.await.unwrap();
        assert!(shared.error.lock().unwrap().contains("said on stderr"));
    }

    #[test]
    fn sweeps_only_the_sockets_of_proxies_that_are_gone() {
        let dir = std::env::temp_dir().join(format!("valk-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mine = dir.join(format!("remote-{}.sock", std::process::id()));
        // A pid past any the kernel hands out.
        let gone = dir.join("remote-2147483646.sock");
        let other = dir.join("Thor.sock");
        for f in [&mine, &gone, &other] {
            std::fs::write(f, "").unwrap();
        }
        sweep(&dir);
        assert!(mine.exists());
        assert!(!gone.exists());
        assert!(other.exists(), "the daemon's own socket stays");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_agent_here_is_named_to_the_other_machine() {
        let shared = Shared {
            ask: true.into(),
            error: Mutex::default(),
        };
        let host = Host::parse("box").unwrap();
        let args = |agent| {
            bridge_command(&host, agent, &shared)
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        assert!(args(None).last().unwrap().ends_with(" valk bridge"));
        assert!(
            args(Some("agent"))
                .last()
                .unwrap()
                .ends_with(" valk bridge --agent agent")
        );
        assert!(!args(None).contains(&"BatchMode=yes".to_string()));
        shared.ask.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(args(None).contains(&"BatchMode=yes".to_string()));
    }
}

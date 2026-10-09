mod bench;
mod context;
mod hook;
mod service;
mod tools;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use valkyrie_proto::client::{Client, Pushes};
use valkyrie_proto::{SessionId, Size, SpawnSpec};

#[derive(Parser)]
#[command(
    name = "valk",
    version,
    about = "Attention-first manager for coding-agent CLIs"
)]
struct Cli {
    /// Daemon socket path.
    #[arg(long, global = true, env = "VALK_SOCKET")]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon in the foreground.
    Daemon {
        /// Take over from a previous daemon image (set by an upgrade handoff).
        #[arg(long, hide = true)]
        resume_fd: Option<i32>,
        /// Check that a handoff on stdin parses, print the format and exit.
        #[arg(long, hide = true)]
        handoff_check: bool,
    },
    /// Switch the running daemon to this binary, keeping every session (ADR-0006).
    Upgrade {
        /// The valk binary to switch to (default: this one).
        #[arg(long)]
        exe: Option<PathBuf>,
    },
    /// Spawn a session: `valk new -- claude`.
    New {
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
        /// Attach right after spawning.
        #[arg(short, long)]
        attach: bool,
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// List sessions.
    Ls,
    /// Open the TUI, optionally attached to a session.
    Attach { session: Option<SessionId> },
    /// Kill a session.
    Kill { session: SessionId },
    /// Name a session; without a name it goes back to its directory's.
    Rename {
        session: SessionId,
        name: Option<String>,
    },
    /// Print a session's visible screen as plain text.
    Dump { session: SessionId },
    /// Send input to a session. Understands \r \n \t \e and \xHH escapes.
    Send { session: SessionId, text: String },
    /// Forward an agent hook event (stdin JSON) to the daemon. Agents run this; it
    /// prints nothing, always exits 0, and does nothing outside a Valkyrie session.
    #[command(hide = true)]
    Hook { agent: String },
    /// Print the project's decisions for an agent's session start (ADR-0007). Agents
    /// run this; like `hook`, it never fails.
    #[command(hide = true, name = "context-hook")]
    ContextHook { agent: String },
    /// Record a project decision that every agent in this repo gets from now on. Run
    /// by an agent, it is only proposed, for you to accept.
    Decide {
        /// One line.
        title: String,
        /// A sentence or two of why; `-` reads it from stdin.
        body: Option<String>,
        #[arg(short, long, default_value = "decision", value_parser = context::parse_kind)]
        kind: valkyrie_proto::DecisionKind,
        /// Ask for review instead of making it active at once.
        #[arg(long)]
        propose: bool,
        /// The decision this one replaces, once accepted.
        #[arg(long, value_name = "ID")]
        supersedes: Option<u32>,
    },
    /// Hand a session's work to another agent: its goal and asks, where it left off
    /// (summarized by a small model), the files it changed, the project's decisions.
    /// Prints it, or with --to starts that agent on it.
    Handoff {
        session: SessionId,
        /// Start this agent (claude or codex) in the same directory, with the
        /// handoff as its first message.
        #[arg(long, value_parser = ["claude", "codex"])]
        to: Option<String>,
        /// Attach to the new session (with --to).
        #[arg(short, long, requires = "to")]
        attach: bool,
        /// Skip the model's summary; use the session's last reply.
        #[arg(long)]
        no_summary: bool,
    },
    /// List this project's decisions, or review one.
    Decisions {
        /// Include rejected, superseded and retired ones.
        #[arg(long)]
        all: bool,
        #[command(subcommand)]
        action: Option<context::DecisionsCmd>,
    },
    /// Install agent integrations.
    #[command(subcommand)]
    Setup(SetupCmd),
    /// Serve the web app (phones, other machines) on localhost; put it on your
    /// tailnet with `tailscale serve`.
    Web {
        /// Where to listen. Keep it on localhost unless you know why not.
        #[arg(long, default_value = "127.0.0.1:8790")]
        listen: std::net::SocketAddr,
        /// The address phones open, for the pairing QR code (default: this
        /// machine's tailnet name).
        #[arg(long)]
        url: Option<String>,
        #[command(subcommand)]
        action: Option<WebCmd>,
    },
    /// Print the screen a recorded transcript (`.raw`) leaves behind.
    Render {
        file: PathBuf,
        #[arg(long, default_value_t = 120)]
        cols: u16,
        #[arg(long, default_value_t = 40)]
        rows: u16,
        /// Stop after this many bytes.
        #[arg(long)]
        upto: Option<usize>,
    },
    /// Re-run a recorded session (`<stem>.events.jsonl`) through the current state
    /// detection: timeline, drift from the recording, and accuracy against labels.
    Replay {
        events: PathBuf,
        /// JSON lines `{"at": <seconds>, "state": "<state>"}`, each holding until the next.
        #[arg(long)]
        labels: Option<PathBuf>,
    },
    #[command(subcommand)]
    Bench(bench::BenchCmd),
}

#[derive(Subcommand)]
enum WebCmd {
    /// Show a new pairing QR code for the running server.
    Pair,
    /// List paired devices.
    Devices,
    /// Unpair a device by name, or `all`.
    Revoke { name: String },
}

#[derive(Subcommand)]
enum SetupCmd {
    /// Add Valkyrie's hooks to ~/.codex/hooks.json (then trust them once with /hooks).
    Codex {
        /// Print the resulting hooks.json instead of writing it.
        #[arg(long)]
        dry_run: bool,
        /// Remove Valkyrie's hooks instead.
        #[arg(long)]
        remove: bool,
        /// The valk binary the hooks run (default: this one).
        #[arg(long)]
        exe: Option<PathBuf>,
    },
    /// Keep `valk web` running in the background: a systemd user service on Linux,
    /// a launchd agent on macOS. Also puts it behind `tailscale serve`, then shows
    /// a pairing QR code.
    Web {
        /// Where it listens.
        #[arg(long, default_value = "127.0.0.1:8790")]
        listen: std::net::SocketAddr,
        /// The address phones open (default: this machine's tailnet name).
        #[arg(long)]
        url: Option<String>,
        /// The valk binary the service runs (default: this one, by its PATH name).
        #[arg(long)]
        exe: Option<PathBuf>,
        /// Print the service file instead of installing it.
        #[arg(long)]
        dry_run: bool,
        /// Stop the service and remove it instead.
        #[arg(long)]
        remove: bool,
        /// The service's name (tests run one beside the real one).
        #[arg(long, hide = true, default_value = service::DEFAULT_NAME)]
        name: String,
    },
}

fn main() -> Result<()> {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // A hook must never print or exit nonzero (exit 2 blocks the agent), even
        // when its arguments or environment don't parse.
        Err(_) if std::env::args().any(|a| a == "hook" || a == "context-hook") => return Ok(()),
        Err(e) => e.exit(),
    };
    let socket = cli
        .socket
        .unwrap_or_else(valkyrie_proto::default_socket_path);
    // Hooks run on every agent event: skip the async runtime and never fail.
    if let Some(Cmd::Hook { agent }) = &cli.cmd {
        hook::run(agent, &socket);
        return Ok(());
    }
    if let Some(Cmd::ContextHook { agent }) = &cli.cmd {
        context::hook(agent);
        return Ok(());
    }
    tokio::runtime::Runtime::new()?.block_on(run(cli.cmd, socket))
}

async fn run(cmd: Option<Cmd>, socket: PathBuf) -> Result<()> {
    match cmd {
        Some(Cmd::Daemon {
            handoff_check: true,
            ..
        }) => {
            let mut json = Vec::new();
            std::io::Read::read_to_end(&mut std::io::stdin(), &mut json)?;
            valkyrie_daemon::check_handoff(&json)?;
            println!("{}", valkyrie_daemon::HANDOFF_VERSION);
            Ok(())
        }
        Some(Cmd::Daemon { resume_fd, .. }) => {
            init_logging();
            let exe = std::env::current_exe()?;
            let state = valkyrie_proto::state_dir();
            match resume_fd {
                Some(fd) => valkyrie_daemon::resume(fd, &state, &exe).await,
                None => valkyrie_daemon::run(&socket, &state, &exe).await,
            }
        }
        Some(Cmd::Upgrade { exe }) => upgrade(&socket, exe).await,
        None => {
            require_tty()?;
            let (client, pushes) = connect(&socket).await?;
            valkyrie_tui::run(client, pushes, None, socket).await
        }
        Some(Cmd::Attach { session }) => {
            require_tty()?;
            let (client, pushes) = connect(&socket).await?;
            valkyrie_tui::run(client, pushes, session, socket).await
        }
        Some(Cmd::New {
            cwd,
            name,
            attach,
            command,
        }) => {
            // Check before spawning so a failed attach can't leave a stray session behind.
            if attach {
                require_tty()?;
            }
            let (client, pushes) = connect(&socket).await?;
            // A terminal without a real size (`ssh host valk new …`, scripts)
            // reports 0×0; the first attach resizes the session anyway.
            let size = valkyrie_tui::session_size()
                .ok()
                .filter(|s| s.cols >= 20 && s.rows >= 4)
                .unwrap_or(Size {
                    cols: 120,
                    rows: 40,
                });
            let cwd = Some(match cwd {
                Some(dir) => std::fs::canonicalize(dir)?,
                None => std::env::current_dir()?,
            });
            // Before the spawn: an image program asks for the cell size at startup.
            if let Some((width, height)) = valkyrie_tui::cell_pixels() {
                let _ = client.cell_pixels(None, width, height);
            }
            let info = client
                .spawn(SpawnSpec {
                    command,
                    cwd,
                    name,
                    size,
                    env: valkyrie_proto::login_env(),
                })
                .await?;
            if attach {
                valkyrie_tui::run(client, pushes, Some(info.id), socket).await
            } else {
                println!("{}", info.id);
                Ok(())
            }
        }
        Some(Cmd::Ls) => {
            let (client, _) = connect(&socket).await?;
            for s in client.list().await? {
                let state = match s.exited {
                    None => s.status.state.label().to_string(),
                    Some(Some(code)) => format!("exited {code}"),
                    Some(None) => "exited".into(),
                };
                println!(
                    "{:>3}  {:<14} {:<7} {:<13} {}  {}",
                    s.id,
                    s.name,
                    s.status.agent,
                    state,
                    s.command.join(" "),
                    s.cwd.display()
                );
                if let Some(summary) = &s.status.summary {
                    println!("     {summary}");
                }
            }
            Ok(())
        }
        Some(Cmd::Web {
            listen,
            url,
            action,
        }) => match action {
            None => {
                init_logging();
                // The daemon must be up, and speak this protocol, before phones arrive.
                let (client, _) = connect(&socket).await?;
                let pair = client.vouch("pairing a phone").await.is_ok();
                drop(client);
                valkyrie_web::run(valkyrie_web::Options {
                    listen,
                    socket,
                    url,
                    pair,
                })
                .await
            }
            Some(WebCmd::Pair) => {
                // A paired device can accept decisions; an agent mustn't pair one.
                connect(&socket).await?.0.vouch("pairing a phone").await?;
                let url = valkyrie_web::public_url(url, listen);
                valkyrie_web::print_pairing(&valkyrie_web::store()?, &url)
            }
            Some(WebCmd::Devices) => {
                for d in valkyrie_web::store()?.devices() {
                    println!("{}  (paired {})", d.name, d.created_unix);
                }
                Ok(())
            }
            Some(WebCmd::Revoke { name }) => {
                let n = valkyrie_web::store()?.revoke(&name)?;
                println!("unpaired {n} device(s)");
                Ok(())
            }
        },
        Some(Cmd::Kill { session }) => connect(&socket).await?.0.kill(session).await,
        Some(Cmd::Rename { session, name }) => {
            connect(&socket).await?.0.rename(session, name).await
        }
        Some(Cmd::Dump { session }) => {
            print!("{}", connect(&socket).await?.0.dump(session).await?);
            Ok(())
        }
        Some(Cmd::Send { session, text }) => {
            let client = connect(&socket).await?.0;
            client.input(session, unescape(&text)?)?;
            // Input is fire-and-forget; a round trip guarantees it was flushed before exit.
            client.list().await.map(drop)
        }
        Some(Cmd::Hook { .. } | Cmd::ContextHook { .. }) => {
            unreachable!("handled before the runtime starts")
        }
        Some(Cmd::Decide {
            title,
            body,
            kind,
            propose,
            supersedes,
        }) => {
            let client = connect(&socket).await?.0;
            let args = context::Decide {
                title,
                body,
                kind,
                propose,
                supersedes,
            };
            context::decide(&client, args).await
        }
        Some(Cmd::Handoff {
            session,
            to,
            attach,
            no_summary,
        }) => {
            if attach {
                require_tty()?;
            }
            let (client, pushes) = connect(&socket).await?;
            let (text, from) = context::handoff(&client, session, !no_summary).await?;
            let Some(agent) = to else {
                print!("{text}");
                return Ok(());
            };
            let info = client
                .spawn(SpawnSpec {
                    command: vec![agent, text],
                    cwd: Some(from.cwd),
                    name: None,
                    size: Size {
                        cols: 120,
                        rows: 40,
                    },
                    env: valkyrie_proto::login_env(),
                })
                .await?;
            if attach {
                valkyrie_tui::run(client, pushes, Some(info.id), socket).await
            } else {
                println!("{}", info.id);
                Ok(())
            }
        }
        Some(Cmd::Decisions { all, action }) => {
            context::decisions(&connect(&socket).await?.0, all, action).await
        }
        Some(Cmd::Setup(SetupCmd::Codex {
            dry_run,
            remove,
            exe,
        })) => {
            let exe = match exe {
                Some(exe) => std::path::absolute(exe)?,
                None => std::env::current_exe()?,
            };
            hook::setup_codex(&exe, &hook::codex_hooks_path(), remove, dry_run)
        }
        Some(Cmd::Setup(SetupCmd::Web {
            listen,
            url,
            exe,
            dry_run,
            remove,
            name,
        })) => {
            let exe = match exe {
                Some(exe) => std::path::absolute(exe)?,
                None => service::default_exe()?,
            };
            // It ends by printing a pairing code.
            connect(&socket)
                .await?
                .0
                .vouch("setting up the web app")
                .await?;
            service::run(service::Setup {
                name,
                exe,
                socket: std::path::absolute(&socket)?,
                listen,
                url,
                dry_run,
                remove,
            })
        }
        Some(Cmd::Render {
            file,
            cols,
            rows,
            upto,
        }) => {
            print!("{}", tools::render(&file, Size { cols, rows }, upto)?);
            Ok(())
        }
        Some(Cmd::Replay { events, labels }) => {
            print!("{}", tools::replay_report(&events, labels.as_deref())?);
            Ok(())
        }
        Some(Cmd::Bench(cmd)) => bench::run(cmd, &socket).await,
    }
}

/// The TUI reads raw keys from stdin and draws to stdout, so both must be a terminal.
fn require_tty() -> Result<()> {
    use std::io::IsTerminal;
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        bail!("the TUI needs an interactive terminal (stdin and stdout must be a tty)");
    }
    Ok(())
}

/// Connect, starting a background daemon first if none is listening.
/// Hands the running daemon over to `exe` and waits until the new image answers.
/// Deliberately skips the protocol check: the old daemon may speak an older one, and
/// `Hello` and `Upgrade` keep their shape across versions for exactly this.
async fn upgrade(socket: &Path, exe: Option<PathBuf>) -> Result<()> {
    let exe = std::fs::canonicalize(match exe {
        Some(exe) => exe,
        None => std::env::current_exe()?,
    })?;
    let (client, _pushes) = Client::connect(socket).await.with_context(|| {
        format!(
            "no daemon on {}; nothing to upgrade (the next valk command starts one)",
            socket.display()
        )
    })?;
    const PREDATES: &str =
        "the running daemon predates upgrades; restart it once instead (this ends its sessions)";
    let before = client.hello_info().await.context(PREDATES)?;
    anyhow::ensure!(before.protocol >= 3, PREDATES);
    let sessions = client.list().await?.len();
    client.upgrade(exe.clone()).await?;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Ok((client, _)) = Client::connect(socket).await
            && let Ok(now) = client.hello_info().await
        {
            anyhow::ensure!(
                now.boot == before.boot,
                "a different daemon answered: the old one went away instead of handing off; see {}",
                valkyrie_proto::state_dir().join("daemon.log").display()
            );
            anyhow::ensure!(
                now.generation > before.generation,
                "the daemon is still the old one (generation {}); see {}",
                now.generation,
                valkyrie_proto::state_dir().join("daemon.log").display()
            );
            let (protocol, after) = (now.protocol, now.generation);
            let kept = client.list().await?.len();
            println!(
                "daemon now runs {} (protocol {protocol}, generation {after}); {kept}/{sessions} sessions kept",
                exe.display()
            );
            return Ok(());
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "the daemon did not come back; see {}",
            valkyrie_proto::state_dir().join("daemon.log").display()
        );
    }
}

async fn connect(socket: &Path) -> Result<(Client, Pushes)> {
    let conn = start_or_connect(socket).await?;
    match conn.0.hello().await {
        Ok(p) if p == valkyrie_proto::PROTOCOL => Ok(conn),
        Ok(p) => bail!(
            "the running daemon speaks protocol {p}, this valk {}; run `valk upgrade` \
             (keeps every session)",
            valkyrie_proto::PROTOCOL
        ),
        // Daemons before protocol 2 hang up on `Hello`.
        Err(_) => bail!(
            "the running daemon on {} is older than this valk. Stop it to upgrade \
             (this ends its sessions): pkill -f 'valk --socket {} daemon'",
            socket.display(),
            socket.display()
        ),
    }
}

async fn start_or_connect(socket: &Path) -> Result<(Client, Pushes)> {
    if let Ok(conn) = Client::connect(socket).await {
        return Ok(conn);
    }
    let len = socket.as_os_str().len();
    if len > valkyrie_proto::MAX_SOCKET_PATH {
        bail!(
            "socket path {} is {len} bytes, over the {} a unix socket allows; \
             pick a shorter one with --socket or VALK_SOCKET",
            socket.display(),
            valkyrie_proto::MAX_SOCKET_PATH
        );
    }
    for old in valkyrie_proto::legacy_socket_paths() {
        // Checked first: connecting creates the socket's directory.
        if old != socket && old.exists() && Client::connect(&old).await.is_ok() {
            eprintln!(
                "note: an older daemon is still running on {} with its own \
                 sessions; this starts a new one. Stop the old one when done with them: \
                 pkill -f 'overseer --socket {} daemon'",
                old.display(),
                old.display()
            );
        }
    }
    let log_dir = valkyrie_proto::state_dir();
    std::fs::create_dir_all(&log_dir)?;
    // The log records full command lines, so keep it private like the transcripts.
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log_dir.join("daemon.log"))?;
    let mut daemon = std::process::Command::new(std::env::current_exe()?);
    daemon
        .arg("--socket")
        .arg(socket)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // Its own session, like herdr's server: closing the terminal or SSH connection
    // that started it must not take the sessions down with it.
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        daemon.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    daemon.spawn().context("start daemon")?;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        if let Ok(conn) = Client::connect(socket).await {
            return Ok(conn);
        }
    }
    bail!(
        "daemon did not start; see {}",
        log_dir.join("daemon.log").display()
    )
}

fn unescape(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b != b'\\' {
            out.push(b);
            continue;
        }
        match bytes.next() {
            Some(b'r') => out.push(b'\r'),
            Some(b'n') => out.push(b'\n'),
            Some(b't') => out.push(b'\t'),
            Some(b'e') => out.push(0x1b),
            Some(b'\\') => out.push(b'\\'),
            Some(b'x') => {
                let hex: Vec<u8> = bytes.by_ref().take(2).collect();
                let hex = std::str::from_utf8(&hex)?;
                out.push(u8::from_str_radix(hex, 16).with_context(|| format!("bad \\x{hex}"))?);
            }
            other => bail!("unknown escape \\{}", other.map(char::from).unwrap_or(' ')),
        }
    }
    Ok(out)
}

/// Logs to stderr, at `info` unless `RUST_LOG` says otherwise.
fn init_logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();
}

#[cfg(test)]
mod tests {
    #[test]
    fn unescape() {
        assert_eq!(
            super::unescape(r"a\e[B\r\x1d\\").unwrap(),
            b"a\x1b[B\r\x1d\\"
        );
        assert!(super::unescape(r"\q").is_err());
    }
}

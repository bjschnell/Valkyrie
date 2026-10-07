mod bench;
mod hook;
mod tools;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use overseer_proto::client::{Client, Pushes};
use overseer_proto::{SessionId, Size, SpawnSpec};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

#[derive(Parser)]
#[command(version, about = "Attention-first manager for coding-agent CLIs")]
struct Cli {
    /// Daemon socket path.
    #[arg(long, global = true, env = "OVERSEER_SOCKET")]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the daemon in the foreground.
    Daemon,
    /// Spawn a session: `overseer new -- claude`.
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
    /// Print a session's visible screen as plain text.
    Dump { session: SessionId },
    /// Send input to a session. Understands \r \n \t \e and \xHH escapes.
    Send { session: SessionId, text: String },
    /// Forward an agent hook event (stdin JSON) to the daemon. Agents run this; it
    /// prints nothing, always exits 0, and does nothing outside an overseer session.
    #[command(hide = true)]
    Hook { agent: String },
    /// Install agent integrations.
    #[command(subcommand)]
    Setup(SetupCmd),
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
enum SetupCmd {
    /// Add overseer's hooks to ~/.codex/hooks.json (then trust them once with /hooks).
    Codex {
        /// Print the resulting hooks.json instead of writing it.
        #[arg(long)]
        dry_run: bool,
        /// Remove overseer's hooks instead.
        #[arg(long)]
        remove: bool,
        /// The overseer binary the hooks run (default: this one).
        #[arg(long)]
        exe: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // A hook must never print or exit nonzero (exit 2 blocks the agent), even
        // when its arguments or environment don't parse.
        Err(_) if std::env::args().any(|a| a == "hook") => return Ok(()),
        Err(e) => e.exit(),
    };
    let socket = cli
        .socket
        .unwrap_or_else(overseer_proto::default_socket_path);
    // Hooks run on every agent event: skip the async runtime and never fail.
    if let Some(Cmd::Hook { agent }) = &cli.cmd {
        hook::run(agent, &socket);
        return Ok(());
    }
    tokio::runtime::Runtime::new()?.block_on(run(cli.cmd, socket))
}

async fn run(cmd: Option<Cmd>, socket: PathBuf) -> Result<()> {
    match cmd {
        Some(Cmd::Daemon) => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "info".into()),
                )
                .with_writer(std::io::stderr)
                .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
                .init();
            let exe = std::env::current_exe()?;
            overseer_daemon::run(&socket, &overseer_proto::state_dir(), &exe).await
        }
        None => {
            require_tty()?;
            let (client, pushes) = connect(&socket).await?;
            overseer_tui::run(client, pushes, None).await
        }
        Some(Cmd::Attach { session }) => {
            require_tty()?;
            let (client, pushes) = connect(&socket).await?;
            overseer_tui::run(client, pushes, session).await
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
            // A terminal without a real size (`ssh host overseer new …`, scripts)
            // reports 0×0; the first attach resizes the session anyway.
            let size = overseer_tui::session_size()
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
            let info = client
                .spawn(SpawnSpec {
                    command,
                    cwd,
                    name,
                    size,
                    env: overseer_proto::login_env(),
                })
                .await?;
            if attach {
                overseer_tui::run(client, pushes, Some(info.id)).await
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
        Some(Cmd::Kill { session }) => connect(&socket).await?.0.kill(session).await,
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
        Some(Cmd::Hook { .. }) => unreachable!("handled before the runtime starts"),
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
async fn connect(socket: &Path) -> Result<(Client, Pushes)> {
    let conn = start_or_connect(socket).await?;
    match conn.0.hello().await {
        Ok(p) if p == overseer_proto::PROTOCOL => Ok(conn),
        Ok(p) => bail!(
            "the running daemon speaks protocol {p}, this overseer {}; restart the daemon",
            overseer_proto::PROTOCOL
        ),
        // Daemons before protocol 2 hang up on `Hello`.
        Err(_) => bail!(
            "the running daemon on {} is older than this overseer. Stop it to upgrade \
             (this ends its sessions): pkill -f 'overseer.*daemon'",
            socket.display()
        ),
    }
}

async fn start_or_connect(socket: &Path) -> Result<(Client, Pushes)> {
    if let Ok(conn) = Client::connect(socket).await {
        return Ok(conn);
    }
    let len = socket.as_os_str().len();
    if len > overseer_proto::MAX_SOCKET_PATH {
        bail!(
            "socket path {} is {len} bytes, over the {} a unix socket allows; \
             pick a shorter one with --socket or OVERSEER_SOCKET",
            socket.display(),
            overseer_proto::MAX_SOCKET_PATH
        );
    }
    for old in overseer_proto::legacy_socket_paths() {
        if old != socket && Client::connect(&old).await.is_ok() {
            eprintln!(
                "note: an older overseer daemon is still running on {} with its own \
                 sessions; this starts a new one. Stop the old one when done with them: \
                 pkill -f 'overseer --socket {} daemon'",
                old.display(),
                old.display()
            );
        }
    }
    let log_dir = overseer_proto::state_dir();
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

mod bench;

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
    #[command(subcommand)]
    Bench(bench::BenchCmd),
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket = cli
        .socket
        .unwrap_or_else(overseer_proto::default_socket_path);
    match cli.cmd {
        Some(Cmd::Daemon) => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "info".into()),
                )
                .with_writer(std::io::stderr)
                .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
                .init();
            overseer_daemon::run(&socket, &overseer_proto::state_dir()).await
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
            let size = overseer_tui::session_size().unwrap_or(Size {
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
                    None => "running".into(),
                    Some(Some(code)) => format!("exited {code}"),
                    Some(None) => "exited".into(),
                };
                println!(
                    "{:>3}  {:<14} {:<10} {}  {}",
                    s.id,
                    s.name,
                    state,
                    s.command.join(" "),
                    s.cwd.display()
                );
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
    if let Ok(conn) = Client::connect(socket).await {
        return Ok(conn);
    }
    let log_dir = overseer_proto::state_dir();
    std::fs::create_dir_all(&log_dir)?;
    // The log records full command lines, so keep it private like the transcripts.
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log_dir.join("daemon.log"))?;
    std::process::Command::new(std::env::current_exe()?)
        .arg("--socket")
        .arg(socket)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .context("start daemon")?;
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

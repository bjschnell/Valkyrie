//! The shell without its window: what it would load, and what the tray and the
//! notification centre would be told. For trying the core against a daemon.
//!
//!     cargo run -p valkyrie-desktop --example headless -- [--socket PATH] [--setup] [--pair]
//!
//! Without `--setup` nothing is installed: a machine with no `valk web` running
//! is reported, not changed. `--pair` asks for the launch's pairing link, twice.

use anyhow::Result;
use std::net::SocketAddr;
use std::path::PathBuf;
use valkyrie_desktop::pair::Pairing;
use valkyrie_desktop::start::{self, Host, Valk};
use valkyrie_web::proof::Local;

/// The real machine, except that it sets nothing up.
struct LookOnly(Valk);

impl Host for LookOnly {
    async fn check(&self, addr: SocketAddr) -> Result<Local> {
        self.0.check(addr).await
    }

    async fn setup(&self, addr: SocketAddr) -> Result<()> {
        anyhow::bail!("nothing answers on {addr}, and --setup wasn't given")
    }

    fn recorded(&self) -> Option<SocketAddr> {
        self.0.recorded()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let socket = args
        .iter()
        .position(|a| a == "--socket")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(valkyrie_proto::default_socket_path);

    let valk = Valk::find();
    println!("valk: {}", valk.exe.display());
    let found = if has("--setup") {
        start::ensure(&valk).await
    } else {
        start::ensure(&LookOnly(valk)).await
    };
    let addr = match found {
        Ok(addr) => addr,
        Err(e) => {
            println!("can't start: {e}");
            return Ok(());
        }
    };
    let url = start::url(addr);
    println!("window loads: {url}");

    if has("--pair") {
        let pairing = Pairing::new();
        for attempt in ["first", "second"] {
            match pairing.link(&socket, &url).await {
                // The code is a secret: say that there is one, not what it is.
                Ok(Some(link)) => println!(
                    "{attempt} ask: a link to {}",
                    link.split('#').next().unwrap_or_default()
                ),
                Ok(None) => println!("{attempt} ask: none, this launch had its one"),
                Err(e) => println!("{attempt} ask: refused: {e:#}"),
            }
        }
    }

    let (events, mut seen) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move { valkyrie_desktop::watch::run(&socket, || false, events).await });
    while let Some(event) = seen.recv().await {
        println!("{event:?}");
    }
    Ok(())
}

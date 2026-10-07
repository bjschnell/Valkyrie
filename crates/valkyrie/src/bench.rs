//! Built-in measurements for the M0 exit criteria (DESIGN §13, ADR-0002).

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use valkyrie_proto::{ServerMsg, Size, SpawnSpec};
use valkyrie_term::VtScreen;

#[derive(Subcommand)]
pub enum BenchCmd {
    /// Keystroke → screen-update round trip through the daemon, PTY, and a raw-mode echo program.
    Latency {
        #[arg(short, default_value_t = 2000)]
        n: usize,
    },
    /// VT parse + diff throughput over a recorded transcript (`~/.local/state/valkyrie/sessions/*.raw`).
    Parse {
        file: PathBuf,
        #[arg(long, default_value_t = 200)]
        cols: u16,
        #[arg(long, default_value_t = 50)]
        rows: u16,
    },
}

pub async fn run(cmd: BenchCmd, socket: &Path) -> Result<()> {
    match cmd {
        BenchCmd::Latency { n } => latency(socket, n).await,
        BenchCmd::Parse { file, cols, rows } => parse(&file, Size { cols, rows }),
    }
}

async fn latency(socket: &Path, n: usize) -> Result<()> {
    let (client, mut pushes) = super::connect(socket).await?;
    let size = Size { cols: 80, rows: 24 };
    // `stty raw -echo; cat` makes the program, not the line discipline, echo each byte,
    // which matches how agent TUIs (raw mode, app-side echo) behave.
    let spec = SpawnSpec {
        command: vec![
            "sh".into(),
            "-c".into(),
            "stty raw -echo; printf ready; exec cat".into(),
        ],
        cwd: None,
        name: Some("bench-latency".into()),
        size,
        env: Vec::new(),
    };
    let session = client.spawn(spec).await?.id;
    client.attach(session, size).await?;

    let mut cursor_x = None;
    let next_cursor = async |pushes: &mut valkyrie_proto::client::Pushes| -> Result<u16> {
        loop {
            match tokio::time::timeout(Duration::from_secs(2), pushes.recv()).await {
                Ok(Some(ServerMsg::Screen { update, .. })) => return Ok(update.cursor.x),
                Ok(Some(ServerMsg::Exited { .. })) => bail!("echo program exited"),
                Ok(Some(_)) => {}
                Ok(None) => bail!("daemon connection closed"),
                Err(_) => bail!("timed out waiting for echo"),
            }
        }
    };
    // Wait for "ready" so stty has applied before timing.
    while cursor_x != Some(5) {
        cursor_x = Some(next_cursor(&mut pushes).await?);
    }

    let mut samples = Vec::with_capacity(n);
    let mut x = 5u16;
    for i in 0..n {
        if x >= 70 {
            client.input(session, b"\r".to_vec())?;
            while next_cursor(&mut pushes).await? != 0 {}
            x = 0;
        }
        let byte = b'a' + (i % 26) as u8;
        let start = Instant::now();
        client.input(session, vec![byte])?;
        loop {
            let got = next_cursor(&mut pushes).await?;
            if got == x + 1 {
                break;
            }
        }
        samples.push(start.elapsed());
        x += 1;
    }
    let _ = client.kill(session).await;

    samples.sort();
    let pct = |p: f64| samples[((samples.len() - 1) as f64 * p) as usize];
    println!(
        "keystroke→screen update, n={n}: p50 {:?}  p90 {:?}  p99 {:?}  max {:?}",
        pct(0.50),
        pct(0.90),
        pct(0.99),
        samples[samples.len() - 1]
    );
    println!("(daemon path only: excludes the client's own terminal render)");
    Ok(())
}

fn parse(file: &Path, size: Size) -> Result<()> {
    let data = std::fs::read(file).with_context(|| format!("read {}", file.display()))?;
    if data.is_empty() {
        bail!("{} is empty", file.display());
    }
    let mut screen = VtScreen::new(size);
    let mut diff_rows = 0usize;
    let start = Instant::now();
    // Same chunking as the session reader so diff cost is counted realistically.
    for chunk in data.chunks(64 * 1024) {
        screen.feed(chunk);
        if let Some(update) = screen.take_diff() {
            diff_rows += update.rows.len();
        }
    }
    let elapsed = start.elapsed();
    let mb = data.len() as f64 / (1024.0 * 1024.0);
    println!(
        "{:.2} MiB in {elapsed:?} → {:.1} MiB/s ({} diff rows, {}x{})",
        mb,
        mb / elapsed.as_secs_f64(),
        diff_rows,
        size.cols,
        size.rows
    );
    Ok(())
}

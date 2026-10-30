//! `valk worktree`: the worktrees sessions and backlog launches made (DESIGN §4.3).

use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Subcommand)]
pub enum WorktreeCmd {
    /// Take away this repository's worktrees whose branch is merged into origin's
    /// default branch (or the main checkout's, when it's on that branch), squash
    /// merges included, with the
    /// branch, unless a session runs in one, a backlog item is on it, it has no
    /// commits of its own, or it has changes. Only says what it would do without
    /// --yes; either way it first runs `git fetch --prune origin`, with your
    /// credentials.
    Prune {
        /// Take them away.
        #[arg(short, long)]
        yes: bool,
    },
}

/// Fetches `origin` here, with the user's own credentials, then has the daemon
/// judge: it knows which sessions run where and which items own a checkout.
pub async fn run(socket: &Path, cmd: WorktreeCmd) -> Result<()> {
    let WorktreeCmd::Prune { yes } = cmd;
    let cwd = std::env::current_dir()?;
    let remote = fetch(&cwd);
    let client = crate::connect(socket)
        .await
        .context(
            "can't ask the daemon whether a session runs in a worktree, so nothing was removed",
        )?
        .0;
    let (against, worktrees) = client.prune_worktrees(cwd, remote, yes).await?;
    if worktrees.is_empty() {
        println!("no worktrees besides the main checkout");
        return Ok(());
    }
    if against.is_empty() {
        println!("no default branch to judge against: nothing counts as merged");
    } else {
        println!("merged means in {}", against.join(" or "));
    }
    for w in &worktrees {
        let branch = w.branch.as_deref().unwrap_or("detached");
        let path = w.path.display();
        match (w.prunable, yes) {
            // What taking it away did, which names it.
            (true, true) if w.removed => {
                println!("  {}{}", w.why, ignored("with it went", &w.ignored))
            }
            (true, true) => println!("  {}", w.why),
            (true, false) => println!(
                "  would go  {path} ({branch}): {}{}",
                w.why,
                ignored("also deletes", &w.ignored)
            ),
            (false, _) => println!("  keeps     {path} ({branch}): {}", w.why),
        }
    }
    let can = worktrees.iter().filter(|w| w.prunable).count();
    if !yes && can > 0 {
        println!("{can} may go: `valk worktree prune --yes` takes them away");
    }
    Ok(())
}

/// `; also deletes ignored: target/, .env, +3 more`, for what `git worktree remove`
/// takes without asking.
fn ignored(verb: &str, entries: &[String]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut shown = entries[..entries.len().min(3)].join(", ");
    if entries.len() > 3 {
        shown += &format!(", +{} more", entries.len() - 3);
    }
    format!("; {verb} ignored: {shown}")
}

/// `git fetch --prune origin`, quietly; whether `origin`'s branches are fresh enough
/// to judge merged against. Without them only the main checkout's `HEAD` counts.
fn fetch(dir: &Path) -> bool {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .output()
    };
    match git(&["remote", "get-url", "origin"]) {
        Ok(out) if out.status.success() => {}
        _ => {
            eprintln!("no remote origin: judging against this repository's HEAD only");
            return false;
        }
    }
    match git(&["fetch", "--prune", "--quiet", "origin"]) {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let err = err.lines().next().unwrap_or("").trim();
            eprintln!("couldn't fetch origin ({err}): judging against HEAD only");
            false
        }
        Err(e) => {
            eprintln!("couldn't run git ({e}): judging against HEAD only");
            false
        }
    }
}

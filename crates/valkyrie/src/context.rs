//! `valk decide`, `valk decisions` and the context hook (ADR-0007).

use anyhow::{Context, Result};
use clap::Subcommand;
use std::io::Read;
use std::path::{Path, PathBuf};
use valkyrie_proto::client::Client;
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus, NewDecision, ReviewAction};

#[derive(Subcommand)]
pub enum DecisionsCmd {
    /// Make a proposed decision active: agents get it from their next session start.
    Accept { id: u32 },
    /// Turn a proposal down.
    Reject { id: u32 },
    /// Retire an active decision that no longer holds.
    Retire { id: u32 },
    /// Show one decision in full, with where it came from.
    Show { id: u32 },
    /// Print the block agents get at session start.
    Preview,
    /// Copy the active decisions into <repo>/.valkyrie/decisions/ for git.
    Export,
}

pub fn parse_kind(s: &str) -> Result<DecisionKind, String> {
    DecisionKind::parse(s).ok_or_else(|| {
        let all: Vec<_> = DecisionKind::ALL.iter().map(|k| k.as_str()).collect();
        format!("one of {}", all.join(", "))
    })
}

pub struct Decide {
    pub title: String,
    pub body: Option<String>,
    pub kind: DecisionKind,
    pub propose: bool,
    pub supersedes: Option<u32>,
}

pub async fn decide(client: &Client, args: Decide) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let body = match args.body.as_deref() {
        Some("-") => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text)?;
            text
        }
        Some(body) => body.to_owned(),
        None => String::new(),
    };
    let d = client
        .decide(NewDecision {
            commit: head_commit(&cwd),
            cwd,
            title: args.title,
            body,
            kind: args.kind,
            propose: args.propose,
            supersedes: args.supersedes,
            session: std::env::var("VALK_SESSION")
                .ok()
                .and_then(|s| s.parse().ok()),
        })
        .await?;
    match d.status {
        DecisionStatus::Active => println!("#{} recorded: {}", d.id, d.title),
        _ => println!(
            "#{} proposed: {} (the user reviews it in Valkyrie before it applies)",
            d.id, d.title
        ),
    }
    Ok(())
}

pub async fn decisions(client: &Client, all: bool, cmd: Option<DecisionsCmd>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root = valkyrie_context::project::root(&cwd);
    let review = async |id, action| -> Result<()> {
        let d = client.review(root.clone(), id, action).await?;
        println!("#{} {}: {}", d.id, d.status.as_str(), d.title);
        Ok(())
    };
    match cmd {
        Some(DecisionsCmd::Accept { id }) => review(id, ReviewAction::Accept).await,
        Some(DecisionsCmd::Reject { id }) => review(id, ReviewAction::Reject).await,
        Some(DecisionsCmd::Retire { id }) => review(id, ReviewAction::Retire).await,
        Some(DecisionsCmd::Show { id }) => {
            let list = client.decisions(Some(cwd)).await?;
            let d = list
                .iter()
                .find(|d| d.id == id)
                .with_context(|| format!("no decision #{id} in {}", root.display()))?;
            print!("{}", valkyrie_context::render(d));
            Ok(())
        }
        Some(DecisionsCmd::Preview) => {
            let list = client.decisions(Some(cwd)).await?;
            let valk = valk_command();
            print!(
                "{}",
                valkyrie_context::inject::block(
                    &root,
                    &list,
                    valkyrie_context::inject::BUDGET,
                    &valk
                )
            );
            Ok(())
        }
        Some(DecisionsCmd::Export) => {
            // The daemon's store, as the daemon would read it.
            let store = valkyrie_context::Store::new(store_base());
            let checkout = valkyrie_context::project::checkout(&cwd);
            for path in store.export(&root, &checkout)? {
                println!("{}", path.display());
            }
            Ok(())
        }
        None => {
            let list = client.decisions(Some(cwd)).await?;
            print_list(&root, &list, all);
            Ok(())
        }
    }
}

fn print_list(root: &Path, list: &[Decision], all: bool) {
    let shown: Vec<&Decision> = list
        .iter()
        .filter(|d| all || matches!(d.status, DecisionStatus::Active | DecisionStatus::Proposed))
        .collect();
    if shown.is_empty() {
        println!(
            "no decisions for {} yet: `valk decide \"<title>\" \"<why>\"`",
            root.display()
        );
        return;
    }
    for d in shown {
        let status = match d.status {
            DecisionStatus::Active => String::new(),
            other => format!(" ({})", other.as_str()),
        };
        println!(
            "{:>4}  {:<10} {}{status}",
            format!("#{}", d.id),
            d.kind.as_str(),
            d.title
        );
    }
}

/// `HEAD` of the checkout `dir` is in, short; `None` outside git.
fn head_commit(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !sha.is_empty()).then_some(sha)
}

/// The store the daemon that started this session uses, else the default one.
fn store_base() -> PathBuf {
    std::env::var_os("VALK_CONTEXT")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(valkyrie_context::Store::default_base)
}

/// How an agent should run Valkyrie: `valk` when that is this binary on `$PATH`,
/// else this binary's full path.
fn valk_command() -> String {
    let Ok(me) = std::env::current_exe().and_then(std::fs::canonicalize) else {
        return "valk".into();
    };
    let on_path = std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .any(|dir| std::fs::canonicalize(dir.join("valk")).is_ok_and(|p| p == me))
    });
    if on_path {
        "valk".into()
    } else {
        me.to_string_lossy().into_owned()
    }
}

/// `valk context-hook claude`: Claude Code's `SessionStart` hook. Prints the
/// project's decisions as added context. Like `valk hook` it never fails and never
/// prints anything else; outside a Valkyrie session it prints nothing.
pub fn hook(agent: &str) {
    if agent != "claude" || std::env::var_os("VALK_SESSION").is_none() {
        let _ = std::io::copy(&mut std::io::stdin().take(1 << 20), &mut std::io::sink());
        return;
    }
    let mut raw = Vec::new();
    if std::io::stdin()
        .take(1 << 20)
        .read_to_end(&mut raw)
        .is_err()
    {
        return;
    }
    let payload: serde_json::Value = serde_json::from_slice(&raw).unwrap_or_default();
    let cwd = payload["cwd"]
        .as_str()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let Some(cwd) = cwd else {
        return;
    };
    let root = valkyrie_context::project::root(&cwd);
    let decisions = valkyrie_context::Store::new(store_base()).load(&root);
    let text = valkyrie_context::inject::block(
        &root,
        &decisions,
        valkyrie_context::inject::BUDGET,
        &valk_command(),
    );
    println!("{}", valkyrie_context::inject::claude_hook_output(&text));
}

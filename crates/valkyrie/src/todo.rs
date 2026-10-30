//! `valk todo`: the backlog from the command line (DESIGN §8.12).

use anyhow::{Result, bail};
use clap::Subcommand;
use std::path::PathBuf;
use valkyrie_proto::client::Client;
use valkyrie_proto::{Item, ItemChange, ItemKind, ItemStatus, NewItem, SessionInfo, Size};

#[derive(Subcommand)]
pub enum TodoCmd {
    /// Write something down for later. It's for the repository you're in, unless
    /// you say otherwise.
    Add {
        /// What it is; the first line is the title. `-` reads it from stdin.
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
        #[arg(short, long, default_value = "feature", value_parser = parse_kind)]
        kind: ItemKind,
        /// The repository it's for (default: the one you're in).
        #[arg(short = 'C', long, value_name = "DIR", conflicts_with = "no_folder")]
        dir: Option<PathBuf>,
        /// Not for any repository yet.
        #[arg(long)]
        no_folder: bool,
        /// A screenshot or file to keep with it; repeat for more.
        #[arg(short, long = "file", value_name = "FILE")]
        files: Vec<PathBuf>,
        /// The detail, the text then being only the title.
        #[arg(short, long)]
        body: Option<String>,
    },
    /// The open and proposed items (with --all, shipped and dropped ones too).
    List {
        #[arg(long)]
        all: bool,
        /// As JSON, every field, for a script or an agent to read.
        #[arg(long)]
        json: bool,
    },
    /// Accept an item an agent proposed, so it can be started.
    Accept { id: u32 },
    /// Reword it, change its kind, or move it to another repository. What you
    /// don't give stays as it is.
    #[command(arg_required_else_help = true)]
    Edit {
        id: u32,
        #[arg(short, long)]
        title: Option<String>,
        /// The detail; `-` reads it from stdin, `""` clears it.
        #[arg(short, long)]
        body: Option<String>,
        #[arg(short, long, value_parser = parse_kind)]
        kind: Option<ItemKind>,
        /// The repository it's for.
        #[arg(short = 'C', long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
    /// Leave a handoff: what changed, what's left, what to check. It shows on the
    /// item and goes into the brief of its next launch.
    Note {
        /// What happened; `-` reads it from stdin.
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
        /// The item (default: the one this session was launched for).
        #[arg(short, long = "item", env = "VALK_ITEM", value_name = "ID")]
        id: u32,
    },
    /// One item in full (default: the one this session was launched for).
    Show {
        #[arg(env = "VALK_ITEM")]
        id: u32,
    },
    /// Start an agent on an item, in a worktree of its own on a branch named after
    /// it. Prints the session's id.
    Launch {
        id: u32,
        #[arg(long, default_value = "claude", value_parser = ["claude", "codex"])]
        agent: String,
        /// Work in the repository itself instead of a new worktree.
        #[arg(long)]
        no_worktree: bool,
        /// Attach to the new session.
        #[arg(short, long)]
        attach: bool,
    },
    /// Mark it done. Takes away the worktree its launch made, unless it has changes,
    /// and the branch, if it's merged.
    Ship { id: u32 },
    /// Not going to happen.
    Drop { id: u32 },
    /// Open a shipped or dropped item again.
    Reopen { id: u32 },
    /// Forget it for good, attachments and all: for one written down by mistake.
    /// Takes away its worktree as `ship` does, and refuses if that has changes.
    #[command(alias = "delete")]
    Rm { id: u32 },
}

pub fn parse_kind(s: &str) -> Result<ItemKind, String> {
    ItemKind::parse(s).ok_or_else(|| "bug, feature or chore".into())
}

/// Runs `cmd`; `Launch --attach` hands back the session to attach to.
pub async fn run(client: &Client, cmd: Option<TodoCmd>) -> Result<Option<SessionInfo>> {
    match cmd.unwrap_or(TodoCmd::List {
        all: false,
        json: false,
    }) {
        TodoCmd::Add {
            text,
            kind,
            dir,
            no_folder,
            files,
            body,
        } => {
            let mut text = text.join(" ");
            if text == "-" {
                text.clear();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
            }
            let cwd = match (dir, no_folder) {
                (_, true) => None,
                (Some(dir), _) => Some(std::path::absolute(dir)?),
                (None, _) => {
                    let here = std::env::current_dir()?;
                    valkyrie_context::project::in_git(&here).then_some(here)
                }
            };
            let attachments = files
                .into_iter()
                .map(std::path::absolute)
                .collect::<std::io::Result<_>>()?;
            let item = client
                .add_item(NewItem {
                    title: if body.is_some() {
                        text.clone()
                    } else {
                        String::new()
                    },
                    body: body.unwrap_or(text),
                    kind,
                    cwd,
                    attachments,
                })
                .await?;
            println!("#{} {} ({})", item.id, item.title, folder(&item));
            Ok(None)
        }
        TodoCmd::List { all, json } => {
            let items = client.items().await?;
            let items: Vec<Item> = items
                .into_iter()
                .filter(|i| all || matches!(i.status, ItemStatus::Open | ItemStatus::Proposed))
                .collect();
            if json {
                println!("{}", serde_json::to_string_pretty(&items)?);
                return Ok(None);
            }
            let sessions = client.list().await?;
            let boot = client.hello_info().await?.boot;
            let shown = &items;
            if shown.is_empty() {
                println!("nothing written down; `valk todo add \"…\"` adds one");
            }
            for i in shown {
                println!(
                    "#{:<4} {:<8} {:<12} {:<14} {}",
                    i.id,
                    i.kind.as_str(),
                    lane(i, &sessions, boot),
                    short(&folder(i), 14),
                    i.title
                );
            }
            Ok(None)
        }
        TodoCmd::Show { id } => {
            let Some(i) = client.items().await?.into_iter().find(|i| i.id == id) else {
                bail!("no item #{id}");
            };
            println!(
                "#{} {} [{} · {} · by {}]",
                i.id,
                i.title,
                i.kind.as_str(),
                i.status.as_str(),
                i.by
            );
            println!("folder: {}", folder(&i));
            if let Some(b) = &i.branch {
                let at = i.checkout.as_ref().map(|c| format!(" in {}", c.display()));
                println!("branch: {b}{}", at.unwrap_or_default());
            }
            if !i.body.is_empty() {
                println!("\n{}", i.body);
            }
            for a in &i.attachments {
                println!("attached: {}", a.display());
            }
            if let Some(f) = client.item_facts(id).await? {
                println!("\n{}", facts(&f));
                for c in &f.commits {
                    println!("  {c}");
                }
            }
            for n in &i.notes {
                let by = if n.by == "human" { "you" } else { &n.by };
                println!("\nnote from {by}:\n{}", n.text);
            }
            Ok(None)
        }
        TodoCmd::Edit {
            id,
            title,
            body,
            kind,
            dir,
        } => {
            if title.is_none() && body.is_none() && kind.is_none() && dir.is_none() {
                bail!("nothing to change: give --title, --body, --kind or --dir");
            }
            let Some(i) = client.items().await?.into_iter().find(|i| i.id == id) else {
                bail!("no item #{id}");
            };
            let body = match body.as_deref() {
                Some("-") => {
                    let mut text = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
                    text
                }
                Some(b) => b.to_owned(),
                None => i.body,
            };
            let change = ItemChange::Edit {
                title: title.unwrap_or(i.title),
                body,
                kind: kind.unwrap_or(i.kind),
                cwd: dir.map(std::path::absolute).transpose()?,
            };
            let (item, _) = client.change_item(id, change).await?;
            println!(
                "#{} {} [{}] ({})",
                item.id,
                item.title,
                item.kind.as_str(),
                folder(&item)
            );
            Ok(None)
        }
        TodoCmd::Note { text, id } => {
            let mut text = text.join(" ");
            if text == "-" {
                text.clear();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
            }
            let item = client.note_item(id, text).await?;
            println!("noted on #{} {}", item.id, item.title);
            Ok(None)
        }
        TodoCmd::Launch {
            id,
            agent,
            no_worktree,
            attach,
        } => {
            let size = Size {
                cols: 120,
                rows: 40,
            };
            let info = client.launch_item(id, &agent, !no_worktree, size).await?;
            if attach {
                return Ok(Some(info));
            }
            println!("{}", info.id);
            Ok(None)
        }
        TodoCmd::Ship { id } => status(client, id, ItemStatus::Shipped).await,
        TodoCmd::Drop { id } => status(client, id, ItemStatus::Dropped).await,
        TodoCmd::Reopen { id } | TodoCmd::Accept { id } => {
            status(client, id, ItemStatus::Open).await
        }
        TodoCmd::Rm { id } => {
            let (item, note) = client.delete_item(id).await?;
            println!("#{} {}: deleted", item.id, item.title);
            if let Some(note) = note.filter(|n| !n.is_empty()) {
                println!("{note}");
            }
            Ok(None)
        }
    }
}

async fn status(client: &Client, id: u32, status: ItemStatus) -> Result<Option<SessionInfo>> {
    let (item, note) = client
        .change_item(id, ItemChange::Status { status })
        .await?;
    println!("#{} {}: {}", item.id, item.title, item.status.as_str());
    if let Some(note) = note {
        println!("{note}");
    }
    Ok(None)
}

/// `fix/3-x: 2 commits, 3 files changed, 40 insertions(+), 1 not committed`.
fn facts(f: &valkyrie_proto::ItemFacts) -> String {
    let mut parts = vec![match f.ahead {
        1 => "1 commit".to_owned(),
        n => format!("{n} commits"),
    }];
    if !f.diff.is_empty() {
        parts.push(f.diff.clone());
    }
    if f.dirty > 0 {
        parts.push(format!("{} not committed", f.dirty));
    }
    format!("{}: {}", f.branch, parts.join(", "))
}

/// Where an open item is: not started, or what its newest live session is doing.
fn lane(item: &Item, sessions: &[SessionInfo], boot: u64) -> String {
    if item.status != ItemStatus::Open {
        return item.status.as_str().into();
    }
    let live = (item.boot == boot)
        .then(|| {
            item.sessions
                .iter()
                .rev()
                .find_map(|id| sessions.iter().find(|s| s.id == *id && s.exited.is_none()))
        })
        .flatten();
    match live {
        Some(s) => s.status.state.label().into(),
        None if item.checkout.is_some() || !item.sessions.is_empty() => "to review".into(),
        None => "backlog".into(),
    }
}

fn folder(item: &Item) -> String {
    item.project
        .as_deref()
        .and_then(|p| p.file_name())
        .map_or("no folder".into(), |n| n.to_string_lossy().into_owned())
}

fn short(s: &str, n: usize) -> String {
    match s.chars().count() > n {
        true => format!("{}…", s.chars().take(n - 1).collect::<String>()),
        false => s.to_owned(),
    }
}

//! `valk todo`: the backlog from the command line (DESIGN §8.12).

use anyhow::{Result, bail};
use clap::Subcommand;
use std::path::PathBuf;
use valkyrie_proto::client::Client;
use valkyrie_proto::{
    AgentState, Item, ItemChange, ItemKind, ItemStatus, NewItem, OrderProposal, Priority,
    SessionInfo, Size,
};

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
        /// p0 (first) to p3 (whenever); p2 if not given.
        #[arg(short, long, default_value = "p2", value_parser = parse_priority)]
        priority: Priority,
        /// An item to finish before this one; repeat for more.
        #[arg(long, value_name = "ID")]
        after: Vec<u32>,
        /// Also take it for this session: work you're doing that nobody planned,
        /// tracked like a taken item (from inside a valk session).
        #[arg(long)]
        take: bool,
        /// With --take: the branch the work is on.
        #[arg(long, requires = "take")]
        branch: Option<String>,
        /// With --take: the pull request's https:// URL.
        #[arg(long, value_name = "URL", requires = "take")]
        pr: Option<String>,
    },
    /// The open and proposed items (with --all, shipped and dropped ones too), of
    /// every repository unless you pick one.
    List {
        #[arg(long)]
        all: bool,
        /// Only the items for the repository holding DIR (`-C .` for the one you're in).
        #[arg(short = 'C', long, value_name = "DIR", conflicts_with = "no_folder")]
        dir: Option<PathBuf>,
        /// Only the items not for any repository yet.
        #[arg(long)]
        no_folder: bool,
        /// As JSON, every field, for a script or an agent to read.
        #[arg(long)]
        json: bool,
    },
    /// The item to start next: the first open one nobody's on and that waits on
    /// nothing unfinished, by priority, then the oldest.
    Next {
        /// Only the items for the repository holding DIR (`-C .` for the one you're in).
        #[arg(short = 'C', long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
    /// Propose a build order for a repository's open items: what to do first,
    /// why, and the priority and after-links that make the board show it. A model
    /// works it out (`valk decisions auto` must be on). Nothing changes until you
    /// accept it: all of it, or only some items with `--accept ID…`.
    Plan {
        /// The repository holding DIR (default: the one you're in).
        #[arg(short = 'C', long, value_name = "DIR")]
        dir: Option<PathBuf>,
        /// Show the one waiting instead of asking for a new one.
        #[arg(long, conflicts_with_all = ["accept", "decline"])]
        show: bool,
        /// Accept the one waiting: every change, or only those items' changes.
        #[arg(long, value_name = "ID", num_args = 0.., conflicts_with = "decline")]
        accept: Option<Vec<u32>>,
        /// Decline the one waiting: nothing changes.
        #[arg(long)]
        decline: bool,
        /// As JSON.
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
        /// p0 (first) to p3 (whenever).
        #[arg(short, long, value_parser = parse_priority)]
        priority: Option<Priority>,
        /// An item to finish before this one; repeat for more.
        #[arg(long, value_name = "ID")]
        after: Vec<u32>,
        /// No longer wait on this item; repeat for more.
        #[arg(long, value_name = "ID")]
        not_after: Vec<u32>,
    },
    /// Leave a handoff: what changed, what's left, what to check. It shows on the
    /// item and goes into the brief of its next launch.
    Note {
        /// What happened; `-` reads it from stdin.
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
        /// The item (default: the one this session is on).
        #[arg(short, long = "item", env = "VALK_ITEM", value_name = "ID")]
        id: Option<u32>,
    },
    /// One item in full (default: the one this session is on).
    Show {
        #[arg(env = "VALK_ITEM")]
        id: Option<u32>,
    },
    /// Start an agent on an item, in a worktree of its own on a branch named after
    /// it. Prints the session's id.
    Launch {
        id: u32,
        #[arg(long, default_value = "claude", value_parser = ["claude", "codex"])]
        agent: String,
        /// The model to run, as the agent names it (`opus`, `gpt-5-codex`), or
        /// `default` for the agent's own. Without it, the item's last launch of
        /// that agent decides.
        #[arg(short, long)]
        model: Option<String>,
        /// Work in the repository itself instead of a new worktree.
        #[arg(long)]
        no_worktree: bool,
        /// Attach to the new session.
        #[arg(short, long)]
        attach: bool,
    },
    /// Say you're working on an accepted item (default: the one this session is
    /// on), from inside a valk session: its card follows the session, and
    /// `--merged` can ship it once its branch is merged.
    Take {
        #[arg(env = "VALK_ITEM")]
        id: Option<u32>,
        /// The branch the work is on.
        #[arg(short, long)]
        branch: Option<String>,
        /// The pull request's https:// URL.
        #[arg(long, value_name = "URL")]
        pr: Option<String>,
        /// Take it from the session already on it (yours to do, not an agent's).
        #[arg(long)]
        steal: bool,
    },
    /// Turn a live claude or codex session started outside the backlog into an
    /// item: its card follows the session, which stays in its checkout, and its
    /// agent is told on its next prompt. Yours to do, not an agent's.
    Adopt {
        /// The session's id (`valk ls`).
        session: u32,
        /// The title (default: the session's name, else what it was last asked).
        #[arg(short, long)]
        title: Option<String>,
        #[arg(short, long, default_value = "feature", value_parser = parse_kind)]
        kind: ItemKind,
    },
    /// Mark it done. Takes away the worktree its launch made, unless it has changes,
    /// and the branch, if it's merged.
    Ship {
        #[arg(required_unless_present = "merged", conflicts_with = "merged")]
        id: Option<u32>,
        /// Ship every open item whose branch is merged into the default branch,
        /// squash merges included. Only says what it would ship without --yes;
        /// either way it first runs `git fetch --prune origin` in their
        /// repositories, with your credentials.
        #[arg(long)]
        merged: bool,
        /// With --merged: ship them.
        #[arg(short, long, requires = "merged")]
        yes: bool,
    },
    /// Not going to happen.
    Drop { id: u32 },
    /// Open a shipped or dropped item again.
    Reopen { id: u32 },
    /// Forget it for good, attachments and all: for one written down by mistake.
    /// Takes away its worktree as `ship` does, and refuses if that has changes.
    #[command(alias = "delete")]
    Rm { id: u32 },
}

/// `id`, else the item the session this runs in is on: an adopted session has no
/// `VALK_ITEM`.
async fn or_mine(client: &Client, id: Option<u32>) -> Result<u32> {
    match id {
        Some(id) => Ok(id),
        None => Ok(client.my_item().await?.id),
    }
}

pub fn parse_priority(s: &str) -> Result<Priority, String> {
    Priority::parse(s).ok_or_else(|| "p0, p1, p2 or p3".into())
}

/// The repository `-C DIR` names, as items keep it (its main root, which every
/// worktree shares); `Some(None)` for `--no-folder`; `None` for every one.
fn repo_of(dir: Option<PathBuf>, no_folder: bool) -> Result<Option<Option<PathBuf>>> {
    Ok(match (dir, no_folder) {
        (_, true) => Some(None),
        (Some(dir), _) => {
            let dir = std::path::absolute(dir)?;
            if !valkyrie_context::project::in_git(&dir) {
                bail!("{} is not in a git repository", dir.display());
            }
            Some(Some(valkyrie_context::project::root(&dir)))
        }
        (None, _) => None,
    })
}

/// `p1`, then ` after #3, #4` for those it still waits on.
fn rank(item: &Item, items: &[Item]) -> String {
    let waits = item.waiting_on(items);
    if waits.is_empty() {
        return item.priority.as_str().into();
    }
    let ids: Vec<String> = waits.iter().map(|id| format!("#{id}")).collect();
    format!("{} after {}", item.priority.as_str(), ids.join(", "))
}

pub fn parse_kind(s: &str) -> Result<ItemKind, String> {
    ItemKind::parse(s).ok_or_else(|| "bug, feature or chore".into())
}

/// Runs `cmd`; `Launch --attach` hands back the session to attach to.
pub async fn run(client: &Client, cmd: Option<TodoCmd>) -> Result<Option<SessionInfo>> {
    match cmd.unwrap_or(TodoCmd::List {
        all: false,
        dir: None,
        no_folder: false,
        json: false,
    }) {
        TodoCmd::Add {
            text,
            kind,
            dir,
            no_folder,
            files,
            body,
            priority,
            after,
            take,
            branch,
            pr,
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
            let new = NewItem {
                title: if body.is_some() {
                    text.clone()
                } else {
                    String::new()
                },
                body: body.unwrap_or(text),
                kind,
                cwd,
                attachments,
                priority,
                blocked_by: after,
            };
            let item = match take {
                true => client.add_and_take(new, branch, pr).await?,
                false => client.add_item(new).await?,
            };
            let taken = match (take, item.status) {
                (false, _) => "",
                (true, ItemStatus::Proposed) => ", taken; proposed until the user accepts it",
                (true, _) => ", taken",
            };
            println!("#{} {} ({}){taken}", item.id, item.title, folder(&item));
            Ok(None)
        }
        TodoCmd::List {
            all,
            dir,
            no_folder,
            json,
        } => {
            let repo = repo_of(dir, no_folder)?;
            let (every, orders) = client.backlog().await?;
            let mut items: Vec<Item> = every
                .iter()
                .filter(|i| all || matches!(i.status, ItemStatus::Open | ItemStatus::Proposed))
                .filter(|i| repo.as_ref().is_none_or(|r| i.project == *r))
                .cloned()
                .collect();
            // In the order to work them, which is the order a script should too.
            items.sort_by(valkyrie_proto::order(&every));
            if json {
                println!("{}", serde_json::to_string_pretty(&items)?);
                return Ok(None);
            }
            let sessions = client.list().await?;
            let boot = client.hello_info().await?.boot;
            if items.is_empty() {
                match repo {
                    Some(_) => println!("nothing written down for it"),
                    None => println!("nothing written down; `valk todo add \"…\"` adds one"),
                }
            }
            for i in &items {
                println!(
                    "#{:<4} {:<3} {:<8} {:<12} {:<14} {}{}{}{}",
                    i.id,
                    i.priority.as_str(),
                    i.kind.as_str(),
                    lane(i, &sessions, boot),
                    short(&folder(i), 14),
                    i.title,
                    match i.waiting_on(&every).as_slice() {
                        [] => String::new(),
                        ids => format!(
                            " (after {})",
                            ids.iter()
                                .map(|id| format!("#{id}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    },
                    if i.pr.is_some() { " (PR)" } else { "" },
                    if i.agent_filed() {
                        " (agent-filed)"
                    } else {
                        ""
                    }
                );
            }
            for o in orders.iter().filter(|o| {
                repo.as_ref()
                    .is_none_or(|r| r.as_deref() == Some(&*o.project))
            }) {
                println!(
                    "a build order waits for {} ({}): `valk todo plan -C {} --show`",
                    o.project.display(),
                    o.trigger,
                    o.project.display()
                );
            }
            Ok(None)
        }
        TodoCmd::Plan {
            dir,
            show,
            accept,
            decline,
            json,
        } => {
            let dir = std::path::absolute(dir.unwrap_or(std::env::current_dir()?))?;
            let Some(Some(project)) = repo_of(Some(dir.clone()), false)? else {
                unreachable!("repo_of gives a repository for a folder");
            };
            if accept.is_some() || decline || show {
                let (items, orders) = client.backlog().await?;
                let Some(order) = orders.into_iter().find(|o| o.project == project) else {
                    bail!(
                        "no build order waits for {}: `valk todo plan` asks for one",
                        project.display()
                    );
                };
                if show {
                    print_order(&order, &items, json)?;
                    return Ok(None);
                }
                let only = accept.clone().filter(|ids| !ids.is_empty());
                let applied = client
                    .answer_order(project, order.id, accept.is_some(), only)
                    .await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&applied)?);
                } else if accept.is_none() {
                    println!("declined: nothing changed");
                } else {
                    let ids = |v: &[u32]| {
                        v.iter()
                            .map(|i| format!("#{i}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    match applied.changed.as_slice() {
                        [] => println!("nothing changed"),
                        changed => println!("changed {}", ids(changed)),
                    }
                    for (id, why) in &applied.skipped {
                        println!("left #{id} as it is: {why}");
                    }
                }
                return Ok(None);
            }
            if !json {
                eprintln!("asking the model for a build order…");
            }
            let order = client.plan_order(dir).await?;
            let items = client.items().await?;
            print_order(&order, &items, json)?;
            Ok(None)
        }
        TodoCmd::Next { dir } => {
            let repo = repo_of(dir, false)?;
            let every = client.items().await?;
            let sessions = client.list().await?;
            let boot = client.hello_info().await?.boot;
            let mut free: Vec<&Item> = every
                .iter()
                .filter(|i| i.status == ItemStatus::Open && lane(i, &sessions, boot) == "backlog")
                .filter(|i| repo.as_ref().is_none_or(|r| i.project == *r))
                .collect();
            free.sort_by(|a, b| valkyrie_proto::order(&every)(a, b));
            match free.first() {
                Some(i) if i.waiting_on(&every).is_empty() => println!(
                    "#{} {} [{} · {}] ({})",
                    i.id,
                    i.title,
                    i.priority.as_str(),
                    i.kind.as_str(),
                    folder(i)
                ),
                // All of them wait: say on what, since that's where to start.
                Some(_) => bail!(
                    "every open item waits on another: {}",
                    free.iter()
                        .take(5)
                        .map(|i| format!("#{} ({})", i.id, rank(i, &every)))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => bail!("nothing open that nobody's on"),
            }
            Ok(None)
        }
        TodoCmd::Show { id } => {
            let id = or_mine(client, id).await?;
            let Some(i) = client.items().await?.into_iter().find(|i| i.id == id) else {
                bail!("no item #{id}");
            };
            let every = client.items().await?;
            println!(
                "#{} {} [{} · {} · {} · by {}]",
                i.id,
                i.title,
                rank(&i, &every),
                i.kind.as_str(),
                i.status.as_str(),
                i.by
            );
            println!("folder: {}", folder(&i));
            for w in every.iter().filter(|w| i.blocked_by.contains(&w.id)) {
                println!("after: #{} {} ({})", w.id, w.title, w.status.as_str());
            }
            if let Some(b) = &i.branch {
                let at = i.checkout.as_ref().map(|c| format!(" in {}", c.display()));
                println!("branch: {b}{}", at.unwrap_or_default());
            }
            if let Some(pr) = &i.pr {
                println!("pr: {pr}");
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
                let took = if n.taken { " (from a take)" } else { "" };
                println!("\nnote from {by}{took}:\n{}", n.text);
            }
            Ok(None)
        }
        TodoCmd::Edit {
            id,
            title,
            body,
            kind,
            dir,
            priority,
            after,
            not_after,
        } => {
            if title.is_none()
                && body.is_none()
                && kind.is_none()
                && dir.is_none()
                && priority.is_none()
                && after.is_empty()
                && not_after.is_empty()
            {
                bail!(
                    "nothing to change: give --title, --body, --kind, --dir, --priority, --after or --not-after"
                );
            }
            if title.as_deref().is_some_and(|t| t.trim().is_empty()) {
                bail!("the title can't be empty");
            }
            let body = match body {
                Some(b) if b == "-" => {
                    let mut text = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
                    Some(text)
                }
                b => b,
            };
            let change = ItemChange::Edit {
                title,
                body,
                kind,
                cwd: dir.map(std::path::absolute).transpose()?,
                priority,
                blocked_by: blocked_by(client, id, &after, &not_after).await?,
            };
            let (item, _) = client.change_item(id, change).await?;
            let every = client.items().await?;
            println!(
                "#{} {} [{} · {}] ({})",
                item.id,
                item.title,
                rank(&item, &every),
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
            let id = or_mine(client, id).await?;
            let item = client.note_item(id, text).await?;
            println!("noted on #{} {}", item.id, item.title);
            Ok(None)
        }
        TodoCmd::Launch {
            id,
            agent,
            model,
            no_worktree,
            attach,
        } => {
            let size = Size {
                cols: 120,
                rows: 40,
            };
            let model = model.map(|m| if m == "default" { String::new() } else { m });
            let info = client
                .launch_item(id, &agent, !no_worktree, size, model)
                .await?;
            if attach {
                return Ok(Some(info));
            }
            println!("{}", info.id);
            Ok(None)
        }
        TodoCmd::Take {
            id,
            branch,
            pr,
            steal,
        } => {
            let id = or_mine(client, id).await?;
            let item = client.take_item(id, branch, pr, steal).await?;
            let mut has = Vec::new();
            if let Some(b) = &item.branch {
                has.push(format!("branch {b}"));
            }
            if let Some(pr) = &item.pr {
                has.push(format!("PR {pr}"));
            }
            let has = match has.is_empty() {
                true => String::new(),
                false => format!(" ({})", has.join(", ")),
            };
            println!("#{} {} taken{has}", item.id, short(&item.title, 40));
            Ok(None)
        }
        TodoCmd::Adopt {
            session,
            title,
            kind,
        } => {
            let item = client.adopt_session(session, title, kind).await?;
            let branch = item
                .branch
                .as_ref()
                .map(|b| format!(", branch {b}"))
                .unwrap_or_default();
            println!(
                "#{} {} ({}{branch}): session {session} is on it",
                item.id,
                item.title,
                folder(&item)
            );
            Ok(None)
        }
        TodoCmd::Ship {
            merged: true, yes, ..
        } => {
            // The daemon judges against `origin` as last fetched, and can't fetch
            // with the user's credentials itself.
            let items = client.items().await?;
            let mut projects: Vec<PathBuf> = items
                .into_iter()
                .filter(|i| (i.status == ItemStatus::Open || i.agent_filed()) && i.branch.is_some())
                .filter_map(|i| i.project)
                .collect();
            projects.sort();
            projects.dedup();
            for p in projects.iter().filter(|p| p.is_dir()) {
                crate::worktree::fetch(p);
            }
            let (shipped, kept) = client.ship_merged(!yes, None).await?;
            let done = if yes { "shipped" } else { "would ship" };
            for s in &shipped {
                // An agent chose a branch a take set: worth a look before shipping.
                // One it filed itself isn't accepted yet: shipping it accepts it.
                let take = match (s.branch_taken, s.filed) {
                    (_, true) => " (agent-filed, not yet accepted)",
                    (true, false) => " (set by a take)",
                    (false, false) => "",
                };
                println!(
                    "#{} {}: {done} {}{take}, merged into {}",
                    s.id, s.title, s.branch, s.against
                );
                if let Some(note) = &s.note {
                    println!("  {note}");
                }
            }
            for k in &kept {
                println!("#{} {}: kept, {}", k.id, k.title, k.why);
            }
            if shipped.is_empty() && kept.is_empty() {
                println!("no open item has a branch; `valk todo take N --branch B` records one");
            }
            if !yes && !shipped.is_empty() {
                println!(
                    "{} may ship: `valk todo ship --merged --yes` ships them",
                    shipped.len()
                );
            }
            Ok(None)
        }
        TodoCmd::Ship { id, .. } => {
            let Some(id) = id else {
                bail!("which item? give its number, or --merged");
            };
            status(client, id, ItemStatus::Shipped).await
        }
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

/// Item `id`'s list with `after` added and `not_after` taken out, or `None` to
/// leave it alone.
async fn blocked_by(
    client: &Client,
    id: u32,
    after: &[u32],
    not_after: &[u32],
) -> Result<Option<Vec<u32>>> {
    if after.is_empty() && not_after.is_empty() {
        return Ok(None);
    }
    let Some(item) = client.items().await?.into_iter().find(|i| i.id == id) else {
        bail!("no item #{id}");
    };
    let mut ids = item.blocked_by;
    ids.extend(after);
    ids.retain(|b| !not_after.contains(b));
    Ok(Some(ids))
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

/// Where an open item is: not started, or what its newest live session is doing;
/// with a pull request open, waiting on review unless that session needs you (a
/// session holding several items would otherwise show them all at work).
fn lane(item: &Item, sessions: &[SessionInfo], boot: u64) -> String {
    // An agent's own filed work follows its session, though not accepted yet.
    if item.status != ItemStatus::Open && !item.agent_filed() {
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
    let needs =
        |s: &SessionInfo| matches!(s.status.state, AgentState::NeedsInput | AgentState::Blocked);
    match live {
        Some(s) if needs(s) => s.status.state.label().into(),
        _ if item.pr_closed => "PR closed".into(),
        _ if item.pr.is_some() => "PR open".into(),
        Some(s) => s.status.state.label().into(),
        None if item.checkout.is_some() || !item.sessions.is_empty() => "to review".into(),
        None => "backlog".into(),
    }
}

/// A build order, numbered, each item with what accepting it changes and why.
fn print_order(order: &OrderProposal, items: &[Item], json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(order)?);
        return Ok(());
    }
    let name = order.project.file_name().map_or_else(
        || order.project.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    println!("Build order for {name} ({}):", order.trigger);
    for (n, step) in order.steps.iter().enumerate() {
        let item = items.iter().find(|i| i.id == step.item);
        let title = item.map_or("(gone)", |i| i.title.as_str());
        println!("{:>3}. #{:<4} {}", n + 1, step.item, short(title, 60));
        let mut changes = Vec::new();
        if let (Some(p), Some(i)) = (step.priority, item) {
            changes.push(format!("{} → {}", i.priority.as_str(), p.as_str()));
        }
        if let Some(after) = &step.blocked_by {
            let had = item.map(|i| i.blocked_by.as_slice()).unwrap_or_default();
            let added: Vec<String> = after
                .iter()
                .filter(|b| !had.contains(b))
                .map(|b| format!("#{b}"))
                .collect();
            if !added.is_empty() {
                changes.push(format!("after {}", added.join(", ")));
            }
        }
        if !changes.is_empty() {
            println!("        change: {}", changes.join(", "));
        }
        if !step.reason.is_empty() {
            println!("        why: {}", step.reason);
        }
    }
    if order.steps.iter().any(|s| s.change().is_some()) {
        println!(
            "\n`valk todo plan --accept` applies every change, `--accept ID…` only those items', `--decline` none."
        );
    } else {
        println!(
            "\nIt changes nothing: the board already shows this order. `valk todo plan --decline` clears it."
        );
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_request_open_waits_on_review_unless_its_session_needs_you() {
        let item: Item = serde_json::from_value(serde_json::json!({
            "id": 1, "title": "x", "kind": "bug", "status": "open", "by": "human",
            "sessions": [4], "boot": 7, "pr": "https://github.com/o/r/pull/7",
            "created": 0, "updated": 0,
        }))
        .unwrap();
        let session = |state| {
            let mut s: SessionInfo = serde_json::from_value(serde_json::json!({
                "id": 4, "name": "", "command": [], "cwd": "/", "pid": null,
                "created_unix": 0, "title": null, "clients": 0, "exited": null,
            }))
            .unwrap();
            s.status.state = state;
            s
        };
        let lane_with = |state| lane(&item, &[session(state)], 7);
        assert_eq!(lane_with(AgentState::Working), "PR open");
        assert_eq!(lane_with(AgentState::NeedsInput), "needs input");
        assert_eq!(lane(&item, &[], 7), "PR open");
        let plain = Item {
            pr: None,
            ..item.clone()
        };
        let filed = Item {
            status: ItemStatus::Proposed,
            filed_by: Some((7, 4)),
            taken: vec![4],
            ..plain.clone()
        };
        assert_eq!(lane(&filed, &[session(AgentState::Working)], 7), "working");
        // Filed by the agent but not taken yet: a proposal like any other.
        let untaken = Item {
            taken: Vec::new(),
            ..filed.clone()
        };
        assert_eq!(lane(&untaken, &[], 7), "proposed");
        let proposed = Item {
            filed_by: None,
            ..filed.clone()
        };
        assert_eq!(
            lane(&proposed, &[session(AgentState::Working)], 7),
            "proposed"
        );
        assert_eq!(lane(&plain, &[session(AgentState::Working)], 7), "working");
    }
}

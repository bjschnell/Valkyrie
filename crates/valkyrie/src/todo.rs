//! `valk todo`: the backlog from the command line (DESIGN §8.12).

use anyhow::{Result, bail};
use clap::Subcommand;
use std::path::PathBuf;
use valkyrie_proto::client::Client;
use valkyrie_proto::{
    AgentState, Item, ItemChange, ItemKind, ItemStatus, NewItem, Priority, SessionInfo, Size,
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
        #[arg(long, value_name = "N")]
        after: Vec<Ref>,
        /// The item it's a piece of; it goes in that item's repository.
        #[arg(long, value_name = "N", conflicts_with_all = ["dir", "no_folder"])]
        parent: Option<Ref>,
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
        /// Only the items of this kind: bug, feature, chore, spike or debt.
        #[arg(short, long, value_parser = parse_kind)]
        kind: Option<ItemKind>,
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
    /// Accept an item an agent proposed, so it can be started.
    Accept { id: Ref },
    /// Reword it, change its kind, or move it to another repository. What you
    /// don't give stays as it is.
    #[command(arg_required_else_help = true)]
    Edit {
        id: Ref,
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
        #[arg(long, value_name = "N")]
        after: Vec<Ref>,
        /// No longer wait on this item; repeat for more.
        #[arg(long, value_name = "N")]
        not_after: Vec<Ref>,
        /// Make it a piece of this item, in the same repository.
        #[arg(long, value_name = "N", conflicts_with = "no_parent")]
        parent: Option<Ref>,
        /// Make it stand alone again, no longer a piece of another.
        #[arg(long)]
        no_parent: bool,
    },
    /// Leave a note: on your own item, a handoff (what changed, what's left, what
    /// to check) that goes into the brief of its next launch; on any other, with
    /// -i, something found that bears on it, shown on its card but not briefed.
    Note {
        /// What happened; `-` reads it from stdin.
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
        /// The item (default: the one this session is on).
        #[arg(short, long = "item", value_name = "N")]
        id: Option<Ref>,
    },
    /// One item in full (default: the one this session is on).
    Show { id: Option<Ref> },
    /// Start an agent on an item, in a worktree of its own on a branch named after
    /// it. Prints the session's id.
    Launch {
        id: Ref,
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
        id: Option<Ref>,
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
        id: Option<Ref>,
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
    /// Put a started item back in the backlog: stops the sessions on it and
    /// forgets them. Its branch and worktree stay for the next launch. Yours to
    /// do, not an agent's; refused once it has a pull request.
    Return { id: Ref },
    /// Not going to happen.
    Drop { id: Ref },
    /// Open a shipped or dropped item again.
    Reopen { id: Ref },
    /// Forget it for good, attachments and all: for one written down by mistake.
    /// Takes away its worktree as `ship` does, and refuses if that has changes.
    #[command(alias = "delete")]
    Rm { id: Ref },
}

/// `id`, else the item the session this runs in is on: an adopted session has no
/// `VALK_ITEM`.
async fn or_mine(client: &Client, id: Option<Ref>) -> Result<u32> {
    match id {
        Some(r) => Items::load(client).await?.resolve(&r),
        // `VALK_ITEM` holds the global id a launch was started on.
        None => match std::env::var("VALK_ITEM").ok().and_then(|v| v.parse().ok()) {
            Some(id) => Ok(id),
            None => Ok(client.my_item().await?.id),
        },
    }
}

/// An item as typed: `3` or `#3` for one in the repository you're in, `repo#3`
/// for one in another (`no-folder#3` for one with no repository). The number is the
/// per-repository `Item.number`; the global id is the daemon's own key.
#[derive(Clone, Debug, PartialEq)]
pub struct Ref {
    repo: Option<String>,
    number: u32,
}

impl std::str::FromStr for Ref {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let (repo, n) = match s.rsplit_once('#') {
            Some((repo, n)) => (Some(repo).filter(|r| !r.is_empty()), n),
            None => (None, s),
        };
        let number = n
            .parse::<u32>()
            .map_err(|_| format!("`{s}` isn't an item: give 3, #3, or repo#3"))?;
        Ok(Self {
            repo: repo.map(str::to_owned),
            number,
        })
    }
}

/// Every item, and the repository the caller stands in: what a typed number is
/// read against and what a printed one is named for.
struct Items {
    all: Vec<Item>,
    here: Option<PathBuf>,
}

impl Items {
    async fn load(client: &Client) -> Result<Self> {
        let here = std::env::current_dir().ok().and_then(|d| {
            valkyrie_context::project::in_git(&d).then(|| valkyrie_context::project::root(&d))
        });
        Ok(Self {
            all: client.items().await?,
            here,
        })
    }

    /// The global id of the item `r` names.
    fn resolve(&self, r: &Ref) -> Result<u32> {
        let project = match r.repo.as_deref() {
            None => self.here.clone(),
            Some("no-folder") => None,
            Some(name) => {
                let mut found: Vec<&PathBuf> = Vec::new();
                for p in self.all.iter().filter_map(|i| i.project.as_ref()) {
                    if p.file_name().is_some_and(|n| n == name) && !found.contains(&p) {
                        found.push(p);
                    }
                }
                match found.as_slice() {
                    [one] => Some((*one).clone()),
                    [] => bail!("no repository {name} has items"),
                    many => bail!(
                        "{name} is more than one repository: {}",
                        many.iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                }
            }
        };
        self.all
            .iter()
            .find(|i| i.project == project && i.number == r.number)
            .map(|i| i.id)
            .ok_or_else(|| {
                let at = match (&r.repo, &self.here) {
                    (Some(name), _) => name.clone(),
                    (None, Some(p)) => folder_of(Some(p)),
                    (None, None) => "no repository".into(),
                };
                anyhow::anyhow!(
                    "no item #{} in {at}; for another repository's, write repo#{}",
                    r.number,
                    r.number
                )
            })
    }

    fn ids(&self, refs: &[Ref]) -> Result<Vec<u32>> {
        refs.iter().map(|r| self.resolve(r)).collect()
    }

    /// `#3` for an item of the caller's repository, `repo#3` for any other.
    fn tag(&self, item: &Item) -> String {
        if item.project == self.here {
            format!("#{}", item.number)
        } else {
            let repo = item
                .project
                .as_deref()
                .map_or("no-folder".into(), |p| folder_of(Some(p)));
            format!("{repo}#{}", item.number)
        }
    }

    /// `tag` of the item with global id `id`; `#id` for one that's gone.
    fn tag_id(&self, id: u32) -> String {
        match self.all.iter().find(|i| i.id == id) {
            Some(item) => self.tag(item),
            None => format!("#{id}"),
        }
    }

    fn tags(&self, ids: &[u32]) -> String {
        ids.iter()
            .map(|id| self.tag_id(*id))
            .collect::<Vec<_>>()
            .join(", ")
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
fn rank(item: &Item, items: &Items) -> String {
    let waits = item.waiting_on(&items.all);
    if waits.is_empty() {
        return item.priority.as_str().into();
    }
    format!("{} after {}", item.priority.as_str(), items.tags(&waits))
}

/// ` (piece of #3)` for a piece, ` [2/5]` for a group's parent, else nothing.
fn group(item: &Item, items: &Items) -> String {
    if let Some(p) = item.parent {
        return format!(" (piece of {})", items.tag_id(p));
    }
    match item.progress(&items.all) {
        (_, 0) => String::new(),
        (done, of) => format!(" [{done}/{of}]"),
    }
}

pub fn parse_kind(s: &str) -> Result<ItemKind, String> {
    ItemKind::parse(s).ok_or_else(|| "bug, feature, chore, spike or debt".into())
}

/// Runs `cmd`; `Launch --attach` hands back the session to attach to.
pub async fn run(client: &Client, cmd: Option<TodoCmd>) -> Result<Option<SessionInfo>> {
    match cmd.unwrap_or(TodoCmd::List {
        all: false,
        dir: None,
        no_folder: false,
        kind: None,
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
            parent,
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
            let names = Items::load(client).await?;
            let new = NewItem {
                parent: parent.as_ref().map(|p| names.resolve(p)).transpose()?,
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
                blocked_by: names.ids(&after)?,
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
            let items = Items::load(client).await?;
            println!(
                "{} {} ({}){taken}",
                items.tag(&item),
                item.title,
                folder(&item)
            );
            Ok(None)
        }
        TodoCmd::List {
            all,
            dir,
            no_folder,
            kind,
            json,
        } => {
            let repo = repo_of(dir, no_folder)?;
            let every = client.items().await?;
            let names = Items {
                all: every.clone(),
                here: Items::load(client).await?.here,
            };
            let mut items: Vec<Item> = every
                .iter()
                .filter(|i| all || matches!(i.status, ItemStatus::Open | ItemStatus::Proposed))
                .filter(|i| repo.as_ref().is_none_or(|r| i.project == *r))
                .filter(|i| kind.is_none_or(|k| i.kind == k))
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
                match (&repo, kind) {
                    (Some(_), Some(k)) => println!("no {} items written down for it", k.as_str()),
                    (None, Some(k)) => println!("no {} items written down", k.as_str()),
                    (Some(_), None) => println!("nothing written down for it"),
                    (None, None) => {
                        println!("nothing written down; `valk todo add \"…\"` adds one")
                    }
                }
            }
            for i in &items {
                println!(
                    "{:<6} {:<3} {:<8} {:<12} {:<14} {}{}{}{}{}",
                    names.tag(i),
                    i.priority.as_str(),
                    i.kind.as_str(),
                    lane(i, &sessions, boot),
                    short(&folder(i), 14),
                    i.title,
                    group(i, &names),
                    match i.waiting_on(&every).as_slice() {
                        [] => String::new(),
                        ids => format!(" (after {})", names.tags(ids)),
                    },
                    if i.pr.is_some() { " (PR)" } else { "" },
                    if i.agent_filed() {
                        " (agent-filed)"
                    } else {
                        ""
                    }
                );
            }
            Ok(None)
        }
        TodoCmd::Next { dir } => {
            let repo = repo_of(dir, false)?;
            let names = Items::load(client).await?;
            let every = names.all.clone();
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
                    "{} {} [{} · {}] ({})",
                    names.tag(i),
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
                        .map(|i| format!("{} ({})", names.tag(i), rank(i, &names)))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None => bail!("nothing open that nobody's on"),
            }
            Ok(None)
        }
        TodoCmd::Show { id } => {
            let id = or_mine(client, id).await?;
            let every = Items::load(client).await?;
            let Some(i) = every.all.iter().find(|i| i.id == id).cloned() else {
                bail!("no item with id {id}");
            };
            println!(
                "{} {} [{} · {} · {} · by {}]",
                every.tag(&i),
                i.title,
                rank(&i, &every),
                i.kind.as_str(),
                i.status.as_str(),
                i.by
            );
            println!("folder: {}", folder(&i));
            if let Some(p) = i.parent.and_then(|p| every.all.iter().find(|w| w.id == p)) {
                println!(
                    "piece of: {} {} ({})",
                    every.tag(p),
                    p.title,
                    p.status.as_str()
                );
            }
            let pieces = i.children(&every.all);
            if !pieces.is_empty() {
                let (done, of) = i.progress(&every.all);
                println!("pieces: {done}/{of} shipped or dropped");
                for c in pieces {
                    println!("  {} {} ({})", every.tag(c), c.title, c.status.as_str());
                }
            }
            for w in every.all.iter().filter(|w| i.blocked_by.contains(&w.id)) {
                println!(
                    "after: {} {} ({})",
                    every.tag(w),
                    w.title,
                    w.status.as_str()
                );
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
                let origin = n.origin().map(|o| format!(" ({o})")).unwrap_or_default();
                println!("\nnote from {by}{origin}:\n{}", n.text);
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
            parent,
            no_parent,
        } => {
            if title.is_none()
                && body.is_none()
                && kind.is_none()
                && dir.is_none()
                && priority.is_none()
                && after.is_empty()
                && not_after.is_empty()
                && parent.is_none()
                && !no_parent
            {
                bail!(
                    "nothing to change: give --title, --body, --kind, --dir, --priority, --after, --not-after, --parent or --no-parent"
                );
            }
            let names = Items::load(client).await?;
            let id = names.resolve(&id)?;
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
            let parent = match (parent, no_parent) {
                (Some(p), _) => Some(names.resolve(&p)?),
                (None, true) => Some(0),
                (None, false) => None,
            };
            let change = ItemChange::Edit {
                parent,
                title,
                body,
                kind,
                cwd: dir.map(std::path::absolute).transpose()?,
                priority,
                blocked_by: blocked_by(&names, id, &after, &not_after)?,
            };
            let (item, _) = client.change_item(id, change).await?;
            let every = Items::load(client).await?;
            println!(
                "{} {} [{} · {}] ({})",
                every.tag(&item),
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
            println!(
                "noted on {} {}",
                Items::load(client).await?.tag(&item),
                item.title
            );
            Ok(None)
        }
        TodoCmd::Launch {
            id,
            agent,
            model,
            no_worktree,
            attach,
        } => {
            let id = Items::load(client).await?.resolve(&id)?;
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
            println!(
                "{} {} taken{has}",
                Items::load(client).await?.tag(&item),
                short(&item.title, 40)
            );
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
                "{} {} ({}{branch}): session {session} is on it",
                Items::load(client).await?.tag(&item),
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
            let names = Items::load(client).await?;
            let mut projects: Vec<PathBuf> = names
                .all
                .iter()
                .filter(|i| (i.status == ItemStatus::Open || i.agent_filed()) && i.branch.is_some())
                .filter_map(|i| i.project.clone())
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
                    "{} {}: {done} {}{take}, merged into {}",
                    names.tag_id(s.id),
                    s.title,
                    s.branch,
                    s.against
                );
                if let Some(note) = &s.note {
                    println!("  {note}");
                }
            }
            for k in &kept {
                println!("{} {}: kept, {}", names.tag_id(k.id), k.title, k.why);
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
            status(client, &id, ItemStatus::Shipped).await
        }
        TodoCmd::Return { id } => {
            let names = Items::load(client).await?;
            let (item, _) = client
                .change_item(names.resolve(&id)?, ItemChange::Return)
                .await?;
            println!("{} {}: back in the backlog", names.tag(&item), item.title);
            Ok(None)
        }
        TodoCmd::Drop { id } => status(client, &id, ItemStatus::Dropped).await,
        TodoCmd::Reopen { id } | TodoCmd::Accept { id } => {
            status(client, &id, ItemStatus::Open).await
        }
        TodoCmd::Rm { id } => {
            let names = Items::load(client).await?;
            let (item, note) = client.delete_item(names.resolve(&id)?).await?;
            println!("{} {}: deleted", names.tag(&item), item.title);
            if let Some(note) = note.filter(|n| !n.is_empty()) {
                println!("{note}");
            }
            Ok(None)
        }
    }
}

/// Item `id`'s list with `after` added and `not_after` taken out, or `None` to
/// leave it alone.
fn blocked_by(
    names: &Items,
    id: u32,
    after: &[Ref],
    not_after: &[Ref],
) -> Result<Option<Vec<u32>>> {
    if after.is_empty() && not_after.is_empty() {
        return Ok(None);
    }
    let Some(item) = names.all.iter().find(|i| i.id == id) else {
        bail!("no item with id {id}");
    };
    let (after, not_after) = (names.ids(after)?, names.ids(not_after)?);
    let mut ids = item.blocked_by.clone();
    ids.extend(after);
    ids.retain(|b| !not_after.contains(b));
    Ok(Some(ids))
}

async fn status(client: &Client, id: &Ref, status: ItemStatus) -> Result<Option<SessionInfo>> {
    let names = Items::load(client).await?;
    let (item, note) = client
        .change_item(names.resolve(id)?, ItemChange::Status { status })
        .await?;
    println!(
        "{} {}: {}",
        names.tag(&item),
        item.title,
        item.status.as_str()
    );
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
        None if !item.sessions.is_empty() => "to review".into(),
        None => "backlog".into(),
    }
}

fn folder(item: &Item) -> String {
    folder_of(item.project.as_deref())
}

fn folder_of(project: Option<&std::path::Path>) -> String {
    project
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

    fn item(id: u32, number: u32, project: Option<&str>) -> Item {
        serde_json::from_value(serde_json::json!({
            "id": id, "number": number, "title": "x", "by": "human",
            "project": project, "created": 0, "updated": 0,
        }))
        .unwrap()
    }

    #[test]
    fn a_typed_number_means_the_one_in_your_repository() {
        let items = Items {
            all: vec![
                item(1, 1, Some("/r/valkyrie")),
                item(2, 1, Some("/r/other")),
                item(3, 2, Some("/r/valkyrie")),
                item(4, 1, None),
            ],
            here: Some("/r/valkyrie".into()),
        };
        let at = |s: &str| items.resolve(&s.parse().unwrap());
        // `2`, `#2` and `valkyrie#2` are all the same item, which is global id 3.
        assert_eq!(at("2").unwrap(), 3);
        assert_eq!(at("#2").unwrap(), 3);
        assert_eq!(at("valkyrie#2").unwrap(), 3);
        assert_eq!(at("1").unwrap(), 1);
        assert_eq!(at("other#1").unwrap(), 2);
        assert_eq!(at("no-folder#1").unwrap(), 4);
        // No #2 in `other`, and no such repository.
        assert!(at("other#2").is_err());
        assert!(at("nowhere#1").is_err());
        assert!("x".parse::<Ref>().is_err());
        // Named as they are typed.
        assert_eq!(items.tag(&items.all[0]), "#1");
        assert_eq!(items.tag(&items.all[1]), "other#1");
        assert_eq!(items.tag_id(4), "no-folder#1");
        assert_eq!(items.tag_id(99), "#99");
    }

    #[test]
    fn outside_a_repository_only_the_ones_with_no_folder_are_bare() {
        let items = Items {
            all: vec![item(1, 1, Some("/r/valkyrie")), item(2, 1, None)],
            here: None,
        };
        assert_eq!(items.resolve(&"1".parse().unwrap()).unwrap(), 2);
        assert_eq!(items.tag(&items.all[0]), "valkyrie#1");
        assert_eq!(items.tag(&items.all[1]), "#1");
    }

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

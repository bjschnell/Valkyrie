//! `valk decide`, `valk decisions` and the context hook (ADR-0007).

use anyhow::{Context, Result};
use clap::Subcommand;
use std::io::Read;
use std::path::{Path, PathBuf};
use valkyrie_proto::client::Client;
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus, NewDecision, NoWindow, ReviewAction};

#[derive(Subcommand)]
pub enum DecisionsCmd {
    /// Make a proposed decision active: agents get it from their next session start.
    Accept { id: u32 },
    /// Turn a proposal down.
    Reject { id: u32 },
    /// Retire an active decision that no longer holds.
    Retire { id: u32 },
    /// Say an active decision still holds (one flagged as maybe out of date).
    Confirm { id: u32 },
    /// Put an active decision's body in every session's block, not only its
    /// title. Constraints always have theirs; `preview` shows what it costs.
    Pin { id: u32 },
    /// Back to a title in the block, its body read when an agent needs it.
    Unpin { id: u32 },
    /// How this project's decisions are doing: what waits on you, what may be
    /// out of date.
    Health,
    /// Show one decision in full, with where it came from.
    Show { id: u32 },
    /// Print the block agents get at session start.
    Preview,
    /// Copy the active decisions into <repo>/.valkyrie/decisions/ for git.
    Export,
    /// Find what you keep telling agents in this repo's transcripts, and have a
    /// small model propose the decisions that would make repeating it unnecessary.
    /// Codex transcripts are read only when `auto` is `all`.
    Mine {
        /// How many days of transcripts to read.
        #[arg(long, default_value_t = 30)]
        days: u64,
        /// Show what you repeat; send nothing to the model and propose nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Have a small model turn what a session did into a skill: a procedure for the
    /// next agent, or a fix to a skill the session followed. Proposed for review.
    /// Claude sessions only.
    Learn {
        /// The session (default: this one).
        session: Option<valkyrie_proto::SessionId>,
        /// What the procedure is, if the session did more than one thing.
        #[arg(long)]
        about: Option<String>,
    },
    /// Which sessions' corrections to agents a small model reads, to propose the
    /// rules in them: off, claude (the default) or all (Codex sessions too, whose
    /// conversations then also go to Anthropic). Without one, shows which.
    Auto {
        #[arg(value_parser = parse_auto)]
        mode: Option<valkyrie_proto::AutoMode>,
    },
}

fn parse_auto(s: &str) -> Result<valkyrie_proto::AutoMode, String> {
    valkyrie_proto::AutoMode::parse(s).ok_or_else(|| "off, claude or all".into())
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
    pub review_in: Option<u32>,
    /// Into the user's habits (ADR-0016 §1), not the project's.
    pub user: bool,
}

pub async fn decide(client: &Client, args: Decide) -> Result<()> {
    let cwd = match args.user {
        true => valkyrie_context::project::user(),
        false => std::env::current_dir()?,
    };
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
            cwd,
            title: args.title,
            body,
            kind: args.kind,
            propose: args.propose,
            supersedes: args.supersedes,
            review_every: args.review_in,
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

pub async fn decisions(
    client: &Client,
    all: bool,
    user: bool,
    cmd: Option<DecisionsCmd>,
) -> Result<()> {
    let here = std::env::current_dir()?;
    let cwd = match user {
        true => valkyrie_context::project::user(),
        false => here.clone(),
    };
    let root = valkyrie_context::project::root(&cwd);
    if user
        && matches!(
            cmd,
            Some(
                DecisionsCmd::Preview
                    | DecisionsCmd::Pin { .. }
                    | DecisionsCmd::Unpin { .. }
                    | DecisionsCmd::Export
                    | DecisionsCmd::Mine { .. }
                    | DecisionsCmd::Learn { .. }
                    | DecisionsCmd::Auto { .. }
            )
        )
    {
        anyhow::bail!("that's for a project's decisions; drop --user");
    }
    let review = async |id, action| -> Result<()> {
        let d = client.review(root.clone(), id, action, None).await?;
        println!("#{} {}: {}", d.id, d.status.as_str(), d.title);
        Ok(())
    };
    match cmd {
        Some(DecisionsCmd::Accept { id }) => review(id, ReviewAction::Accept).await,
        Some(DecisionsCmd::Reject { id }) => review(id, ReviewAction::Reject).await,
        Some(DecisionsCmd::Retire { id }) => review(id, ReviewAction::Retire).await,
        Some(DecisionsCmd::Confirm { id }) => review(id, ReviewAction::Confirm).await,
        Some(DecisionsCmd::Pin { id }) => review(id, ReviewAction::Pin).await,
        Some(DecisionsCmd::Unpin { id }) => review(id, ReviewAction::Unpin).await,
        Some(DecisionsCmd::Health) => {
            let list = client.decisions(Some(cwd)).await?;
            // Your habits act in every project, so a project's health has them too.
            let habits = match user {
                true => Vec::new(),
                false => {
                    client
                        .decisions(Some(valkyrie_context::project::user()))
                        .await?
                }
            };
            print_health(&root, &list, &habits);
            Ok(())
        }
        Some(DecisionsCmd::Show { id }) => {
            let list = client.decisions(Some(cwd)).await?;
            let d = list
                .iter()
                .find(|d| d.id == id)
                .with_context(|| format!("no decision #{id} in {}", where_(&root)))?;
            print!("{}", valkyrie_context::render(d));
            if let Some(session) = reader()
                && d.status == DecisionStatus::Active
            {
                let store = valkyrie_context::Store::new(store_base());
                record_served(
                    &store,
                    &root,
                    &session,
                    [(d.id, valkyrie_context::served::Layer::Show)],
                );
            }
            if d.kind == DecisionKind::Policy {
                print_policy(client, &here).await?;
            }
            Ok(())
        }
        Some(DecisionsCmd::Preview) => {
            let list = client.decisions(Some(cwd)).await?;
            let valk = valk_command();
            let user = client
                .decisions(Some(valkyrie_context::project::user()))
                .await?;
            let budget = valkyrie_context::inject::BUDGET;
            let (block, shown) = valkyrie_context::inject::laid_out(&root, &list, budget, &valk);
            print!("{}{block}", valkyrie_context::inject::habits(&user, &valk));
            // On stderr, so the block itself can still be piped.
            eprintln!(
                "({} of {budget} characters: {} in full, {} as titles. \
                 `valk decisions pin <id>` puts one in full.)",
                block.len(),
                shown.whole.len(),
                shown.titles.len()
            );
            Ok(())
        }
        Some(DecisionsCmd::Auto { mode }) => {
            let mode = client.auto(mode).await?;
            println!(
                "{mode}: {}",
                match mode.as_str() {
                    "off" => "corrections aren't read",
                    "all" =>
                        "corrections in Claude and Codex sessions are read by a small model \
                              (claude -p --model haiku, no tools), which proposes the rules in them",
                    _ =>
                        "corrections in Claude sessions are read by a small model (claude -p \
                          --model haiku, no tools), which proposes the rules in them",
                }
            );
            Ok(())
        }
        Some(DecisionsCmd::Mine { days, dry_run }) => {
            mine(client, &cwd, &root, days, dry_run).await
        }
        Some(DecisionsCmd::Learn { session, about }) => {
            let session = match session {
                Some(id) => id,
                None => std::env::var("VALK_SESSION")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .context("which session? `valk decisions learn <session>`")?,
            };
            learn(client, session, about).await
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

/// How a scope is named to the user: a project's root, or their habits.
fn where_(root: &Path) -> String {
    match valkyrie_context::project::is_user(root) {
        true => "your habits".into(),
        false => root.display().to_string(),
    }
}

/// The `ship` policy in effect in `here`'s project, the user's habit with the
/// project's settings over it (ADR-0016 §1), and where each setting came from.
async fn print_policy(client: &Client, here: &Path) -> Result<()> {
    use valkyrie_context::project;
    let root = project::root(here);
    let ship = valkyrie_context::policy::ship(
        &client.decisions(Some(here.to_path_buf())).await?,
        &client.decisions(Some(project::user())).await?,
    );
    println!("\nIn effect in {}:", root.display());
    match ship {
        Some(ship) => {
            for line in ship.describe().lines() {
                println!("  {line}");
            }
        }
        None => println!(
            "  no ship policy: neither your habits nor this project's settings say how far to \
             ship (`until`)"
        ),
    }
    Ok(())
}

/// `valk decisions mine`: what you said again and again in this repository's
/// transcripts, and the decisions a small model proposes from it.
async fn mine(client: &Client, cwd: &Path, root: &Path, days: u64, dry_run: bool) -> Result<()> {
    use valkyrie_context::{extract, mine, model};
    let auto = valkyrie_proto::AutoMode::parse(&client.auto(None).await?);
    let codex = auto == Some(valkyrie_proto::AutoMode::All);
    let home = valkyrie_proto::home_dir();
    let claude_home = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let since = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(days.saturating_mul(86_400)))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let root_owned = root.to_owned();
    let (read, repeats) = tokio::task::spawn_blocking(move || {
        let codex_home = codex.then_some(codex_home.as_path());
        let sessions: Vec<Vec<String>> =
            mine::transcripts(&root_owned, &claude_home, codex_home, since)
                .iter()
                .filter_map(|path| std::fs::read_to_string(path).ok())
                .map(|jsonl| mine::said(&jsonl))
                .collect();
        (sessions.len(), mine::repeats(&sessions))
    })
    .await?;
    if repeats.is_empty() {
        println!(
            "Nothing said {}+ times in {}+ sessions ({read} transcripts from the last {days} days).",
            mine::MIN_TIMES,
            mine::MIN_SESSIONS,
        );
        return Ok(());
    }
    if dry_run {
        for r in &repeats {
            println!("{}x in {} sessions:", r.said.len(), r.sessions);
            for s in r.said.iter().take(3) {
                println!("  - {s}");
            }
        }
        return Ok(());
    }
    // `auto off` keeps your conversations on this machine; that holds here too.
    anyhow::ensure!(
        auto.is_some_and(|a| a != valkyrie_proto::AutoMode::Off),
        "`valk decisions auto` is off, so nothing goes to the model; --dry-run shows what you repeat"
    );
    eprintln!(
        "Asking a small model (claude -p --model haiku, no tools) about {} repeats from {read} sessions…",
        repeats.len(),
    );
    let mut known = client.decisions(Some(cwd.to_owned())).await?;
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let prompt = mine::prompt(&name, &known, &repeats);
    let dir = store_base();
    let reply = tokio::task::spawn_blocking(move || {
        valkyrie_proto::ensure_private_dir(&dir)?;
        model::ask(
            &model::command(mine::SYSTEM),
            &prompt,
            &dir,
            std::time::Duration::from_secs(120),
        )
    })
    .await??;
    let mut proposed = 0;
    for (found, group) in mine::parse_reply(&reply, &repeats) {
        if extract::already_known(&found.title, &known) {
            continue;
        }
        let r = &repeats[group];
        let quote = mine::quote(&r.said[0]);
        let d = client
            .decide(NewDecision {
                cwd: cwd.to_owned(),
                title: found.title,
                body: format!(
                    "{}\n\nYou said this {} times in {} sessions, like “{quote}”",
                    found.body,
                    r.said.len(),
                    r.sessions
                ),
                kind: found.kind,
                propose: true,
                supersedes: None,
                review_every: None,
                session: std::env::var("VALK_SESSION")
                    .ok()
                    .and_then(|s| s.parse().ok()),
            })
            .await?;
        println!("#{} proposed: {}", d.id, d.title);
        known.push(d);
        proposed += 1;
    }
    if proposed == 0 {
        println!(
            "Nothing a decision would fix in what you repeat ({} repeats looked at).",
            repeats.len()
        );
    } else {
        println!("Review them with `valk decisions`, the TUI home screen or the web app.");
    }
    Ok(())
}

/// `valk decisions learn`: the skill in what one session did.
async fn learn(
    client: &Client,
    id: valkyrie_proto::SessionId,
    about: Option<String>,
) -> Result<()> {
    use valkyrie_context::{extract, model, skill};
    let auto = valkyrie_proto::AutoMode::parse(&client.auto(None).await?);
    // `auto off` keeps your conversations on this machine; that holds here too.
    anyhow::ensure!(
        auto.is_some_and(|a| a != valkyrie_proto::AutoMode::Off),
        "`valk decisions auto` is off, so nothing goes to the model"
    );
    let info = client
        .list()
        .await?
        .into_iter()
        .find(|s| s.id == id)
        .with_context(|| format!("no session {id}"))?;
    anyhow::ensure!(
        info.status.agent == "claude",
        "session {id} isn't a Claude session; learn reads Claude transcripts only"
    );
    let chat = info
        .chat
        .clone()
        .with_context(|| format!("session {id} has no transcript Valkyrie knows of"))?;
    let jsonl =
        std::fs::read_to_string(&chat).with_context(|| format!("read {}", chat.display()))?;
    let digest = skill::digest(&jsonl);
    anyhow::ensure!(!digest.is_empty(), "session {id} has done nothing yet");
    let known = client.decisions(Some(info.cwd.clone())).await?;
    let root = valkyrie_context::project::root(&info.cwd);
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Only a skill shown whole may be rewritten: it would lose what wasn't.
    let (prompt, shown) = skill::prompt(&name, &known, &digest, about.as_deref());
    eprintln!(
        "Asking a small model (claude -p --model haiku, no tools) about {}…",
        info.name
    );
    let dir = store_base();
    let reply = tokio::task::spawn_blocking(move || {
        valkyrie_proto::ensure_private_dir(&dir)?;
        model::ask(
            &model::command(skill::SYSTEM),
            &prompt,
            &dir,
            std::time::Duration::from_secs(180),
        )
    })
    .await??;
    let Some((found, patch)) = skill::parse_reply(&reply, &shown) else {
        println!(
            "No procedure to keep in {} (or one a skill already covers).",
            info.name
        );
        return Ok(());
    };
    if patch.is_none() && extract::already_known(&found.title, &known) {
        println!("“{}” is already known.", found.title);
        return Ok(());
    }
    let d = client
        .decide(NewDecision {
            cwd: info.cwd.clone(),
            title: found.title,
            body: format!("{}\n\nLearned from the session {}.", found.body, info.name),
            kind: DecisionKind::Skill,
            propose: true,
            supersedes: patch,
            review_every: None,
            session: Some(id),
        })
        .await?;
    match patch {
        Some(old) => println!("#{} proposed, to replace skill #{old}: {}", d.id, d.title),
        None => println!("#{} proposed: {}", d.id, d.title),
    }
    println!(
        "Review it with `valk decisions show {}`, the TUI or the web app.",
        d.id
    );
    Ok(())
}

/// What `valk decisions` says when `root` has nothing to list.
/// For your habits, the one `valk decide --user` takes: how far you ship.
fn empty_hint(root: &Path) -> String {
    if valkyrie_context::project::is_user(root) {
        // Body on stdin with `-`, which works the same in bash, zsh and fish.
        return "no decisions for your habits yet; record how far you ship:\n  \
                printf '%s\\n' 'template: ship' 'until: pr' | \
                valk decide --user -k policy \"<title>\" -"
            .into();
    }
    format!(
        "no decisions for {} yet: `valk decide \"<title>\" \"<why>\"`",
        where_(root)
    )
}

fn print_list(root: &Path, list: &[Decision], all: bool) {
    let shown: Vec<&Decision> = list
        .iter()
        .filter(|d| all || matches!(d.status, DecisionStatus::Active | DecisionStatus::Proposed))
        .collect();
    if shown.is_empty() {
        println!("{}", empty_hint(root));
        return;
    }
    for d in shown {
        let mut status = match (&d.fresh.review, d.status) {
            (Some(why), _) => format!(" (may be out of date: {why})"),
            (None, DecisionStatus::Active) => String::new(),
            (None, other) => format!(" ({})", other.as_str()),
        };
        if d.pinned && d.status == DecisionStatus::Active {
            status.push_str(" (pinned)");
        }
        println!(
            "{:>4}  {:<10} {}{status}",
            format!("#{}", d.id),
            d.kind.as_str(),
            d.title
        );
    }
}

fn print_health(root: &Path, list: &[Decision], habits: &[Decision]) {
    let count = |s: DecisionStatus| list.iter().filter(|d| d.status == s).count();
    let flagged: Vec<&Decision> = list.iter().filter(|d| d.fresh.review.is_some()).collect();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let old = list
        .iter()
        .filter(|d| {
            d.status == DecisionStatus::Proposed && now.saturating_sub(d.created) > 7 * 86_400
        })
        .count();
    println!("{}", root.display());
    println!(
        "  {} active, {} proposed{}, {} rejected, {} retired or superseded",
        count(DecisionStatus::Active),
        count(DecisionStatus::Proposed),
        if old > 0 {
            format!(" ({old} over a week old)")
        } else {
            String::new()
        },
        count(DecisionStatus::Rejected),
        count(DecisionStatus::Retired) + count(DecisionStatus::Superseded),
    );
    for d in &flagged {
        println!(
            "  #{} may be out of date: {}",
            d.id,
            d.fresh.review.as_deref().unwrap_or("")
        );
    }
    let store = valkyrie_context::Store::new(store_base());
    let recurred = store.recurred(root, list);
    for (id, times) in &recurred {
        println!("  #{id} isn't working: {}", not_working(*times));
    }
    let overridden = overridden(&store, root, list, habits, now);
    for line in &overridden {
        println!("  {line}");
    }
    let unserved = store.unserved(root, list, now);
    for (id, days) in &unserved {
        println!("  #{id}: {}", valkyrie_context::served::says(*days));
    }
    let waiting = count(DecisionStatus::Proposed)
        + flagged.len()
        + recurred.len()
        + overridden.len()
        + unserved.len();
    println!(
        "  {}",
        if waiting == 0 {
            "healthy: nothing waits on you".to_owned()
        } else {
            format!("{waiting} wait on you: valk decisions, the TUI or your phone")
        }
    );
}

/// The policies in `root` and the user's habits that the user keeps
/// overriding (ADR-0013 §6), one line each.
pub(crate) fn overridden(
    store: &valkyrie_context::Store,
    root: &Path,
    list: &[Decision],
    habits: &[Decision],
    now: u64,
) -> Vec<String> {
    use valkyrie_context::outcome::says;
    let user = valkyrie_context::project::user();
    let mut out: Vec<String> = store
        .overridden(root, list, now)
        .iter()
        .map(|f| format!("#{} is getting overridden: {}", f.id, says(f)))
        .collect();
    if root != user {
        out.extend(store.overridden(&user, habits, now).iter().map(|f| {
            format!(
                "your habit #{} is getting overridden: {} (`valk decisions --user`)",
                f.id,
                says(f)
            )
        }));
    }
    out
}

/// Why a decision whose failure recurred `times` since it was accepted needs you.
pub(crate) fn not_working(times: usize) -> String {
    format!(
        "the failure it came from happened in {times} sessions since it was accepted; reword it \
         (`valk decisions` → edit) or retire it"
    )
}

/// `HEAD` of the checkout `dir` is in, short; `None` outside git.
fn head_commit(dir: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .no_window()
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
pub(crate) fn store_base() -> PathBuf {
    std::env::var_os("VALK_CONTEXT")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(valkyrie_context::Store::default_base)
}

/// How an agent should run Valkyrie: `valk` when that is this binary on `$PATH`,
/// else this binary's full path.
fn valk_command() -> String {
    let Ok(me) = std::env::current_exe().and_then(|me| valkyrie_proto::canonical(&me)) else {
        return "valk".into();
    };
    let name = format!("valk{}", std::env::consts::EXE_SUFFIX);
    let on_path = std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path)
            .any(|dir| valkyrie_proto::canonical(&dir.join(&name)).is_ok_and(|p| p == me))
    });
    if on_path {
        "valk".into()
    } else {
        me.to_string_lossy().into_owned()
    }
}

/// `valk context-hook <agent>`: Claude Code's and Codex's context hook. At session
/// start it prints the project's decisions as added context; with each prompt,
/// what the other agents in the repository are doing, when that's news. Like `valk hook` it
/// never fails and never prints anything else; outside a Valkyrie session it
/// prints nothing. It prints nothing either when the Context setting is off, or
/// with `ambient` (the copy in the user's own settings) where the session's own
/// settings already carry the hook.
pub fn hook(agent: &str, ambient: bool) {
    let ours = matches!(agent, "claude" | "codex")
        && std::env::var_os("VALK_SESSION").is_some()
        && !(ambient && std::env::var_os("VALK_SESSION_HOOKS").is_some());
    let mut raw = Vec::new();
    let read = std::io::stdin().take(1 << 20).read_to_end(&mut raw);
    // Drain whatever is past the limit (a huge paste), so the agent never hits
    // EPIPE writing it.
    let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
    if read.is_err() || !ours || !valkyrie_tui::context_enabled() {
        return;
    }
    // Answer only an event it names: a payload cut short or garbled gets nothing.
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return;
    };
    let cwd = payload["cwd"]
        .as_str()
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let Some(cwd) = cwd else {
        return;
    };
    let Some(event) = payload["hook_event_name"].as_str() else {
        return;
    };
    let text = match event {
        "SessionStart" => {
            let root = valkyrie_context::project::root(&cwd);
            let store = valkyrie_context::Store::new(store_base());
            let valk = valk_command();
            let habits = store.load(&valkyrie_context::project::user());
            let mut text = valkyrie_context::inject::habits(&habits, &valk);
            let (block, shown) = valkyrie_context::inject::laid_out(
                &root,
                &store.load(&root),
                valkyrie_context::inject::BUDGET,
                &valk,
            );
            text.push_str(&block);
            if let Some(session) = payload["session_id"].as_str().filter(|s| !s.is_empty()) {
                use valkyrie_context::served::Layer;
                let serves = shown
                    .whole
                    .iter()
                    .map(|id| (*id, Layer::Block))
                    .chain(shown.titles.iter().map(|id| (*id, Layer::Title)));
                record_served(&store, &root, session, serves);
            }
            text
        }
        "UserPromptSubmit" => {
            let prompt = payload["prompt"].as_str().unwrap_or("");
            let root = valkyrie_context::project::root(&cwd);
            let known = valkyrie_context::Store::new(store_base()).load(&root);
            let skills = valkyrie_context::skill::about(prompt, &known);
            let mut text = siblings().unwrap_or_default();
            for d in skills {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&format!(
                    "Skill #{} “{}” looks like what this asks for: read it first with `{} decisions show {}`.",
                    d.id,
                    d.title,
                    valk_command(),
                    d.id
                ));
            }
            if text.is_empty() {
                return;
            }
            text
        }
        _ => return,
    };
    println!("{}", valkyrie_context::inject::hook_output(event, &text));
}

/// Logs decisions served to `session` in `root` (ADR-0014 §5). Measuring only:
/// a failure to write is dropped, never shown to the agent.
pub(crate) fn record_served(
    store: &valkyrie_context::Store,
    root: &Path,
    session: &str,
    serves: impl IntoIterator<Item = (u32, valkyrie_context::served::Layer)>,
) {
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let serves: Vec<valkyrie_context::served::Serve> = serves
        .into_iter()
        .map(|(id, layer)| valkyrie_context::served::Serve {
            session: session.to_owned(),
            id,
            layer,
            at,
        })
        .collect();
    let _ = store.record_served(root, &serves, at);
}

/// The session a read from inside a Valkyrie session is served to, for the
/// serving log; `None` outside one, where it's you reading, not an agent.
pub(crate) fn reader() -> Option<String> {
    let id = std::env::var("VALK_SESSION")
        .ok()
        .filter(|s| !s.is_empty())?;
    Some(format!("valk:{id}"))
}

/// Asks the daemon what this session's agent should hear about the agents beside
/// it. Blocking, and quick to give up: a prompt must never wait on Valkyrie.
fn siblings() -> Option<String> {
    use std::time::Duration;
    let session = std::env::var("VALK_SESSION").ok()?.parse().ok()?;
    let socket = std::env::var_os("VALK_SOCKET")?;
    let frame =
        valkyrie_proto::codec::encode(&valkyrie_proto::ClientMsg::Siblings { req: 1, session })
            .ok()?;
    let body = valkyrie_proto::ipc::exchange(
        std::path::Path::new(&socket),
        &frame,
        true,
        Duration::from_millis(300),
    )
    .ok()?;
    match serde_json::from_slice(&body).ok()? {
        valkyrie_proto::ServerMsg::Ok {
            reply: valkyrie_proto::Reply::Text { text },
            ..
        } => Some(text),
        _ => None,
    }
}

/// `valk handoff`: the resume of session `id` for another agent, and that session.
pub async fn handoff(
    client: &Client,
    id: valkyrie_proto::SessionId,
    summarize: bool,
) -> Result<(String, valkyrie_proto::SessionInfo)> {
    use valkyrie_context::{extract, handoff, model};
    let info = client
        .list()
        .await?
        .into_iter()
        .find(|s| s.id == id)
        .with_context(|| format!("no session {id}"))?;
    let chat = info.chat.clone().with_context(|| {
        format!(
            "session {id} has no agent transcript Valkyrie knows of (it needs Claude Code or Codex, started with valk new)"
        )
    })?;
    let jsonl =
        std::fs::read_to_string(&chat).with_context(|| format!("read {}", chat.display()))?;
    let messages = extract::messages(&jsonl);
    let rel = |p: &str| {
        Path::new(p)
            .strip_prefix(&info.cwd)
            .map(|r| r.display().to_string())
            .unwrap_or_else(|_| p.to_owned())
    };
    let files: Vec<String> = handoff::files(&jsonl).iter().map(|p| rel(p)).collect();
    let decisions = client.decisions(Some(info.cwd.clone())).await?;
    let summary = if summarize && !messages.is_empty() {
        let prompt = handoff::summary_prompt(&messages);
        let dir = store_base();
        let asked = tokio::task::spawn_blocking(move || {
            valkyrie_proto::ensure_private_dir(&dir)?;
            model::ask(
                &model::command(handoff::SUMMARY_SYSTEM),
                &prompt,
                &dir,
                std::time::Duration::from_secs(120),
            )
        })
        .await?;
        match asked {
            Ok(text) if !text.trim().is_empty() => Some(text),
            Ok(_) => None,
            Err(e) => {
                eprintln!("no summary ({e:#}); using its last reply instead");
                None
            }
        }
    } else {
        None
    };
    let text = handoff::render(
        &handoff::Handoff {
            from: format!(
                "{} ({}, {})",
                info.name,
                info.status.agent,
                info.status.state.label()
            ),
            cwd: info.cwd.display().to_string(),
            git: git_where(&info.cwd),
            messages: &messages,
            files: &files,
            decisions: &decisions,
            summary,
        },
        handoff::BUDGET,
    );
    Ok((text, info))
}

/// `branch @ commit` of the checkout `dir` is in.
fn git_where(dir: &Path) -> Option<String> {
    let branch = std::process::Command::new("git")
        .no_window()
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())?;
    Some(match head_commit(dir) {
        Some(commit) => format!("{branch} @ {commit}"),
        None => branch,
    })
}

/// Saves a handoff where only its owner can read it, and says where.
pub fn save_handoff(text: &str, from: valkyrie_proto::SessionId) -> Result<PathBuf> {
    let dir = store_base().join("handoffs");
    valkyrie_proto::ensure_private_dir(&store_base())?;
    valkyrie_proto::ensure_private_dir(&dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let path = dir.join(format!("{stamp}-session-{from}.md"));
    let mut file = valkyrie_proto::private_file()
        .write(true)
        .create_new(true)
        .open(&path)?;
    std::io::Write::write_all(&mut file, text.as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #63: an empty habit list must suggest recording a habit, not a project
    /// decision, and the habit it suggests must be one `valk decide --user` takes.
    #[test]
    fn an_empty_list_hints_at_the_scope_it_lists() {
        let user = empty_hint(&valkyrie_context::project::user());
        assert!(user.contains("your habits"), "{user}");
        assert!(user.contains("valk decide --user -k policy"), "{user}");
        let body = "template: ship\nuntil: pr";
        assert!(user.contains("'template: ship' 'until: pr'"), "{user}");
        assert!(
            valkyrie_context::policy::check(body, true).is_ok(),
            "the suggested habit must be accepted"
        );

        let project = empty_hint(Path::new("/src/app"));
        assert!(project.contains("/src/app"), "{project}");
        assert!(
            project.contains("`valk decide \"<title>\" \"<why>\"`"),
            "{project}"
        );
        assert!(!project.contains("--user"), "{project}");
    }
}

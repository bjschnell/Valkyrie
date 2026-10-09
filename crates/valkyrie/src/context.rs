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
    /// Say an active decision still holds (one flagged as maybe out of date).
    Confirm { id: u32 },
    /// How this project's decisions are doing: what waits on you, what may be
    /// out of date.
    Health,
    /// Show one decision in full, with where it came from.
    Show { id: u32 },
    /// Print the block agents get at session start.
    Preview,
    /// Copy the active decisions into <repo>/.valkyrie/decisions/ for git.
    Export,
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

pub async fn decisions(client: &Client, all: bool, cmd: Option<DecisionsCmd>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root = valkyrie_context::project::root(&cwd);
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
        Some(DecisionsCmd::Health) => {
            let list = client.decisions(Some(cwd)).await?;
            print_health(&root, &list);
            Ok(())
        }
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
        let status = match (&d.fresh.review, d.status) {
            (Some(why), _) => format!(" (may be out of date: {why})"),
            (None, DecisionStatus::Active) => String::new(),
            (None, other) => format!(" ({})", other.as_str()),
        };
        println!(
            "{:>4}  {:<10} {}{status}",
            format!("#{}", d.id),
            d.kind.as_str(),
            d.title
        );
    }
}

fn print_health(root: &Path, list: &[Decision]) {
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
    let waiting = count(DecisionStatus::Proposed) + flagged.len();
    println!(
        "  {}",
        if waiting == 0 {
            "healthy: nothing waits on you".to_owned()
        } else {
            format!("{waiting} wait on you: valk decisions, the TUI or your phone")
        }
    );
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
/// prints nothing.
pub fn hook(agent: &str) {
    let ours = matches!(agent, "claude" | "codex") && std::env::var_os("VALK_SESSION").is_some();
    let mut raw = Vec::new();
    let read = std::io::stdin().take(1 << 20).read_to_end(&mut raw);
    // Drain whatever is past the limit (a huge paste), so the agent never hits
    // EPIPE writing it.
    let _ = std::io::copy(&mut std::io::stdin(), &mut std::io::sink());
    if read.is_err() || !ours {
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
            let decisions = valkyrie_context::Store::new(store_base()).load(&root);
            valkyrie_context::inject::block(
                &root,
                &decisions,
                valkyrie_context::inject::BUDGET,
                &valk_command(),
            )
        }
        "UserPromptSubmit" => match siblings() {
            Some(text) if !text.is_empty() => text,
            _ => return,
        },
        _ => return,
    };
    println!("{}", valkyrie_context::inject::hook_output(event, &text));
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

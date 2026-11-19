//! `valk wiki`: the repository's agent wiki (backlog #210), pages agents write
//! for agents about how the code is built. Read and written straight in the
//! context store, as the hooks read it; only `explore` needs the daemon, to start
//! the agent that writes it.

use crate::context::{head_commit, store_base, valk_command};
use anyhow::{Context, Result};
use clap::Subcommand;
use std::io::Read;
use std::path::{Path, PathBuf};
use valkyrie_context::Store;
use valkyrie_context::wiki::{self, NewPage, Page};
use valkyrie_proto::Size;
use valkyrie_proto::client::Client;

#[derive(Subcommand)]
pub enum WikiCmd {
    /// Every page, the stale ones marked.
    List,
    /// Print a page, with what it covers and whether it may be out of date.
    Show { page: String },
    /// Write a page, replacing one of the same name. For agents mapping the code.
    Write {
        /// Lowercase letters, digits and dashes (`overview`, `daemon`, `web-api`).
        page: String,
        #[arg(long)]
        title: String,
        /// One line: what the page answers. Sessions see it in the index.
        #[arg(long, default_value = "")]
        summary: String,
        /// A directory or file the page describes, relative to the repository
        /// root; repeat it. When they change a lot, the page reads as stale.
        #[arg(long, value_name = "PATH")]
        scope: Vec<String>,
        /// The markdown; `-` (or nothing) reads it from stdin.
        body: Option<String>,
    },
    /// Delete a page.
    Rm { page: String },
    /// Start an agent that explores this repository and writes its wiki.
    Explore {
        /// Only rewrite the pages whose code changed since they were written.
        #[arg(long)]
        stale: bool,
        /// The agent to start.
        #[arg(long, default_value = "claude", value_parser = ["claude", "codex"])]
        agent: String,
        /// Print the agent's brief instead of starting it.
        #[arg(long)]
        print: bool,
        /// Attach to the new session.
        #[arg(short, long, conflicts_with = "print")]
        attach: bool,
    },
}

fn here() -> Result<(Store, PathBuf)> {
    let cwd = std::env::current_dir().context("current directory")?;
    Ok((
        Store::new(store_base()),
        valkyrie_context::project::root(&cwd),
    ))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// A page as `show` prints it: what it covers and its age first, then the body.
pub fn shown(root: &Path, p: &Page) -> String {
    let stale = wiki::staleness(root, std::slice::from_ref(p));
    let mut out = format!("# {}\n", p.title);
    if !p.summary.is_empty() {
        out.push_str(&format!("{}\n", p.summary));
    }
    let date = valkyrie_context::docs::rfc3339(p.updated);
    out.push_str(&format!(
        "(wiki page `{}` · written {} at {} · covers {})\n",
        p.slug,
        date.get(..10).unwrap_or("?"),
        p.commit.as_deref().unwrap_or("no commit"),
        match p.scope.is_empty() {
            true => "no paths".into(),
            false => p.scope.join(", "),
        }
    ));
    if let Some(why) = stale.get(&p.slug) {
        out.push_str(&format!(
            "(stale: {why}; check it against the code, and rewrite it if it's wrong)\n"
        ));
    }
    out.push('\n');
    out.push_str(&p.body);
    out.push('\n');
    out
}

/// What `valk wiki list` prints.
pub fn listing(root: &Path, pages: &[Page]) -> String {
    if pages.is_empty() {
        return format!(
            "{} has no wiki yet: `valk wiki explore` starts an agent that writes it.\n",
            root.display()
        );
    }
    let stale = wiki::staleness(root, pages);
    let mut out = String::new();
    for p in pages {
        let mark = match stale.get(&p.slug) {
            Some(why) => format!("  [stale: {why}]"),
            None => String::new(),
        };
        out.push_str(&format!("{:<20} {}{mark}\n", p.slug, p.title));
    }
    if !stale.is_empty() {
        out.push_str(&format!(
            "\n{} stale: `valk wiki explore --stale` rewrites them.\n",
            stale.len()
        ));
    }
    out
}

/// Runs the commands that don't need the daemon; `None` for `explore`, which does.
pub fn run_local(cmd: &WikiCmd) -> Result<Option<()>> {
    let (store, root) = here()?;
    match cmd {
        WikiCmd::List => print!("{}", listing(&root, &store.wiki(&root))),
        WikiCmd::Show { page } => {
            let p = store
                .wiki_page(&root, page)
                .with_context(|| format!("no wiki page {page:?}: `valk wiki list` names them"))?;
            print!("{}", shown(&root, &p));
        }
        WikiCmd::Write {
            page,
            title,
            summary,
            scope,
            body,
        } => {
            let body = match body.as_deref() {
                None | Some("-") => {
                    let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s)?;
                    s
                }
                Some(b) => b.to_owned(),
            };
            let new = NewPage {
                slug: page.clone(),
                title: title.clone(),
                summary: summary.clone(),
                scope: scope.clone(),
                body,
            };
            let p = store.write_wiki(&root, &new, head_commit(&root), now())?;
            println!("wiki page `{}` written: {}", p.slug, p.title);
        }
        WikiCmd::Rm { page } => {
            store.rm_wiki(&root, page)?;
            println!("wiki page `{page}` deleted");
        }
        WikiCmd::Explore { .. } => return Ok(None),
    }
    Ok(Some(()))
}

/// The brief `explore` starts its agent with, checked here so `--print` and a
/// refusal ("no page is stale") need no daemon.
pub fn explore_brief(stale_only: bool) -> Result<(PathBuf, String)> {
    let (store, root) = here()?;
    let brief = wiki::explore_brief(&root, &store.wiki(&root), stale_only, &valk_command())?;
    Ok((root, brief))
}

/// Starts the agent through the daemon, which builds the same brief, as the
/// TUI and the web app do.
pub async fn explore(
    client: &Client,
    root: PathBuf,
    stale: bool,
    agent: String,
) -> Result<valkyrie_proto::SessionInfo> {
    let size = Size {
        cols: 120,
        rows: 40,
    };
    client
        .wiki_explore(root, stale, agent, size, valkyrie_proto::login_env())
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_wiki_says_how_to_start_one() {
        let text = listing(Path::new("/r/app"), &[]);
        assert!(text.contains("valk wiki explore"), "{text}");
    }

    #[test]
    fn show_leads_with_what_the_page_covers() {
        let p = Page {
            slug: "daemon".into(),
            title: "The daemon".into(),
            summary: "How sessions run".into(),
            scope: vec!["src".into()],
            commit: Some("abc1234".into()),
            updated: 0,
            body: "Body.".into(),
        };
        let text = shown(Path::new("/nonexistent-root"), &p);
        assert!(
            text.starts_with("# The daemon\nHow sessions run\n"),
            "{text}"
        );
        assert!(text.contains("at abc1234 · covers src"), "{text}");
        // Its path is gone from this root, so it reads as stale.
        assert!(text.contains("(stale: src no longer exists"), "{text}");
        assert!(text.ends_with("Body.\n"));
    }
}

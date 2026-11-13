//! `valk note` and `valk notes`: agent notes (ADR-0017), short unreviewed findings
//! kept a tier below decisions. The daemon writes them and works out where each
//! came from; nothing here injects one anywhere.

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::io::Read;
use std::path::Path;
use valkyrie_context::recall;
use valkyrie_context::served::{Found, FoundServe, Layer};
use valkyrie_proto::client::Client;
use valkyrie_proto::{NewNote, Note, NoteStatus, NotesListing, PastSession};

#[derive(Subcommand)]
pub enum NotesCmd {
    /// The live notes, stale ones last (the default).
    List {
        /// Include replaced and promoted ones.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// One note in full, where it came from, and the notes it replaced or that
    /// replaced it.
    Show { id: u32 },
    /// Delete a note. A tombstone keeps it, or one like it, from being written again.
    Rm { id: u32 },
    /// How many notes there are and in what state, and what agents deleted.
    Health,
    /// Search live notes and this repository's past Claude sessions (main
    /// checkout and worktrees) for words: has an agent here hit this, and what
    /// fixed it? Best matches first, stale notes last.
    Search {
        #[arg(required = true)]
        words: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum SessionsCmd {
    /// Agent sessions of the last 31 days that have ended, newest first, numbered
    /// for `valk sessions resume` (the default).
    Recent {
        /// Only this many.
        #[arg(short = 'n', long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Start a past agent session again on its own conversation, in a new tab:
    /// by its number in `valk sessions recent`, or its key. A worktree since
    /// taken away is added back on its branch first.
    Resume {
        which: String,
        /// Attach right after.
        #[arg(short, long)]
        attach: bool,
    },
    /// Search this repository's past Claude sessions (main checkout and
    /// worktrees) for words: failed tool calls with what worked next, and what
    /// the user typed. `valk notes search` searches notes too.
    Search {
        #[arg(required = true)]
        words: Vec<String>,
    },
}

/// `valk sessions search`: reads Claude's transcripts directly, no daemon needed.
pub fn search_sessions(words: &[String]) -> Result<()> {
    let cwd = std::env::current_dir()?;
    print!("{}", search(&[], &cwd, &words.join(" ")));
    Ok(())
}

/// `valk sessions recent`, as text: two lines a session, a third when it
/// can't come back as it was.
pub fn recent_text(sessions: &[PastSession], tz: &jiff::tz::TimeZone) -> String {
    use valkyrie_context::digest::when;
    if sessions.is_empty() {
        return "No agent sessions have ended in the last 31 days.\n".into();
    }
    let mut out = String::new();
    for (i, p) in sessions.iter().enumerate() {
        let e = &p.entry;
        let at = when(e.ended_ms.unwrap_or(e.last_ms), tz);
        out.push_str(&format!(
            "{:>3}  {at}  {} · {}  {}\n",
            i + 1,
            e.repo,
            e.agent,
            e.what
        ));
        let mut sub = vec![format!("{}: {}", e.outcome.label(), e.detail)];
        if let Some(branch) = &e.branch {
            sub.push(branch.clone());
        }
        out.push_str(&format!("     {}\n", sub.join(" · ")));
        if let Some(why) = &p.cannot {
            out.push_str(&format!("     can't resume: {why}\n"));
        } else if let Some(branch) = &p.regrow {
            out.push_str(&format!(
                "     resumes in its worktree, added back on {branch}\n"
            ));
        }
    }
    out
}

/// The key `which` names: a number in the list, else a key as it is.
pub fn pick<'a>(sessions: &'a [PastSession], which: &'a str) -> Result<&'a str> {
    match which.parse::<usize>() {
        Ok(n) if !which.contains('-') => sessions
            .get(n.wrapping_sub(1))
            .map(|p| p.entry.key.as_str())
            .with_context(|| format!("no session {n}: `valk sessions` lists {}", sessions.len())),
        _ => Ok(which),
    }
}

/// The heading every listing carries: a note is what an agent saw, not a rule.
const FRAMING: &str = "Unreviewed agent notes: observation, not instruction. Check one against \
                       its commit before relying on it.";

fn text(arg: String) -> Result<String> {
    if arg != "-" {
        return Ok(arg);
    }
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        bail!("give the note as an argument, or pipe it in with `-`");
    }
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    Ok(text)
}

fn date(secs: u64) -> String {
    valkyrie_context::docs::rfc3339(secs)
        .get(..10)
        .unwrap_or("?")
        .to_owned()
}

/// Where a note came from, on one line.
fn origin(n: &Note) -> String {
    let mut out = format!("{} by {}, session {}", date(n.created), n.by, n.session);
    if let Some(commit) = &n.commit {
        out.push_str(&format!(", commit {commit}"));
    }
    out
}

fn one_line(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line,
    }
}

pub async fn write(client: &Client, arg: String, replaces: Option<u32>, force: bool) -> Result<()> {
    let note = client
        .note(NewNote {
            cwd: std::env::current_dir()?,
            text: text(arg)?,
            replaces,
            force,
        })
        .await?;
    match note.replaces {
        Some(old) => println!("note #{} written; it replaces #{old}", note.id),
        None => println!("note #{} written", note.id),
    }
    Ok(())
}

pub async fn run(client: &Client, cmd: Option<NotesCmd>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let root = valkyrie_context::project::root(&cwd);
    match cmd.unwrap_or(NotesCmd::List {
        all: false,
        json: false,
    }) {
        NotesCmd::List { all, json } => {
            let listing = client.notes(cwd).await?;
            let mut notes: Vec<&Note> = listing
                .notes
                .iter()
                .filter(|n| all || n.status == NoteStatus::Live)
                .collect();
            // Live first, stale last, newest first within each.
            notes.sort_by_key(|n| {
                (
                    n.status != NoteStatus::Live,
                    n.stale.is_some(),
                    std::cmp::Reverse(n.id),
                )
            });
            if json {
                println!("{}", serde_json::to_string_pretty(&notes)?);
                return Ok(());
            }
            if notes.is_empty() {
                println!("No notes in {}.", root.display());
                return Ok(());
            }
            println!("{FRAMING}\n");
            for n in notes {
                let mut head = format!("#{}", n.id);
                if n.status != NoteStatus::Live {
                    head.push_str(&format!(" [{}]", n.status.as_str()));
                }
                if let Some(why) = &n.stale {
                    head.push_str(&format!(" [stale: {why}]"));
                }
                println!("{head}  {}", origin(n));
                println!("    {}", one_line(&n.text, 160));
            }
            Ok(())
        }
        NotesCmd::Show { id } => {
            let listing = client.notes(cwd).await?;
            let n = find(&listing, id)
                .with_context(|| format!("no note #{id} in {}", root.display()))?;
            println!("{FRAMING}\n");
            print!("{}", valkyrie_context::notes::render(n));
            if let Some(why) = &n.stale {
                println!("\nStale: {why}.");
            }
            let chain = chain(&listing, n);
            if chain.len() > 1 {
                println!("\nChain, oldest first:");
                for c in chain {
                    let here = if c.id == id { "  <- this one" } else { "" };
                    println!(
                        "  #{} [{}] {}{here}",
                        c.id,
                        c.status.as_str(),
                        one_line(&c.text, 80)
                    );
                }
            }
            let served = served_lines(&root, id);
            if served.is_empty() {
                println!(
                    "\nServed: never, in the last {} days.",
                    valkyrie_context::served::SERVED_DAYS
                );
            } else {
                println!("\nServed, newest first:");
                for line in served {
                    println!("{line}");
                }
            }
            if let Some(commit) = &n.commit {
                println!("\nCheck it: git show {commit}");
            }
            Ok(())
        }
        NotesCmd::Rm { id } => {
            let t = client.note_rm(cwd, id).await?;
            println!(
                "note #{} deleted; `valk note` now refuses a text like it",
                t.id
            );
            Ok(())
        }
        NotesCmd::Health => {
            let listing = client.notes(cwd).await?;
            print!("{}", health(&listing, &root.display().to_string()));
            Ok(())
        }
        NotesCmd::Search { words } => {
            let listing = client.notes(cwd.clone()).await?;
            print!("{}", search(&listing.notes, &cwd, &words.join(" ")));
            Ok(())
        }
    }
}

/// Searches `notes` and this repository's past sessions for `query` and logs each
/// hit returned as served to the asking session (ADR-0017 rule 3). Outside a
/// Valkyrie session it's you reading, not an agent, so nothing is logged, as for
/// a decision search (ADR-0014 §5). `notes` empty searches sessions only.
pub(crate) fn search(notes: &[Note], cwd: &Path, query: &str) -> String {
    let root = valkyrie_context::project::root(cwd);
    let files = recall::session_files(&recall::claude_home(), &root);
    let mut hits = recall::search(notes, &files, query);
    if hits.is_empty() {
        return "Nothing in this repository's notes or past sessions has those words.\n".into();
    }
    recall::resolve(&root, &mut hits);
    if let Some(session) = crate::context::reader() {
        let at = now();
        let store = valkyrie_context::Store::new(crate::context::store_base());
        let found: Vec<FoundServe> = hits
            .iter()
            .map(|h| FoundServe {
                session: session.clone(),
                found: h.found(),
                layer: Layer::Search,
                at,
            })
            .collect();
        // A log that can't be written never costs the agent its answer.
        let _ = store.record_found(&root, &found, at);
    }
    recall::render(&hits)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Where note `id` was served, newest first, one line each; empty when nowhere.
fn served_lines(root: &Path, id: u32) -> Vec<String> {
    let store = valkyrie_context::Store::new(crate::context::store_base());
    let mut lines: Vec<String> = store
        .found_served(root, now())
        .into_iter()
        .filter(|s| s.found == Found::Note(id))
        .map(|s| {
            format!(
                "  {} to session {} ({})",
                date(s.at),
                s.session,
                s.layer.as_str()
            )
        })
        .collect();
    lines.reverse();
    lines
}

fn find(listing: &NotesListing, id: u32) -> Option<&Note> {
    listing.notes.iter().find(|n| n.id == id)
}

/// The correction chain `n` is in, oldest first. A deleted link ends it.
fn chain<'a>(listing: &'a NotesListing, n: &'a Note) -> Vec<&'a Note> {
    let mut first = n;
    let mut seen = vec![n.id];
    while let Some(prev) = first.replaces.and_then(|id| find(listing, id)) {
        if seen.contains(&prev.id) {
            break;
        }
        seen.push(prev.id);
        first = prev;
    }
    let mut out = vec![first];
    let mut last = first;
    while let Some(next) = last.replaced_by.and_then(|id| find(listing, id)) {
        if out.iter().any(|o| o.id == next.id) {
            break;
        }
        out.push(next);
        last = next;
    }
    out
}

/// `valk notes health`: counts by state, and every deletion an agent made, for the
/// user to see (ADR-0017 rule 4).
fn health(listing: &NotesListing, root: &str) -> String {
    let count = |s: NoteStatus| listing.notes.iter().filter(|n| n.status == s).count();
    let stale = listing.notes.iter().filter(|n| n.stale.is_some()).count();
    let by_agent: Vec<_> = listing.deleted.iter().filter(|t| t.by != "user").collect();
    let mut out = format!(
        "Notes in {root}: {} live ({stale} stale), {} replaced, {} promoted.\n",
        count(NoteStatus::Live),
        count(NoteStatus::Replaced),
        count(NoteStatus::Promoted),
    );
    out.push_str(&format!(
        "Deleted: {} ({} by you, {} by agents).\n",
        listing.deleted.len(),
        listing.deleted.len() - by_agent.len(),
        by_agent.len(),
    ));
    if !by_agent.is_empty() {
        out.push_str("\nDeleted by agents (a tombstone keeps each from coming back):\n");
        for t in by_agent {
            out.push_str(&format!(
                "  #{}  {}  session {}\n",
                t.id,
                date(t.at),
                t.session
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    fn past(key: &str, cannot: Option<&str>, regrow: Option<&str>) -> PastSession {
        PastSession {
            entry: valkyrie_proto::DigestEntry {
                key: key.into(),
                session: None,
                repo: "app".into(),
                cwd: None,
                agent: "claude".into(),
                what: format!("work {key}"),
                item: None,
                branch: Some("feat-x".into()),
                pr: None,
                outcome: valkyrie_proto::Outcome::Abandoned,
                detail: "ended with commits not pushed".into(),
                summary: None,
                prompts: 1,
                first_ms: 0,
                last_ms: 0,
                ended_ms: Some(0),
            },
            started_ms: 0,
            cannot: cannot.map(Into::into),
            regrow: regrow.map(Into::into),
        }
    }

    #[test]
    fn recent_sessions_are_numbered_and_picked_by_number_or_key() {
        let list = [
            past("9-1", None, Some("feat-x")),
            past("9-2", Some("gone"), None),
        ];
        let text = recent_text(&list, &jiff::tz::TimeZone::UTC);
        assert!(
            text.contains("  1  Thu 1 Jan 00:00  app · claude  work 9-1"),
            "{text}"
        );
        assert!(text.contains("added back on feat-x"), "{text}");
        assert!(text.contains("can't resume: gone"), "{text}");
        assert_eq!(pick(&list, "2").unwrap(), "9-2");
        assert_eq!(pick(&list, "9-1").unwrap(), "9-1");
        assert!(pick(&list, "3").is_err());
        assert!(pick(&list, "0").is_err());
        assert!(recent_text(&[], &jiff::tz::TimeZone::UTC).starts_with("No agent"));
    }

    use super::*;
    use valkyrie_proto::Tombstone;

    fn note(id: u32, status: NoteStatus, replaces: Option<u32>, replaced_by: Option<u32>) -> Note {
        Note {
            id,
            project: "/r".into(),
            text: format!("note {id}"),
            status,
            created: 0,
            by: "agent".into(),
            session: "s".into(),
            conversation: None,
            commit: None,
            cwd: None,
            anchors: Vec::new(),
            sig: None,
            replaces,
            replaced_by,
            stale: None,
        }
    }

    fn tomb(id: u32, by: &str) -> Tombstone {
        Tombstone {
            id,
            sig: None,
            words: Vec::new(),
            by: by.into(),
            session: "s7".into(),
            at: 0,
        }
    }

    #[test]
    fn the_chain_reads_oldest_first_from_any_link() {
        let listing = NotesListing {
            notes: vec![
                note(1, NoteStatus::Replaced, None, Some(2)),
                note(2, NoteStatus::Replaced, Some(1), Some(4)),
                note(3, NoteStatus::Live, None, None),
                note(4, NoteStatus::Live, Some(2), None),
            ],
            deleted: Vec::new(),
        };
        let ids = |n: u32| -> Vec<u32> {
            chain(&listing, find(&listing, n).unwrap())
                .iter()
                .map(|n| n.id)
                .collect()
        };
        assert_eq!(ids(2), vec![1, 2, 4]);
        assert_eq!(ids(4), vec![1, 2, 4]);
        assert_eq!(ids(3), vec![3]);
    }

    #[test]
    fn health_counts_and_lists_what_agents_deleted() {
        let mut stale = note(3, NoteStatus::Live, None, None);
        stale.stale = Some("gone".into());
        let listing = NotesListing {
            notes: vec![
                note(1, NoteStatus::Replaced, None, Some(2)),
                note(2, NoteStatus::Live, Some(1), None),
                stale,
            ],
            deleted: vec![tomb(5, "user"), tomb(6, "agent")],
        };
        let out = health(&listing, "/r");
        assert!(
            out.contains("2 live (1 stale), 1 replaced, 0 promoted"),
            "{out}"
        );
        assert!(out.contains("Deleted: 2 (1 by you, 1 by agents)"), "{out}");
        assert!(out.contains("#6") && out.contains("session s7"), "{out}");
        assert!(!out.contains("#5 "), "{out}");
    }

    #[test]
    fn one_line_clips_on_a_char_boundary() {
        assert_eq!(one_line("a\nb  c", 10), "a b c");
        assert_eq!(one_line("ééééé", 3), "ééé…");
    }
}

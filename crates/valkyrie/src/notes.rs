//! `valk note` and `valk notes`: agent notes (ADR-0017), short unreviewed findings
//! kept a tier below decisions. The daemon writes them and works out where each
//! came from; nothing here injects one anywhere.

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::io::Read;
use valkyrie_proto::client::Client;
use valkyrie_proto::{NewNote, Note, NoteStatus, NotesListing};

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
    }
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

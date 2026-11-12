//! Agent notes (ADR-0017): short, unreviewed texts a tier below decisions, kept in
//! `<state dir>/context/<project>/notes/NNNN.md` with the same `key: value` front
//! matter. Anyone writes one without review; a correction replaces the note it
//! corrects; one command deletes a note and leaves a tombstone so nothing writes it
//! again. Notes are never injected: nothing in this module feeds a hook.

use crate::{Store, extract, file::one_line, project, shown, stale, write_atomic};
use anyhow::{Context, Result, bail};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use valkyrie_proto::{NewNote, Note, NoteStatus, Provenance, Tombstone};

/// Longest note kept (ADR-0017 §1).
pub const MAX_NOTE: usize = 1000;

/// The tombstones' file, beside the notes.
const DELETED: &str = "deleted.jsonl";

pub fn render(n: &Note) -> String {
    let mut out = String::from("---\n");
    let mut field = |k: &str, v: &str| {
        if !v.is_empty() {
            out.push_str(&format!("{k}: {}\n", one_line(v)));
        }
    };
    field("id", &n.id.to_string());
    field("status", n.status.as_str());
    field("created", &n.created.to_string());
    field("by", &n.by);
    field("session", &n.session);
    field("conversation", n.conversation.as_deref().unwrap_or(""));
    field("commit", n.commit.as_deref().unwrap_or(""));
    let cwd = n.cwd.as_ref().map(|p| p.to_string_lossy());
    field("cwd", cwd.as_deref().unwrap_or(""));
    field("anchors", &n.anchors.join(", "));
    field("sig", n.sig.as_deref().unwrap_or(""));
    field(
        "replaces",
        &n.replaces.map(|r| r.to_string()).unwrap_or_default(),
    );
    field(
        "replaced_by",
        &n.replaced_by.map(|r| r.to_string()).unwrap_or_default(),
    );
    out.push_str("---\n");
    out.push_str(n.text.trim());
    out.push('\n');
    out
}

/// Reads a note file back. `project` is the directory it's in, not a field.
pub fn parse(text: &str, project: PathBuf) -> Option<Note> {
    let rest = text.strip_prefix("---\n")?;
    let (front, body) = rest.split_once("\n---\n")?;
    let mut n = Note {
        id: 0,
        project,
        text: body.trim().to_owned(),
        status: NoteStatus::Live,
        created: 0,
        by: String::new(),
        session: "none".into(),
        conversation: None,
        commit: None,
        cwd: None,
        anchors: Vec::new(),
        sig: None,
        replaces: None,
        replaced_by: None,
        stale: None,
    };
    let list = |v: &str| -> Vec<String> {
        v.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };
    for line in front.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        let some = || (!v.is_empty()).then(|| v.to_owned());
        match k.trim() {
            "id" => n.id = v.parse().ok()?,
            "status" => n.status = NoteStatus::parse(v)?,
            "created" => n.created = v.parse().unwrap_or(0),
            "by" => n.by = v.to_owned(),
            "session" => n.session = some().unwrap_or_else(|| "none".into()),
            "conversation" => n.conversation = some(),
            "commit" => n.commit = some(),
            "cwd" => n.cwd = some().map(PathBuf::from),
            "anchors" => n.anchors = list(v),
            "sig" => n.sig = some(),
            "replaces" => n.replaces = v.parse().ok(),
            "replaced_by" => n.replaced_by = v.parse().ok(),
            _ => {}
        }
    }
    (n.id > 0).then_some(n)
}

/// Hashes of the words that carry `text`'s meaning: what a tombstone keeps instead
/// of the text, enough for [`extract::similar`]'s test.
pub fn word_hashes(text: &str) -> BTreeSet<String> {
    extract::words(text)
        .iter()
        .map(|w| format!("{:016x}", project::fnv1a(w.as_bytes())))
        .collect()
}

/// The tombstone `text` (with failure signature `sig`) matches, if any: the same
/// signature, or most of the same words.
pub fn buried<'a>(
    text: &str,
    sig: Option<&str>,
    deleted: &'a [Tombstone],
) -> Option<&'a Tombstone> {
    let words = word_hashes(text);
    deleted.iter().find(|t| {
        (sig.is_some() && t.sig.as_deref() == sig)
            || (!words.is_empty() && extract::similar(&words, &t.words.iter().cloned().collect()))
    })
}

/// Why a live note may be out of date (ADR-0017 §6): a file it names was deleted,
/// or changed by [`stale::CHURN_LINES`] lines since the commit it was written at.
/// Runs `git` in `root`.
pub fn staleness(root: &Path, n: &Note) -> Option<String> {
    if n.status != NoteStatus::Live {
        return None;
    }
    match stale::changed(root, &n.anchors, n.commit.as_deref())? {
        stale::Change::Gone(gone) => Some(format!("{gone} no longer exists")),
        stale::Change::Churn { added, removed } => Some(format!(
            "{} changed a lot since it was written (+{added}/-{removed})",
            n.anchors.join(", ")
        )),
    }
}

impl Store {
    fn notes_dir(&self, root: &Path) -> PathBuf {
        self.base.join(project::key(root)).join("notes")
    }

    fn note_files(&self, root: &Path) -> BTreeMap<u32, (PathBuf, Note)> {
        let mut out = BTreeMap::new();
        let Ok(entries) = std::fs::read_dir(self.notes_dir(root)) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "md") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(n) = parse(&text, root.to_path_buf()) {
                out.insert(n.id, (path, n));
            }
        }
        // A replacement is written before the note it replaces is marked: if the
        // second write never happened, the replacement still wins, so a correction
        // and what it corrects never both read as live.
        let replaced: Vec<(u32, u32)> = out
            .values()
            .filter_map(|(_, n)| Some((n.replaces?, n.id)))
            .collect();
        for (old, new) in replaced {
            if let Some((_, n)) = out.get_mut(&old)
                && n.status == NoteStatus::Live
            {
                n.status = NoteStatus::Replaced;
                n.replaced_by = Some(new);
            }
        }
        out
    }

    /// A project's notes, every status, by id. Files that don't parse are skipped.
    pub fn notes(&self, root: &Path) -> Vec<Note> {
        self.note_files(root)
            .into_values()
            .map(|(_, n)| n)
            .collect()
    }

    /// A project's tombstones, oldest first.
    pub fn deleted_notes(&self, root: &Path) -> Vec<Tombstone> {
        std::fs::read_to_string(self.notes_dir(root).join(DELETED))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Writes a note. `provenance` is the daemon's, never the client's: `by` is
    /// `agent`, `user` or `valkyrie`, and the session, conversation and commit are
    /// what it found for the connecting process. `sig` is the failure the note
    /// came from, for the retro's.
    pub fn note(
        &self,
        new: &NewNote,
        provenance: Provenance,
        sig: Option<String>,
        now: u64,
    ) -> Result<Note> {
        let root = project::root(&new.cwd);
        let text = shown(&new.text, true).trim().to_owned();
        if text.is_empty() {
            bail!("a note needs some text");
        }
        let len = text.chars().count();
        if len > MAX_NOTE {
            bail!("the note is {len} characters, over {MAX_NOTE}; a note is one short finding");
        }
        if new.force && provenance.by != "user" {
            bail!(
                "--force is for the user, not an agent: a deleted note stays deleted unless they \
                 write it again from their own terminal"
            );
        }
        let files = self.note_files(&root);
        let old = match new.replaces {
            None => None,
            Some(old) => match files.get(&old) {
                None => bail!("no note #{old} in {}", root.display()),
                Some((_, n)) => match n.status {
                    NoteStatus::Live => Some(n),
                    // One chain, never a fork: correct the newest.
                    NoteStatus::Replaced => match n.replaced_by {
                        Some(newer) => bail!(
                            "#{old} was already replaced by #{newer}; replace #{newer} instead"
                        ),
                        None => bail!("#{old} was already replaced"),
                    },
                    NoteStatus::Promoted => {
                        bail!("#{old} became a decision; propose a change to the decision instead")
                    }
                },
            },
        };
        // A correction keeps its failure's signature, so recurrence still finds it.
        let sig = sig.or_else(|| old.and_then(|o| o.sig.clone()));
        let deleted = self.deleted_notes(&root);
        if !new.force
            && let Some(t) = buried(&text, sig.as_deref(), &deleted)
        {
            bail!(
                "this matches note #{} that {} deleted on {}, and a deleted note isn't written \
                 again. If it's worth keeping, ask the user: `valk note --force` from their own \
                 terminal writes it anyway",
                t.id,
                match t.by.as_str() {
                    "user" => "the user",
                    _ => "an agent",
                },
                crate::docs::rfc3339(t.at).get(..10).unwrap_or("?"),
            );
        }
        let anchors = stale::anchors(&root, &new.cwd, &text);
        let note = Note {
            id: next_id(&files, &deleted),
            project: root.clone(),
            text,
            status: NoteStatus::Live,
            created: now,
            by: provenance.by,
            session: provenance.session.unwrap_or_else(|| "none".into()),
            conversation: provenance.conversation,
            commit: provenance.commit.filter(|c| stale::valid_commit(c)),
            cwd: Some(new.cwd.clone()),
            anchors,
            sig,
            replaces: new.replaces,
            replaced_by: None,
            stale: None,
        };
        self.mark_project(&root)?;
        let dir = self.notes_dir(&root);
        valkyrie_proto::ensure_private_dir(&dir)?;
        // The new one first: if the second write fails, loading still reads the
        // old one as replaced (see `note_files`).
        write_atomic(&dir.join(name(note.id)), &render(&note))?;
        if let Some(old) = old {
            let mut old = old.clone();
            old.status = NoteStatus::Replaced;
            old.replaced_by = Some(note.id);
            write_atomic(&dir.join(name(old.id)), &render(&old))?;
        }
        Ok(note)
    }

    /// Deletes note `id`'s file and records a tombstone. `by` is `agent` or
    /// `user`, as the daemon found the caller.
    pub fn rm_note(
        &self,
        root: &Path,
        id: u32,
        by: &str,
        session: Option<String>,
        now: u64,
    ) -> Result<Tombstone> {
        let files = self.note_files(root);
        let Some((path, note)) = files.get(&id) else {
            bail!("no note #{id} in {}", root.display());
        };
        let tombstone = Tombstone {
            id,
            sig: note.sig.clone(),
            words: word_hashes(&note.text).into_iter().collect(),
            by: by.to_owned(),
            session: session.unwrap_or_else(|| "none".into()),
            at: now,
        };
        // The tombstone first: a note gone without one could be written again.
        let dir = self.notes_dir(root);
        let line = serde_json::to_string(&tombstone)? + "\n";
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(DELETED))
                .with_context(|| format!("open {}", dir.join(DELETED).display()))?;
            f.write_all(line.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
        Ok(tombstone)
    }
}

/// One past the highest id a note has had, deleted ones included, so a
/// tombstone's id never names a newer note.
fn next_id(files: &BTreeMap<u32, (PathBuf, Note)>, deleted: &[Tombstone]) -> u32 {
    files
        .keys()
        .copied()
        .chain(deleted.iter().map(|t| t.id))
        .max()
        .map_or(1, |id| id + 1)
}

/// A note's file name: its id, four digits.
fn name(id: u32) -> String {
    format!("{id:04}.md")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory, removed when dropped.
    struct Tmp(PathBuf);

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn store() -> (Tmp, Store, PathBuf) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let dir = Tmp(std::env::temp_dir().join(format!(
            "valk-notes-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        )));
        let _ = std::fs::remove_dir_all(&dir.0);
        let repo = dir.0.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join("src/build.rs"), "fn main() {}\n").unwrap();
        let store = Store::new(dir.0.join("context"));
        (dir, store, repo)
    }

    fn by(who: &str, session: Option<&str>) -> Provenance {
        Provenance {
            by: who.into(),
            session: session.map(str::to_owned),
            conversation: Some("conv-1".into()),
            commit: Some("abc1234".into()),
            cwd: None,
        }
    }

    fn new(repo: &Path, text: &str) -> NewNote {
        NewNote {
            cwd: repo.to_path_buf(),
            text: text.into(),
            replaces: None,
            force: false,
        }
    }

    #[test]
    fn a_note_records_where_it_came_from_and_reads_back() {
        let (_d, store, repo) = store();
        let n = store
            .note(
                &new(&repo, "src/build.rs is slow \u{202e}on first run\u{7}"),
                by("agent", Some("s1")),
                None,
                10,
            )
            .unwrap();
        assert_eq!(n.id, 1);
        assert_eq!(n.text, "src/build.rs is slow on first run");
        assert_eq!(n.session, "s1");
        assert_eq!(n.commit.as_deref(), Some("abc1234"));
        assert_eq!(n.anchors, vec!["src/build.rs".to_owned()]);
        let file = store.notes_dir(&repo).join("0001.md");
        assert!(file.exists());
        let back = store.notes(&repo);
        assert_eq!(back, vec![n]);
    }

    #[test]
    fn outside_a_session_it_says_none_and_a_bad_commit_is_dropped() {
        let (_d, store, repo) = store();
        let mut p = by("user", None);
        p.commit = Some("--output=/etc/x".into());
        let n = store
            .note(&new(&repo, "the cache lives in target"), p, None, 1)
            .unwrap();
        assert_eq!(n.session, "none");
        assert_eq!(n.commit, None);
    }

    #[test]
    fn too_long_or_empty_is_refused() {
        let (_d, store, repo) = store();
        let long = "x".repeat(MAX_NOTE + 1);
        let err = store.note(&new(&repo, &long), by("agent", None), None, 1);
        assert!(err.unwrap_err().to_string().contains("over 1000"));
        assert!(
            store
                .note(&new(&repo, " \u{7} "), by("agent", None), None, 1)
                .is_err()
        );
        assert!(
            store
                .note(
                    &new(&repo, &"x".repeat(MAX_NOTE)),
                    by("agent", None),
                    None,
                    1
                )
                .is_ok()
        );
    }

    #[test]
    fn a_correction_replaces_and_a_chain_never_forks() {
        let (_d, store, repo) = store();
        store
            .note(
                &new(&repo, "run tests with --release"),
                by("agent", None),
                Some("ab".into()),
                1,
            )
            .unwrap();
        let mut fix = new(&repo, "run tests without --release, it hides the overflow");
        fix.replaces = Some(1);
        let two = store.note(&fix, by("agent", None), None, 2).unwrap();
        assert_eq!(two.replaces, Some(1));
        assert_eq!(
            two.sig.as_deref(),
            Some("ab"),
            "a correction keeps its signature"
        );
        let notes = store.notes(&repo);
        assert_eq!(notes[0].status, NoteStatus::Replaced);
        assert_eq!(notes[0].replaced_by, Some(2));
        assert_eq!(notes[1].status, NoteStatus::Live);
        // Replacing #1 again is refused with the newer id.
        let err = store
            .note(&fix, by("agent", None), None, 3)
            .unwrap_err()
            .to_string();
        assert!(err.contains("replaced by #2"), "{err}");
        let mut missing = new(&repo, "x y z");
        missing.replaces = Some(9);
        assert!(store.note(&missing, by("agent", None), None, 3).is_err());
    }

    #[test]
    fn a_replacement_whose_second_write_never_happened_still_wins() {
        let (_d, store, repo) = store();
        store
            .note(&new(&repo, "first take"), by("agent", None), None, 1)
            .unwrap();
        let mut fix = new(&repo, "second take");
        fix.replaces = Some(1);
        store.note(&fix, by("agent", None), None, 2).unwrap();
        // Put #1 back as it was before it was marked.
        let one = store.notes_dir(&repo).join("0001.md");
        let text = std::fs::read_to_string(&one).unwrap();
        let text = text
            .replace("status: replaced", "status: live")
            .replace("replaced_by: 2\n", "");
        std::fs::write(&one, text).unwrap();
        let notes = store.notes(&repo);
        assert_eq!(notes[0].status, NoteStatus::Replaced);
        assert_eq!(notes[0].replaced_by, Some(2));
    }

    #[test]
    fn a_deleted_note_is_not_written_again_unless_the_user_forces_it() {
        let (_d, store, repo) = store();
        store
            .note(
                &new(&repo, "the flaky test is net::retry, rerun it"),
                by("agent", None),
                Some("ff".into()),
                1,
            )
            .unwrap();
        let t = store
            .rm_note(&repo, 1, "agent", Some("s9".into()), 5)
            .unwrap();
        assert_eq!(t.sig.as_deref(), Some("ff"));
        assert!(store.notes(&repo).is_empty());
        let deleted = store.deleted_notes(&repo);
        assert_eq!(deleted, vec![t]);
        // The tombstone holds no words of the text.
        let raw = std::fs::read_to_string(store.notes_dir(&repo).join(DELETED)).unwrap();
        assert!(!raw.contains("flaky"), "{raw}");

        // Reworded, same words mostly: refused.
        let err = store
            .note(
                &new(&repo, "The flaky test is net::retry; just rerun it"),
                by("agent", None),
                None,
                6,
            )
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("#1") && err.contains("an agent deleted"),
            "{err}"
        );
        // By signature, whatever the words.
        let err = store.note(
            &new(&repo, "something else entirely"),
            by("agent", None),
            Some("ff".into()),
            6,
        );
        assert!(err.is_err());
        // An agent can't force it; the user can.
        let mut forced = new(&repo, "the flaky test is net::retry, rerun it");
        forced.force = true;
        assert!(store.note(&forced, by("agent", None), None, 7).is_err());
        let n = store.note(&forced, by("user", None), None, 7).unwrap();
        // Ids never go back to a deleted one's.
        assert_eq!(n.id, 2);
        // Unrelated text is fine.
        assert!(
            store
                .note(
                    &new(&repo, "builds need protoc installed"),
                    by("agent", None),
                    None,
                    8
                )
                .is_ok()
        );
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn a_note_goes_stale_when_its_files_churn_or_go() {
        let (_d, store, repo) = store();
        std::fs::remove_dir_all(repo.join(".git")).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("src/lib.rs"), "a\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "one"]);
        let head = git(&repo, &["rev-parse", "--short", "HEAD"]);
        let mut p = by("agent", None);
        p.commit = Some(head);
        let mut lib = store
            .note(
                &new(&repo, "src/lib.rs needs a rebuild after edits"),
                p.clone(),
                None,
                1,
            )
            .unwrap();
        let build = store
            .note(&new(&repo, "src/build.rs reads an env var"), p, None, 1)
            .unwrap();
        assert_eq!(staleness(&repo, &lib), None);
        std::fs::write(repo.join("src/lib.rs"), "b\n".repeat(200)).unwrap();
        std::fs::remove_file(repo.join("src/build.rs")).unwrap();
        git(&repo, &["commit", "-qam", "two"]);
        let why = staleness(&repo, &lib).unwrap();
        assert!(
            why.contains("src/lib.rs changed a lot since it was written"),
            "{why}"
        );
        assert_eq!(
            staleness(&repo, &build).as_deref(),
            Some("src/build.rs no longer exists")
        );
        // Only a live note is stale; a replaced one is never served anyway.
        lib.status = NoteStatus::Replaced;
        assert_eq!(staleness(&repo, &lib), None);
    }

    #[test]
    fn rm_of_a_missing_note_is_refused() {
        let (_d, store, repo) = store();
        assert!(store.rm_note(&repo, 3, "user", None, 1).is_err());
    }

    #[test]
    fn render_and_parse_round_trip_every_field() {
        let n = Note {
            id: 12,
            project: "/r".into(),
            text: "line one\nline two".into(),
            status: NoteStatus::Replaced,
            created: 99,
            by: "valkyrie".into(),
            session: "s".into(),
            conversation: Some("c".into()),
            commit: Some("abcd".into()),
            cwd: Some("/r/sub".into()),
            anchors: vec!["a.rs".into(), "b/c.md".into()],
            sig: Some("00ff".into()),
            replaces: Some(3),
            replaced_by: Some(14),
            stale: None,
        };
        assert_eq!(parse(&render(&n), "/r".into()), Some(n));
    }
}

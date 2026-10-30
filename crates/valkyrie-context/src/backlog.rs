//! The backlog (DESIGN §8.12): one JSON file per item under `<state dir>/backlog`,
//! with its attachments copied beside it, since the uploads folder is pruned after
//! a week and an item may wait longer than that.

use crate::{MAX_TITLE, file, project, shown, write_atomic};
use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use valkyrie_proto::{Item, ItemChange, ItemKind, ItemNote, ItemStatus, NewItem, SessionId};

/// Longest body kept: room for a bug report with its logs, not a whole file.
pub const MAX_BODY: usize = 20_000;
/// Largest attachment copied, the web app's upload limit.
pub const MAX_ATTACHMENT: u64 = 50 * 1024 * 1024;
const MAX_ATTACHMENTS: usize = 20;
/// Longest handoff note: what changed and what's left, not a transcript.
pub const MAX_NOTE: usize = 4000;
/// Notes kept per item, the newest.
const MAX_NOTES: usize = 20;

pub struct Backlog {
    base: PathBuf,
}

impl Backlog {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// `<state dir>/backlog`.
    pub fn default_base() -> PathBuf {
        valkyrie_proto::state_dir().join("backlog")
    }

    fn path(&self, id: u32) -> PathBuf {
        self.base.join(format!("{id:04}.json"))
    }

    /// Every item, oldest first. Files that don't parse are skipped.
    pub fn load(&self) -> Vec<Item> {
        let Ok(entries) = std::fs::read_dir(&self.base) else {
            return Vec::new();
        };
        let mut items: Vec<Item> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .filter_map(|text| serde_json::from_str(&text).ok())
            .collect();
        items.sort_by_key(|i: &Item| i.id);
        items
    }

    pub fn get(&self, id: u32) -> Result<Item> {
        let text =
            std::fs::read_to_string(self.path(id)).with_context(|| format!("no item #{id}"))?;
        serde_json::from_str(&text).with_context(|| format!("item #{id} doesn't parse"))
    }

    /// Writes `new` down as the next item. `by` is `human` or the agent asking,
    /// worked out by the daemon; an agent's waits, proposed, for the user.
    pub fn add(&self, new: &NewItem, by: &str, now: u64) -> Result<Item> {
        let (title, body) = clean(&new.title, &new.body)?;
        if new.attachments.len() > MAX_ATTACHMENTS {
            bail!("at most {MAX_ATTACHMENTS} attachments");
        }
        valkyrie_proto::ensure_private_dir(&self.base)?;
        let id = self
            .load()
            .iter()
            .map(|i| i.id)
            .max()
            .unwrap_or(0)
            .max(self.last())
            + 1;
        write_atomic(&self.base.join("last"), &id.to_string())?;
        let attachments = self.keep(id, &new.attachments)?;
        let item = Item {
            id,
            title,
            body,
            kind: new.kind,
            status: if by == "human" {
                ItemStatus::Open
            } else {
                ItemStatus::Proposed
            },
            // Outside git there's no project to work in, whichever client asked.
            project: new
                .cwd
                .as_deref()
                .filter(|d| project::in_git(d))
                .map(project::root),
            attachments,
            by: by.to_owned(),
            sessions: Vec::new(),
            boot: 0,
            branch: None,
            checkout: None,
            notes: Vec::new(),
            created: now,
            updated: now,
        };
        self.write(&item)?;
        Ok(item)
    }

    /// Leaves a handoff note on item `id`. Who may is the daemon's to check.
    pub fn note(&self, id: u32, by: &str, text: &str, now: u64) -> Result<Item> {
        let text = shown(text, true).trim().to_owned();
        if text.is_empty() {
            bail!("say what happened");
        }
        if text.chars().count() > MAX_NOTE {
            bail!("a note is at most {MAX_NOTE} characters: what changed and what's left");
        }
        let mut item = self.get(id)?;
        item.notes.push(ItemNote {
            at: now,
            by: by.to_owned(),
            text,
        });
        let over = item.notes.len().saturating_sub(MAX_NOTES);
        item.notes.drain(..over);
        item.updated = now;
        self.write(&item)?;
        Ok(item)
    }

    /// The highest id ever given out, so one deleted isn't given out again with
    /// branches and sessions still named after the old one.
    fn last(&self) -> u32 {
        std::fs::read_to_string(self.base.join("last"))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Forgets item `id` and its attachments.
    pub fn delete(&self, id: u32) -> Result<Item> {
        let item = self.get(id)?;
        let files = self.base.join("files").join(format!("{id:04}"));
        if files.exists() {
            std::fs::remove_dir_all(&files)
                .with_context(|| format!("remove {}", files.display()))?;
        }
        std::fs::remove_file(self.path(id)).with_context(|| format!("remove #{id}"))?;
        Ok(item)
    }

    /// Copies attachments into `files/<id>/`, so they outlive where they came from.
    fn keep(&self, id: u32, files: &[PathBuf]) -> Result<Vec<PathBuf>> {
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let dir = self.base.join("files").join(format!("{id:04}"));
        valkyrie_proto::ensure_private_dir(&self.base.join("files"))?;
        valkyrie_proto::ensure_private_dir(&dir)?;
        let mut kept = Vec::new();
        for (n, from) in files.iter().enumerate() {
            let meta = std::fs::metadata(from)
                .with_context(|| format!("can't read {}", from.display()))?;
            if !meta.is_file() {
                bail!("{} isn't a file", from.display());
            }
            if meta.len() > MAX_ATTACHMENT {
                bail!("{} is over 50 MB", from.display());
            }
            let name = from.file_name().and_then(|n| n.to_str()).unwrap_or("file");
            // Numbered, so two files of one name (two screenshots) both stay.
            let to = dir.join(format!(
                "{}-{}",
                n + 1,
                valkyrie_proto::pastes::safe_name(name)
            ));
            std::fs::copy(from, &to).with_context(|| format!("copy {}", from.display()))?;
            kept.push(to);
        }
        Ok(kept)
    }

    pub fn change(&self, id: u32, change: &ItemChange, now: u64) -> Result<Item> {
        let mut item = self.get(id)?;
        match change {
            ItemChange::Edit {
                title,
                body,
                kind,
                cwd,
            } => {
                (item.title, item.body) = clean(title, body)?;
                item.kind = *kind;
                if let Some(cwd) = cwd {
                    let root = project::root(cwd);
                    if item.checkout.is_some() && item.project.as_ref() != Some(&root) {
                        bail!("#{id} already has a checkout in {}", display(&item.project));
                    }
                    item.project = Some(root);
                }
            }
            ItemChange::Status { status } => item.status = *status,
        }
        item.updated = now;
        self.write(&item)?;
        Ok(item)
    }

    /// Records a session started on `id`, and the checkout it runs in.
    pub fn launched(
        &self,
        id: u32,
        session: SessionId,
        boot: u64,
        checkout: Option<(String, PathBuf)>,
        now: u64,
    ) -> Result<Item> {
        let mut item = self.get(id)?;
        if item.boot != boot {
            item.sessions.clear();
            item.boot = boot;
        }
        item.sessions.push(session);
        if let Some((branch, path)) = checkout {
            item.branch = Some(branch);
            item.checkout = Some(path);
        }
        item.updated = now;
        self.write(&item)?;
        Ok(item)
    }

    /// Forgets the checkout once it's gone.
    pub fn cleared(&self, id: u32, now: u64) -> Result<Item> {
        let mut item = self.get(id)?;
        item.checkout = None;
        item.updated = now;
        self.write(&item)?;
        Ok(item)
    }

    fn write(&self, item: &Item) -> Result<()> {
        valkyrie_proto::ensure_private_dir(&self.base)?;
        write_atomic(&self.path(item.id), &serde_json::to_string_pretty(item)?)
    }
}

fn display(p: &Option<PathBuf>) -> String {
    p.as_deref()
        .map_or("no folder".into(), |p| p.display().to_string())
}

/// A title of its own, or the body's first line when there's none (a note dictated
/// on a phone is all body). A first line taken whole leaves the body, so the brief
/// doesn't say it twice; a cut one stays, as the only place it's said in full.
fn clean(title: &str, body: &str) -> Result<(String, String)> {
    let mut body = shown(body, true).trim().to_owned();
    let mut title = file::one_line(&shown(title, false));
    if title.is_empty() {
        let (first, rest) = body.split_once('\n').unwrap_or((&body, ""));
        title = file::one_line(first);
        if title.chars().count() > 80 {
            title = format!("{}…", title.chars().take(79).collect::<String>().trim_end());
        } else {
            body = rest.trim().to_owned();
        }
    }
    if title.is_empty() {
        bail!("say what it is");
    }
    if title.chars().count() > MAX_TITLE {
        bail!("title is over {MAX_TITLE} characters; put the detail in the body");
    }
    if body.chars().count() > MAX_BODY {
        bail!("body is over {MAX_BODY} characters");
    }
    Ok((title, body))
}

/// The branch a launch makes: `fix/12-tab-crash`, `feat/12-…` or `chore/12-…`,
/// as the repo's own branches are named. Only ASCII letters, digits and `-`, so it
/// is always a valid ref.
pub fn branch(item: &Item) -> String {
    let prefix = match item.kind {
        ItemKind::Bug => "fix",
        ItemKind::Feature => "feat",
        ItemKind::Chore => "chore",
    };
    let mut slug = String::new();
    for word in item
        .title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .take(5)
    {
        if !slug.is_empty() {
            slug.push('-');
        }
        slug.push_str(&word.to_ascii_lowercase());
    }
    match slug.is_empty() {
        true => format!("{prefix}/{}", item.id),
        false => format!(
            "{prefix}/{}-{}",
            item.id,
            slug.chars()
                .take(40)
                .collect::<String>()
                .trim_end_matches('-')
        ),
    }
}

/// What the agent is told first: a brief for the kind of work, then the item.
/// Ideas written on a phone are rough, so a feature asks for a plan and questions
/// before code; a bug asks for a reproduction before a fix.
pub fn brief(item: &Item) -> String {
    let how = match item.kind {
        ItemKind::Bug => {
            "Fix this bug from my backlog. Reproduce it first, as a failing test where you can, then fix it and show the test passing. If you can't reproduce it, stop and tell me what you tried."
        }
        ItemKind::Feature => {
            "Build this feature from my backlog. It was written down quickly, so read the relevant code first and give me a short plan. If anything about what I want is unclear, ask me before writing code."
        }
        ItemKind::Chore => "Do this task from my backlog.",
    };
    let mut text = format!("{how}\n\n# {}\n", item.title);
    if !item.body.is_empty() {
        text.push('\n');
        text.push_str(&item.body);
        text.push('\n');
    }
    if !item.attachments.is_empty() {
        text.push_str("\nAttached (screenshots and files; read them):\n");
        for a in &item.attachments {
            text.push_str(&format!("- {}\n", a.display()));
        }
    }
    if !item.notes.is_empty() {
        // Written by the user, or by an agent on an earlier launch of this item.
        text.push_str("\nNotes from earlier work on this, oldest first:\n");
        for n in &item.notes {
            let by = if n.by == "human" { "me" } else { &n.by };
            text.push_str(&format!("- {by}: {}\n", n.text.replace('\n', "\n  ")));
        }
    }
    text.push_str(&format!(
        "\n(This is item #{} in my Valkyrie backlog. Anything else you find along the way goes in with `valk todo add`, not into this change. When you finish or stop, leave a short handoff with `valk todo note \"<what changed, what's left, anything I should check>\"`.)\n",
        item.id
    ));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A scratch folder, taken away when the test ends, pass or fail.
    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("valk-backlog-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn new(title: &str, body: &str, kind: ItemKind) -> NewItem {
        NewItem {
            title: title.into(),
            body: body.into(),
            kind,
            ..Default::default()
        }
    }

    #[test]
    fn items_are_numbered_and_listed_in_order() {
        let dir = Temp::new();
        let b = Backlog::new(dir.path().join("b"));
        let one = b
            .add(&new("first", "", ItemKind::Bug), "human", 10)
            .unwrap();
        let two = b
            .add(&new("second", "", ItemKind::Chore), "claude", 11)
            .unwrap();
        assert_eq!((one.id, two.id), (1, 2));
        let all = b.load();
        assert_eq!(
            all.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(all[1].by, "claude");
        assert_eq!(all[0].status, ItemStatus::Open);
    }

    #[test]
    fn a_note_with_no_title_takes_its_first_line() {
        let dir = Temp::new();
        let b = Backlog::new(dir.path().join("b"));
        let item = b
            .add(
                &new(
                    "",
                    "  tab rename eats the last letter\nseen on the phone",
                    ItemKind::Bug,
                ),
                "human",
                1,
            )
            .unwrap();
        assert_eq!(item.title, "tab rename eats the last letter");
        assert_eq!(item.body, "seen on the phone");
        let item = b
            .add(&new("", "just a title", ItemKind::Bug), "human", 1)
            .unwrap();
        assert_eq!(
            (item.title.as_str(), item.body.as_str()),
            ("just a title", "")
        );
        let long = "word ".repeat(40);
        let item = b.add(&new("", &long, ItemKind::Bug), "human", 1).unwrap();
        assert!(item.title.ends_with('…') && item.title.chars().count() <= 80);
        assert_eq!(item.body, long.trim());
        assert!(b.add(&new(" ", "\n", ItemKind::Bug), "human", 1).is_err());
    }

    #[test]
    fn text_loses_control_characters() {
        let dir = Temp::new();
        let b = Backlog::new(dir.path().join("b"));
        let item = b
            .add(
                &new(
                    "evil\u{1b}[2Jtitle\u{202e}",
                    "line\u{7}\nnext",
                    ItemKind::Feature,
                ),
                "human",
                1,
            )
            .unwrap();
        assert_eq!(item.title, "evil [2Jtitle");
        assert_eq!(item.body, "line\nnext");
    }

    #[test]
    fn attachments_are_copied_and_survive_the_original() {
        let dir = Temp::new();
        let b = Backlog::new(dir.path().join("backlog"));
        let shot = dir.path().join("123-shot.png");
        std::fs::write(&shot, b"png").unwrap();
        let mut n = new("bug", "", ItemKind::Bug);
        n.attachments = vec![shot.clone(), shot.clone()];
        let item = b.add(&n, "human", 1).unwrap();
        std::fs::remove_file(&shot).unwrap();
        assert_eq!(item.attachments.len(), 2);
        assert_ne!(item.attachments[0], item.attachments[1]);
        for a in &item.attachments {
            assert!(a.starts_with(dir.path().join("backlog/files/0001")));
            assert_eq!(std::fs::read(a).unwrap(), b"png");
        }
        n.attachments = vec![dir.path().to_path_buf()];
        assert!(
            b.add(&n, "human", 1).is_err(),
            "a folder isn't an attachment"
        );
    }

    #[test]
    fn edits_and_status_changes_stick() {
        let dir = Temp::new();
        let b = Backlog::new(dir.path().join("b"));
        b.add(&new("x", "", ItemKind::Feature), "human", 1).unwrap();
        let edit = ItemChange::Edit {
            title: "y".into(),
            body: "more".into(),
            kind: ItemKind::Bug,
            cwd: None,
        };
        b.change(1, &edit, 2).unwrap();
        b.change(
            1,
            &ItemChange::Status {
                status: ItemStatus::Shipped,
            },
            3,
        )
        .unwrap();
        let item = b.get(1).unwrap();
        assert_eq!(
            (item.title.as_str(), item.kind, item.status, item.updated),
            ("y", ItemKind::Bug, ItemStatus::Shipped, 3)
        );
        assert!(b.change(9, &edit, 4).is_err());
    }

    #[test]
    fn an_agents_item_waits_proposed_and_its_brief_names_it() {
        let dir = Temp::new();
        let b = Backlog::new(dir.path().join("b"));
        let mine = b.add(&new("mine", "", ItemKind::Bug), "human", 1).unwrap();
        let theirs = b
            .add(&new("theirs", "", ItemKind::Bug), "claude", 1)
            .unwrap();
        assert_eq!(
            (mine.status, theirs.status),
            (ItemStatus::Open, ItemStatus::Proposed)
        );
        assert!(brief(&theirs).contains("item #2 in my Valkyrie backlog"));
    }

    #[test]
    fn notes_are_kept_newest_last_and_go_into_the_next_brief() {
        let dir = Temp::new();
        let b = Backlog::new(dir.path().join("b"));
        b.add(&new("crash", "", ItemKind::Bug), "human", 1).unwrap();
        assert!(b.note(1, "claude", " \n", 2).is_err());
        assert!(b.note(1, "claude", &"x".repeat(MAX_NOTE + 1), 2).is_err());
        b.note(1, "claude", "fixed the parser\nstill flaky on Windows", 2)
            .unwrap();
        let item = b.note(1, "human", "check the Windows CI", 3).unwrap();
        assert_eq!(item.notes.len(), 2);
        assert_eq!((item.notes[0].by.as_str(), item.updated), ("claude", 3));
        let text = brief(&item);
        assert!(
            text.contains(
                "- claude: fixed the parser\n  still flaky on Windows\n- me: check the Windows CI\n"
            ),
            "{text}"
        );
        for n in 0..30 {
            b.note(1, "claude", &n.to_string(), 4).unwrap();
        }
        let notes = b.get(1).unwrap().notes;
        assert_eq!((notes.len(), notes[19].text.as_str()), (MAX_NOTES, "29"));
    }

    #[test]
    fn a_deleted_item_takes_its_files_and_its_number_isnt_given_out_again() {
        let dir = Temp::new();
        let b = Backlog::new(dir.path().join("b"));
        let shot = dir.path().join("shot.png");
        std::fs::write(&shot, b"png").unwrap();
        let mut with_shot = new("one", "", ItemKind::Bug);
        with_shot.attachments = vec![shot];
        b.add(&new("zero", "", ItemKind::Bug), "human", 1).unwrap();
        let item = b.add(&with_shot, "human", 1).unwrap();
        assert_eq!(item.id, 2);
        assert_eq!(b.delete(2).unwrap().title, "one");
        assert!(!item.attachments[0].exists());
        assert!(b.get(2).is_err() && b.delete(2).is_err());
        assert_eq!(
            b.add(&new("two", "", ItemKind::Bug), "human", 1)
                .unwrap()
                .id,
            3
        );
        assert_eq!(b.load().iter().map(|i| i.id).collect::<Vec<_>>(), [1, 3]);
    }

    #[test]
    fn launches_from_an_older_daemon_are_forgotten() {
        let dir = Temp::new();
        let b = Backlog::new(dir.path().join("b"));
        b.add(&new("x", "", ItemKind::Feature), "human", 1).unwrap();
        let wt = Some(("feat/1-x".to_owned(), PathBuf::from("/r-feat-1-x")));
        b.launched(1, 4, 77, wt, 2).unwrap();
        let item = b.launched(1, 5, 77, None, 3).unwrap();
        assert_eq!(item.sessions, [4, 5]);
        assert_eq!(item.branch.as_deref(), Some("feat/1-x"));
        let item = b.launched(1, 1, 78, None, 4).unwrap();
        assert_eq!((item.sessions.as_slice(), item.boot), (&[1][..], 78));
        assert_eq!(item.checkout, Some(PathBuf::from("/r-feat-1-x")));
    }

    #[test]
    fn branches_follow_the_kind_and_title() {
        let mk = |id, title: &str, kind| Item {
            id,
            title: title.into(),
            body: String::new(),
            kind,
            status: ItemStatus::Open,
            project: None,
            attachments: vec![],
            by: "human".into(),
            sessions: vec![],
            boot: 0,
            branch: None,
            checkout: None,
            notes: Vec::new(),
            created: 0,
            updated: 0,
        };
        assert_eq!(
            branch(&mk(12, "Tab rename eats the last letter!", ItemKind::Bug)),
            "fix/12-tab-rename-eats-the-last"
        );
        assert_eq!(
            branch(&mk(3, "Backlog: kanban-ish view", ItemKind::Feature)),
            "feat/3-backlog-kanban-ish-view"
        );
        assert_eq!(branch(&mk(4, "日本語", ItemKind::Chore)), "chore/4");
        let b = brief(&Item {
            body: "steps".into(),
            attachments: vec![PathBuf::from("/s/1-shot.png")],
            ..mk(5, "crash", ItemKind::Bug)
        });
        assert!(b.starts_with("Fix this bug"));
        assert!(b.contains("# crash\n\nsteps\n"));
        assert!(b.contains("- /s/1-shot.png\n"));
        assert!(b.ends_with("anything I should check>\"`.)\n"));
    }
}

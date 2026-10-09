//! The project context layer (DESIGN §6, ADR-0007): decisions kept per project as
//! markdown files in the state dir, their review lifecycle, and the block injected
//! into agents.

pub mod extract;
mod file;
pub mod handoff;
pub mod inject;
pub mod model;
pub mod project;
pub mod stale;

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use valkyrie_proto::{Decision, DecisionStatus, NewDecision, Provenance, ReviewAction};

pub use file::{name as file_name, parse, render};

/// Longest title kept; titles are one line in every list.
pub const MAX_TITLE: usize = 200;
/// Longest body kept; a decision is a few sentences, not a design doc.
pub const MAX_BODY: usize = 4000;

/// Every project's decisions, under one directory (`<state dir>/context`).
pub struct Store {
    base: PathBuf,
}

impl Store {
    pub fn new(base: impl Into<PathBuf>) -> Self {
        Self { base: base.into() }
    }

    /// `<state dir>/context`.
    pub fn default_base() -> PathBuf {
        valkyrie_proto::state_dir().join("context")
    }

    fn dir(&self, root: &Path) -> PathBuf {
        self.base.join(project::key(root)).join("decisions")
    }

    /// Every project that has decisions.
    pub fn projects(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.base) else {
            return Vec::new();
        };
        let mut roots: Vec<PathBuf> = entries
            .flatten()
            .filter_map(|e| std::fs::read_to_string(e.path().join("root")).ok())
            .map(|r| PathBuf::from(r.trim_end_matches('\n')))
            .collect();
        roots.sort();
        roots
    }

    /// A project's decisions by id. Files that don't parse are skipped, so one bad
    /// hand edit can't hide the rest.
    pub fn load(&self, root: &Path) -> Vec<Decision> {
        self.files(root).into_values().map(|(_, d)| d).collect()
    }

    fn files(&self, root: &Path) -> BTreeMap<u32, (PathBuf, Decision)> {
        let mut out = BTreeMap::new();
        let Ok(entries) = std::fs::read_dir(self.dir(root)) else {
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
            if let Some(d) = file::parse(&text, root.to_path_buf()) {
                out.insert(d.id, (path, d));
            }
        }
        out
    }

    /// Every project's decisions waiting on review, oldest first.
    pub fn proposals(&self) -> Vec<Decision> {
        let mut all: Vec<Decision> = self
            .projects()
            .iter()
            .flat_map(|root| self.load(root))
            .filter(|d| d.status == DecisionStatus::Proposed)
            .collect();
        all.sort_by_key(|d| (d.created, d.id));
        all
    }

    /// Records a decision: `active` when a human asks for it outright, else
    /// `proposed`. `provenance.by` must already be set by the caller (the daemon,
    /// which knows whether the session runs an agent).
    pub fn decide(&self, new: &NewDecision, provenance: Provenance, now: u64) -> Result<Decision> {
        let (title, body) = clean(&new.title, &new.body)?;
        let root = project::root(&new.cwd);
        let files = self.files(&root);
        if let Some(old) = new.supersedes {
            match files.get(&old) {
                Some((_, d)) if d.status == DecisionStatus::Active => {}
                Some((_, d)) => bail!("#{old} is {}, not active", d.status.as_str()),
                None => bail!("no decision #{old} in {}", root.display()),
            }
        }
        let by_human = provenance.by == "human";
        let text = format!("{title}\n{body}");
        let fresh = valkyrie_proto::Freshness {
            anchors: stale::anchors(&root, &new.cwd, &text),
            review_every: new
                .review_every
                .or_else(|| stale::volatile(&text).then_some(stale::VOLATILE_DAYS)),
            ..Default::default()
        };
        let decision = Decision {
            id: self.next_id(&root),
            project: root.clone(),
            title,
            body,
            kind: new.kind,
            status: if by_human && !new.propose {
                DecisionStatus::Active
            } else {
                DecisionStatus::Proposed
            },
            created: now,
            updated: now,
            supersedes: new.supersedes,
            superseded_by: None,
            fresh,
            provenance,
        };
        self.mark_project(&root)?;
        // The new one first: a failure between the two writes must not leave the
        // old one pointing at a decision that doesn't exist.
        self.write(&root, &decision, None)?;
        if decision.status == DecisionStatus::Active
            && let Some(old) = decision.supersedes
        {
            self.supersede(&root, old, decision.id, now)?;
        }
        Ok(decision)
    }

    pub fn review(
        &self,
        root: &Path,
        id: u32,
        action: &ReviewAction,
        seen: Option<u64>,
        now: u64,
    ) -> Result<Decision> {
        use DecisionStatus::*;
        let files = self.files(root);
        let Some((path, d)) = files.get(&id) else {
            bail!("no decision #{id} in {}", root.display());
        };
        if seen.is_some_and(|seen| seen != d.updated) {
            bail!("#{id} changed since you read it; look at it again");
        }
        let mut d = d.clone();
        let mut accepting = false;
        let mut confirming = false;
        let mut reworded = false;
        match (action, d.status) {
            (ReviewAction::Accept, Proposed) => accepting = true,
            (ReviewAction::Reject, Proposed) => d.status = Rejected,
            (ReviewAction::Retire, Active) => d.status = Retired,
            (ReviewAction::Confirm, Active) => confirming = true,
            (ReviewAction::Edit { title, body, kind }, Proposed | Active) => {
                (d.title, d.body) = clean(title, body)?;
                d.kind = *kind;
                reworded = true;
            }
            (ReviewAction::Revise { title, body, kind }, Proposed) => {
                (d.title, d.body) = clean(title, body)?;
                d.kind = *kind;
                accepting = true;
                reworded = true;
            }
            (action, status) => bail!(
                "can't {} #{id}: it is {}",
                match action {
                    ReviewAction::Accept => "accept",
                    ReviewAction::Reject => "reject",
                    ReviewAction::Retire => "retire",
                    ReviewAction::Confirm => "confirm",
                    ReviewAction::Edit { .. } => "edit",
                    ReviewAction::Revise { .. } => "revise",
                },
                status.as_str()
            ),
        }
        if accepting {
            d.status = Active;
        }
        if reworded {
            let cwd = d
                .provenance
                .cwd
                .clone()
                .unwrap_or_else(|| root.to_path_buf());
            d.fresh.anchors = stale::anchors(root, &cwd, &format!("{}\n{}", d.title, d.body));
        }
        // Confirmed as of now, and at this commit: its files' churn counts from here.
        if accepting || confirming {
            d.fresh.confirmed = now;
            if let Some(head) = project::head(root) {
                d.provenance.commit = Some(head);
            }
        }
        d.updated = now;
        self.write(root, &d, Some(path))?;
        // The one it replaces may have been retired meanwhile; then there's
        // nothing left to supersede.
        if accepting
            && let Some(old) = d.supersedes
            && files.get(&old).is_some_and(|(_, o)| o.status == Active)
        {
            self.supersede(root, old, id, now)?;
        }
        Ok(d)
    }

    fn supersede(&self, root: &Path, old: u32, by: u32, now: u64) -> Result<()> {
        let files = self.files(root);
        let Some((path, d)) = files.get(&old) else {
            return Ok(());
        };
        let mut d = d.clone();
        d.status = DecisionStatus::Superseded;
        d.superseded_by = Some(by);
        d.updated = now;
        self.write(root, &d, Some(path))
    }

    /// One past the highest number on any decision file, parsed or not, so a file
    /// broken by a hand edit never has its id given out again.
    fn next_id(&self, root: &Path) -> u32 {
        let Ok(entries) = std::fs::read_dir(self.dir(root)) else {
            return 1;
        };
        entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
                digits.parse::<u32>().ok()
            })
            .max()
            .map_or(1, |id| id + 1)
    }

    /// Writes atomically, then removes the old file if the title (so the name)
    /// changed.
    fn write(&self, root: &Path, d: &Decision, old: Option<&PathBuf>) -> Result<()> {
        let dir = self.dir(root);
        valkyrie_proto::ensure_private_dir(&dir)?;
        let path = dir.join(file::name(d));
        write_atomic(&path, &file::render(d))?;
        if let Some(old) = old
            && *old != path
        {
            let _ = std::fs::remove_file(old);
        }
        Ok(())
    }

    /// Notes which root a project directory is for, so `projects` can list it.
    fn mark_project(&self, root: &Path) -> Result<()> {
        let dir = self.base.join(project::key(root));
        valkyrie_proto::ensure_private_dir(&self.base)?;
        valkyrie_proto::ensure_private_dir(&dir)?;
        let marker = dir.join("root");
        if !marker.exists() {
            write_atomic(&marker, &format!("{}\n", root.display()))?;
        }
        Ok(())
    }

    /// Copies `root`'s active decisions into `<checkout>/.valkyrie/decisions/` (the
    /// checkout being exported from, which may be a worktree of `root`), replacing
    /// what an earlier export of these decisions put there. Files that aren't
    /// this store's (a teammate's, say) are left alone. Returns the files written.
    pub fn export(&self, root: &Path, checkout: &Path) -> Result<Vec<PathBuf>> {
        let all = self.load(root);
        if all.is_empty() {
            bail!("no decisions for {} to export", root.display());
        }
        let into = checkout.join(".valkyrie/decisions");
        std::fs::create_dir_all(&into).with_context(|| format!("create {}", into.display()))?;
        for entry in std::fs::read_dir(&into)?.flatten() {
            let path = entry.path();
            let ours = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| file::parse(&t, root.to_path_buf()))
                .is_some_and(|d| all.iter().any(|s| s.id == d.id && s.created == d.created));
            if ours {
                std::fs::remove_file(&path)?;
            }
        }
        let mut written = Vec::new();
        for d in all {
            if d.status != DecisionStatus::Active {
                continue;
            }
            let path = into.join(file::name(&d));
            write_atomic(&path, &file::render(&d))?;
            written.push(path);
        }
        Ok(written)
    }
}

/// A title on one line and a trimmed body, both without control characters (but
/// the body's line breaks) or bidi overrides: text a reviewer reads in a terminal
/// must be the text agents get.
fn clean(title: &str, body: &str) -> Result<(String, String)> {
    let shown = |c: char, keep: &[char]| {
        keep.contains(&c)
            || !(c.is_control()
                || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
    };
    let title: String = title
        .chars()
        .map(|c| if shown(c, &[]) { c } else { ' ' })
        .collect();
    let title = file::one_line(&title);
    if title.is_empty() {
        bail!("a decision needs a title");
    }
    if title.chars().count() > MAX_TITLE {
        bail!("title is over {MAX_TITLE} characters; put the detail in the body");
    }
    let body: String = body.chars().filter(|&c| shown(c, &['\n', '\t'])).collect();
    let body = body.trim().to_owned();
    if body.chars().count() > MAX_BODY {
        bail!("body is over {MAX_BODY} characters; a decision is a few sentences");
    }
    Ok((title, body))
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, text).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::DecisionKind;

    struct Fixture {
        base: PathBuf,
        store: Store,
        repo: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let base =
                std::env::temp_dir().join(format!("valk-store-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            let repo = base.join("repo");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            let repo = std::fs::canonicalize(repo).unwrap();
            let store = Store::new(base.join("state/context"));
            Self { base, store, repo }
        }

        fn new_decision(&self, title: &str) -> NewDecision {
            NewDecision {
                cwd: self.repo.clone(),
                title: title.into(),
                body: "Because.".into(),
                kind: DecisionKind::Decision,
                propose: false,
                supersedes: None,
                commit: None,
                review_every: None,
                session: None,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn by(who: &str) -> Provenance {
        Provenance {
            by: who.into(),
            ..Provenance::default()
        }
    }

    #[test]
    fn humans_decide_agents_propose_and_ids_count_up() {
        let f = Fixture::new("who");
        let a = f
            .store
            .decide(&f.new_decision("A"), by("human"), 1)
            .unwrap();
        let b = f
            .store
            .decide(&f.new_decision("B"), by("claude"), 2)
            .unwrap();
        let mut asked = f.new_decision("C");
        asked.propose = true;
        let c = f.store.decide(&asked, by("human"), 3).unwrap();
        assert_eq!((a.id, a.status), (1, DecisionStatus::Active));
        assert_eq!((b.id, b.status), (2, DecisionStatus::Proposed));
        assert_eq!((c.id, c.status), (3, DecisionStatus::Proposed));
        assert_eq!(f.store.projects(), vec![f.repo.clone()]);
        let proposed: Vec<u32> = f.store.proposals().iter().map(|d| d.id).collect();
        assert_eq!(proposed, vec![2, 3]);
    }

    #[test]
    fn accepting_a_replacement_supersedes_the_old_decision() {
        let f = Fixture::new("supersede");
        f.store
            .decide(&f.new_decision("Old"), by("human"), 1)
            .unwrap();
        let mut new = f.new_decision("New");
        new.supersedes = Some(1);
        let proposal = f.store.decide(&new, by("codex"), 2).unwrap();
        // Still only proposed: the old one holds until a human accepts.
        assert_eq!(f.store.load(&f.repo)[0].status, DecisionStatus::Active);
        f.store
            .review(&f.repo, proposal.id, &ReviewAction::Accept, None, 3)
            .unwrap();
        let all = f.store.load(&f.repo);
        assert_eq!(all[0].status, DecisionStatus::Superseded);
        assert_eq!(all[0].superseded_by, Some(2));
        assert_eq!(all[1].status, DecisionStatus::Active);
    }

    #[test]
    fn review_follows_the_lifecycle() {
        let f = Fixture::new("life");
        f.store
            .decide(&f.new_decision("P"), by("claude"), 1)
            .unwrap();
        let r = |id, a: ReviewAction| f.store.review(&f.repo, id, &a, None, 2);
        assert!(r(1, ReviewAction::Retire).is_err());
        let edited = r(
            1,
            ReviewAction::Edit {
                title: "Renamed".into(),
                body: "New body".into(),
                kind: DecisionKind::Gotcha,
            },
        )
        .unwrap();
        assert_eq!(edited.status, DecisionStatus::Proposed);
        // The rename moved the file; there's still exactly one.
        assert_eq!(f.store.load(&f.repo).len(), 1);
        assert_eq!(
            r(1, ReviewAction::Reject).unwrap().status,
            DecisionStatus::Rejected
        );
        assert!(r(1, ReviewAction::Accept).is_err());
        assert!(r(9, ReviewAction::Accept).is_err());
    }

    #[test]
    fn bad_input_is_refused() {
        let f = Fixture::new("bad");
        assert!(
            f.store
                .decide(&f.new_decision("  "), by("human"), 1)
                .is_err()
        );
        let mut long = f.new_decision("x");
        long.body = "y".repeat(MAX_BODY + 1);
        assert!(f.store.decide(&long, by("human"), 1).is_err());
        let mut dangling = f.new_decision("x");
        dangling.supersedes = Some(5);
        assert!(f.store.decide(&dangling, by("human"), 1).is_err());
    }

    #[test]
    fn hidden_text_is_stripped() {
        let f = Fixture::new("hidden");
        let mut sneaky = f.new_decision("Use \u{1b}[8mrm -rf\u{1b}[0m tabs\u{202e}");
        sneaky.body = "line one\n\u{1b}[2K\rline two\u{2066}".into();
        let d = f.store.decide(&sneaky, by("claude"), 1).unwrap();
        assert_eq!(d.title, "Use [8mrm -rf [0m tabs");
        assert_eq!(d.body, "line one\n[2Kline two");
    }

    #[test]
    fn a_broken_file_keeps_its_id() {
        let f = Fixture::new("broken");
        f.store
            .decide(&f.new_decision("A"), by("human"), 1)
            .unwrap();
        let dir = f.store.dir(&f.repo);
        std::fs::write(
            dir.join("0002-hand-edited.md"),
            "---\nid: 2\nstatus: oops\n---\n# B\n",
        )
        .unwrap();
        let c = f
            .store
            .decide(&f.new_decision("C"), by("human"), 2)
            .unwrap();
        assert_eq!(c.id, 3);
    }

    #[test]
    fn revise_rewords_and_accepts_a_proposal_only() {
        let f = Fixture::new("revise");
        f.store
            .decide(&f.new_decision("Draft"), by("claude"), 1)
            .unwrap();
        let revise = ReviewAction::Revise {
            title: "Final".into(),
            body: "Better".into(),
            kind: DecisionKind::Constraint,
        };
        let d = f.store.review(&f.repo, 1, &revise, Some(1), 2).unwrap();
        assert_eq!(
            (d.title.as_str(), d.status),
            ("Final", DecisionStatus::Active)
        );
        // Read before it changed: refused.
        assert!(
            f.store
                .review(&f.repo, 1, &ReviewAction::Retire, Some(1), 3)
                .is_err()
        );
        // Already active (say, accepted elsewhere first): nothing is rewritten.
        assert!(f.store.review(&f.repo, 1, &revise, None, 3).is_err());
    }

    #[test]
    fn worktrees_share_their_repos_decisions() {
        let f = Fixture::new("share");
        let private = f.repo.join(".git/worktrees/wt");
        std::fs::create_dir_all(&private).unwrap();
        std::fs::write(private.join("commondir"), "../..").unwrap();
        let wt = f.base.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}", private.display())).unwrap();
        let mut from_wt = f.new_decision("Shared");
        from_wt.cwd = wt;
        let d = f.store.decide(&from_wt, by("human"), 1).unwrap();
        assert_eq!(d.project, f.repo);
        assert_eq!(f.store.load(&f.repo).len(), 1);
    }

    #[test]
    fn export_writes_only_active_decisions_and_replaces_its_own_files() {
        let f = Fixture::new("export");
        f.store
            .decide(&f.new_decision("Keep"), by("human"), 1)
            .unwrap();
        f.store
            .decide(&f.new_decision("Pending"), by("claude"), 2)
            .unwrap();
        let out = f.repo.join(".valkyrie/decisions");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("README.md"), "not a decision").unwrap();
        // A teammate's export: parses, but isn't one of ours.
        let theirs = "---\nid: 9\nstatus: active\ncreated: 5\n---\n# Theirs\n";
        std::fs::write(out.join("0009-theirs.md"), theirs).unwrap();
        std::fs::write(
            out.join("0001-stale-name.md"),
            file::render(&f.store.load(&f.repo)[0]),
        )
        .unwrap();
        let written = f.store.export(&f.repo, &f.repo).unwrap();
        assert_eq!(written, vec![out.join("0001-keep.md")]);
        let mut names: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, vec!["0001-keep.md", "0009-theirs.md", "README.md"]);
        let empty = Fixture::new("export-empty");
        assert!(empty.store.export(&empty.repo, &empty.repo).is_err());
    }
}

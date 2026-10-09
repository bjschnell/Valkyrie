//! The project context layer (DESIGN §6, ADR-0007): decisions kept per project as
//! markdown files in the state dir, their review lifecycle, and the block injected
//! into agents.

mod file;
pub mod inject;
pub mod project;

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
        let title = file::one_line(&new.title);
        if title.is_empty() {
            bail!("a decision needs a title");
        }
        if title.chars().count() > MAX_TITLE {
            bail!("title is over {MAX_TITLE} characters; put the detail in the body");
        }
        let body = new.body.trim().to_owned();
        if body.chars().count() > MAX_BODY {
            bail!("body is over {MAX_BODY} characters; a decision is a few sentences");
        }
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
        let decision = Decision {
            id: files.keys().next_back().map_or(1, |id| id + 1),
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
            provenance,
        };
        self.mark_project(&root)?;
        if decision.status == DecisionStatus::Active
            && let Some(old) = decision.supersedes
        {
            self.supersede(&root, old, decision.id, now)?;
        }
        self.write(&root, &decision, None)?;
        Ok(decision)
    }

    pub fn review(
        &self,
        root: &Path,
        id: u32,
        action: &ReviewAction,
        now: u64,
    ) -> Result<Decision> {
        use DecisionStatus::*;
        let files = self.files(root);
        let Some((path, d)) = files.get(&id) else {
            bail!("no decision #{id} in {}", root.display());
        };
        let mut d = d.clone();
        match (action, d.status) {
            (ReviewAction::Accept, Proposed) => {
                if let Some(old) = d.supersedes {
                    // The one it replaces may have been retired meanwhile; then
                    // there's nothing left to supersede.
                    if files.get(&old).is_some_and(|(_, o)| o.status == Active) {
                        self.supersede(root, old, id, now)?;
                    }
                }
                d.status = Active;
            }
            (ReviewAction::Reject, Proposed) => d.status = Rejected,
            (ReviewAction::Retire, Active) => d.status = Retired,
            (ReviewAction::Edit { title, body, kind }, Proposed | Active) => {
                let title = file::one_line(title);
                if title.is_empty() || title.chars().count() > MAX_TITLE {
                    bail!("a title is 1 to {MAX_TITLE} characters");
                }
                if body.trim().chars().count() > MAX_BODY {
                    bail!("body is over {MAX_BODY} characters");
                }
                d.title = title;
                d.body = body.trim().to_owned();
                d.kind = *kind;
            }
            (action, status) => bail!(
                "can't {} #{id}: it is {}",
                match action {
                    ReviewAction::Accept => "accept",
                    ReviewAction::Reject => "reject",
                    ReviewAction::Retire => "retire",
                    ReviewAction::Edit { .. } => "edit",
                },
                status.as_str()
            ),
        }
        d.updated = now;
        self.write(root, &d, Some(path))?;
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

    /// Copies the active decisions into `<root>/.valkyrie/decisions/`, replacing
    /// what an earlier export put there. Returns the files written.
    pub fn export(&self, root: &Path) -> Result<Vec<PathBuf>> {
        let into = root.join(".valkyrie/decisions");
        std::fs::create_dir_all(&into).with_context(|| format!("create {}", into.display()))?;
        for entry in std::fs::read_dir(&into)?.flatten() {
            let path = entry.path();
            let ours = std::fs::read_to_string(&path)
                .ok()
                .and_then(|t| file::parse(&t, root.to_path_buf()))
                .is_some();
            if ours {
                std::fs::remove_file(&path)?;
            }
        }
        let mut written = Vec::new();
        for d in self.load(root) {
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
            .review(&f.repo, proposal.id, &ReviewAction::Accept, 3)
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
        let r = |id, a: ReviewAction| f.store.review(&f.repo, id, &a, 2);
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
        std::fs::write(
            out.join("0001-stale-name.md"),
            file::render(&f.store.load(&f.repo)[0]),
        )
        .unwrap();
        let written = f.store.export(&f.repo).unwrap();
        assert_eq!(written, vec![out.join("0001-keep.md")]);
        let mut names: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, vec!["0001-keep.md", "README.md"]);
    }
}

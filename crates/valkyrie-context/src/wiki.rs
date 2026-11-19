//! The repository wiki (backlog #210): pages agents write for agents about how a
//! repository is built, so the next agent can design against it without
//! re-exploring the code. Kept in `<state dir>/context/<project>/wiki/<slug>.md`,
//! never in the repository. Each page records the commit it was written at and the
//! paths it covers, so a page whose code moved on reads as stale. Sessions get a
//! short index at start (`index`) and read pages on demand.

use crate::{Store, project, shown, stale, write_atomic};
use anyhow::{Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use valkyrie_proto::NoWindow;

/// Longest page body kept, in characters: a page is one area, not the codebase.
pub const MAX_PAGE: usize = 16_000;
/// Most pages a repository keeps.
pub const MAX_PAGES: usize = 40;
/// Most paths one page covers.
pub const MAX_SCOPE: usize = 12;
/// The index sessions get at start, in characters.
pub const INDEX_BUDGET: usize = 1500;
/// Lines changed under a page's paths since it was written that make it stale.
pub const STALE_LINES: u64 = 300;

/// One wiki page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// Its name in commands and its file name: `[a-z0-9-]`, at most 48.
    pub slug: String,
    pub title: String,
    /// One line: what the page answers, for the index.
    pub summary: String,
    /// Files and directories it describes, relative to the repository root.
    pub scope: Vec<String>,
    /// `HEAD` where it was written, for staleness.
    pub commit: Option<String>,
    /// Unix seconds.
    pub updated: u64,
    pub body: String,
}

/// What a writer gives; the commit and time are found, not claimed.
#[derive(Debug, Clone, Default)]
pub struct NewPage {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub scope: Vec<String>,
    pub body: String,
}

/// Whether `slug` is a page name: lowercase letters, digits and dashes. It becomes
/// a file name, so nothing that could leave the wiki folder.
pub fn valid_slug(slug: &str) -> bool {
    (1..=48).contains(&slug.len())
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
}

fn one_line(s: &str) -> String {
    shown(s, false)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn render(p: &Page) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("title: {}\n", one_line(&p.title)));
    out.push_str(&format!("summary: {}\n", one_line(&p.summary)));
    if !p.scope.is_empty() {
        out.push_str(&format!("scope: {}\n", p.scope.join(", ")));
    }
    if let Some(c) = &p.commit {
        out.push_str(&format!("commit: {c}\n"));
    }
    out.push_str(&format!("updated: {}\n", p.updated));
    out.push_str("---\n");
    out.push_str(p.body.trim());
    out.push('\n');
    out
}

/// Reads a page file back; `slug` is its file name.
pub fn parse(slug: &str, text: &str) -> Option<Page> {
    let rest = text.strip_prefix("---\n")?;
    let (front, body) = rest.split_once("\n---\n")?;
    let mut p = Page {
        slug: slug.to_owned(),
        title: String::new(),
        summary: String::new(),
        scope: Vec::new(),
        commit: None,
        updated: 0,
        body: body.trim().to_owned(),
    };
    for line in front.lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "title" => p.title = v.to_owned(),
            "summary" => p.summary = v.to_owned(),
            "scope" => {
                p.scope = v
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect()
            }
            "commit" => p.commit = Some(v.to_owned()).filter(|c| stale::valid_commit(c)),
            "updated" => p.updated = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    (!p.title.is_empty()).then_some(p)
}

/// `path` as a scope entry: relative to `root`, inside it, and existing. Given
/// relative to `root` (or absolute inside it), with `/` separators.
fn scope_entry(root: &Path, path: &str) -> Result<String> {
    let trimmed = path.trim().trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "." {
        bail!("scope {path:?}: name a file or directory, not the whole repository");
    }
    let rel = match Path::new(trimmed).strip_prefix(root) {
        Ok(rel) => rel.to_path_buf(),
        Err(_) if Path::new(trimmed).is_absolute() => {
            bail!("scope {path:?} is outside {}", root.display())
        }
        Err(_) => PathBuf::from(trimmed),
    };
    if rel
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        bail!("scope {path:?}: a path inside the repository, without `..`");
    }
    if !root.join(&rel).exists() {
        bail!("scope {path:?} doesn't exist in {}", root.display());
    }
    let rel = rel.display().to_string();
    #[cfg(windows)]
    let rel = rel.replace('\\', "/");
    if rel.contains(',') {
        bail!("scope {path:?}: a path with a comma can't be stored");
    }
    Ok(rel)
}

/// Why `p` may no longer describe the code, given the line changes under each
/// path since its commit (`numstat`, from [`changes`]).
fn why_stale(root: &Path, p: &Page, numstat: Option<&[(String, u64)]>) -> Option<String> {
    if let Some(gone) = p.scope.iter().find(|s| !root.join(s).exists()) {
        return Some(format!("{gone} no longer exists"));
    }
    let lines: u64 = numstat?
        .iter()
        .filter(|(file, _)| {
            p.scope
                .iter()
                .any(|s| file == s || file.starts_with(&format!("{s}/")))
        })
        .map(|(_, n)| n)
        .sum();
    (lines >= STALE_LINES)
        .then(|| format!("{lines} lines changed in its files since it was written"))
}

/// Lines added plus removed per file between where `commit` and `HEAD` forked and
/// `HEAD` (as [`stale::changed`] measures), or `None` when git can't say.
fn changes(root: &Path, commit: &str) -> Option<Vec<(String, u64)>> {
    if !stale::valid_commit(commit) {
        return None;
    }
    let out = Command::new("git")
        .no_window()
        .args([
            "diff",
            "--numstat",
            "--no-renames",
            "--end-of-options",
            &format!("{commit}...HEAD"),
        ])
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let mut cols = line.splitn(3, '\t');
                let added: u64 = cols.next()?.parse().unwrap_or(0);
                let removed: u64 = cols.next()?.parse().unwrap_or(0);
                Some((cols.next()?.to_owned(), added + removed))
            })
            .collect(),
    )
}

/// Each page's staleness, by slug: one `git diff` per distinct commit, since an
/// exploration writes its pages at one commit.
pub fn staleness(root: &Path, pages: &[Page]) -> BTreeMap<String, String> {
    let mut by_commit: BTreeMap<&str, Option<Vec<(String, u64)>>> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for p in pages {
        let numstat = p.commit.as_deref().and_then(|c| {
            by_commit
                .entry(c)
                .or_insert_with(|| changes(root, c))
                .as_deref()
        });
        if let Some(why) = why_stale(root, p, numstat) {
            out.insert(p.slug.clone(), why);
        }
    }
    out
}

/// The wiki's index for a session's start: one line a page, stale ones marked,
/// cut to [`INDEX_BUDGET`]. Empty when there are no pages.
pub fn index(pages: &[Page], stale: &BTreeMap<String, String>, valk: &str) -> String {
    if pages.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "This repository has a wiki agents wrote for agents: how the code is laid out and how \
         to build against it. Before exploring the code for a task, read the pages that bear \
         on it with `{valk} wiki show <page>` (MCP `get_wiki_page`). It lags the code at \
         times: check what it says against the files, and when a page is wrong or stale, \
         rewrite it with `{valk} wiki write`.\n"
    );
    let mut left = 0;
    for p in pages {
        let mark = if stale.contains_key(&p.slug) {
            " (stale)"
        } else {
            ""
        };
        let line = format!("- {}{mark}: {}. {}\n", p.slug, p.title, p.summary);
        // Room kept for the line saying how many were left out.
        if left > 0 || out.len() + line.len() + 40 > INDEX_BUDGET {
            left += 1;
            continue;
        }
        out.push_str(&line);
    }
    if left > 0 {
        out.push_str(&format!("({left} more: `{valk} wiki list`)\n"));
    }
    out
}

/// What an agent started by `valk wiki explore` is told. With `stale` pages,
/// only those are rewritten; otherwise the whole repository is mapped, building
/// on whatever pages are there.
pub fn brief(pages: &[Page], stale: &[&Page], valk: &str) -> String {
    let write = format!(
        "Write each page with `{valk} wiki write <page> --title \"<title>\" --summary \"<one \
         line: what it answers>\" --scope <path> [--scope <path>...] -`, the markdown on \
         stdin. `<page>` is lowercase letters, digits and dashes. `--scope` names the \
         directories and files the page describes; when they change a lot, the page is \
         flagged stale. A page is at most {MAX_PAGE} characters, and a repository keeps at \
         most {MAX_PAGES} pages."
    );
    let what = "A page is for an agent about to change this code, not for a newcomer or the \
         user: what the area is for and where its boundaries are, the main types and the \
         flow of data and control through them, the invariants and conventions the code \
         relies on, how it's tested, and how to add the usual kind of change (a new \
         command, endpoint, message, setting) with the files that change. Name real files, \
         types and functions; say what the code shows, and mark anything you inferred \
         rather than read. Leave out what any agent sees in a minute: file listings, \
         dependency lists, code it can read for itself.";
    if !stale.is_empty() {
        let list: Vec<String> = stale
            .iter()
            .map(|p| format!("- {} ({})", p.slug, p.scope.join(", ")))
            .collect();
        return format!(
            "Bring this repository's agent wiki up to date. These pages are stale: the code \
             they describe changed since they were written.\n{}\n\nFor each, read it with \
             `{valk} wiki show <page>`, read the code it covers as it is now, and rewrite \
             it where it's wrong or missing something; keep what still holds. If an area \
             was split, renamed or removed, rewrite, add or `{valk} wiki rm` pages to \
             match.\n\n{what}\n\n{write}\n\nDon't change the repository's files. When \
             you're done, say in a few lines which pages changed and why.\n",
            list.join("\n")
        );
    }
    let have = if pages.is_empty() {
        String::new()
    } else {
        let list: Vec<String> = pages
            .iter()
            .map(|p| format!("- {}: {}", p.slug, p.title))
            .collect();
        format!(
            "\n\nIt already has these pages; read them first, keep what still holds, and \
             fill the gaps:\n{}",
            list.join("\n")
        )
    };
    format!(
        "Explore this repository and write its agent wiki: pages that let a future agent \
         design changes against the system without exploring it from scratch. Every agent \
         Valkyrie starts here gets the wiki's index at session start and reads the pages a \
         task needs.{have}\n\nStart from the README, the design docs and any architecture \
         decision records, then the code. Write an `overview` page first: what the system \
         is, its parts and how they talk to each other, and which page covers each part. \
         Then one page per part (a crate, package, service or layer), and a page for each \
         cross-cutting concern that has rules of its own (a protocol, storage, \
         configuration, errors, testing). Aim for 5 to 15 pages.\n\n{what}\n\n{write}\n\n\
         Don't change the repository's files. When you're done, list the pages you wrote.\n"
    )
}

impl Store {
    fn wiki_dir(&self, root: &Path) -> PathBuf {
        self.base.join(project::key(root)).join("wiki")
    }

    /// A project's wiki pages, by slug. Files that don't parse are skipped.
    pub fn wiki(&self, root: &Path) -> Vec<Page> {
        let Ok(entries) = std::fs::read_dir(self.wiki_dir(root)) else {
            return Vec::new();
        };
        let mut out: Vec<Page> = entries
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let slug = path
                    .file_name()?
                    .to_str()?
                    .strip_suffix(".md")
                    .filter(|s| valid_slug(s))?
                    .to_owned();
                parse(&slug, &std::fs::read_to_string(&path).ok()?)
            })
            .collect();
        // The overview first: it's the map to the rest.
        out.sort_by(|a, b| (a.slug != "overview", &a.slug).cmp(&(b.slug != "overview", &b.slug)));
        out
    }

    /// One page.
    pub fn wiki_page(&self, root: &Path, slug: &str) -> Option<Page> {
        if !valid_slug(slug) {
            return None;
        }
        let text = std::fs::read_to_string(self.wiki_dir(root).join(format!("{slug}.md"))).ok()?;
        parse(slug, &text)
    }

    /// Writes a page, replacing one with the same slug. `commit` is `root`'s
    /// `HEAD`, as the caller found it.
    pub fn write_wiki(
        &self,
        root: &Path,
        new: &NewPage,
        commit: Option<String>,
        now: u64,
    ) -> Result<Page> {
        if !valid_slug(&new.slug) {
            bail!(
                "page name {:?}: lowercase letters, digits and dashes, at most 48",
                new.slug
            );
        }
        let title = one_line(&new.title);
        if title.is_empty() {
            bail!("a page needs a title");
        }
        let body = shown(&new.body, true).trim().to_owned();
        if body.is_empty() {
            bail!("a page needs some text");
        }
        let len = body.chars().count();
        if len > MAX_PAGE {
            bail!(
                "the page is {len} characters, over {MAX_PAGE}; split it into pages, one per area"
            );
        }
        if new.scope.len() > MAX_SCOPE {
            bail!(
                "{} scope paths, over {MAX_SCOPE}; name directories rather than files",
                new.scope.len()
            );
        }
        let mut scope = Vec::new();
        for s in &new.scope {
            let s = scope_entry(root, s)?;
            if !scope.contains(&s) {
                scope.push(s);
            }
        }
        let have = self.wiki(root);
        if have.len() >= MAX_PAGES && !have.iter().any(|p| p.slug == new.slug) {
            bail!("the wiki has {MAX_PAGES} pages, its most; fold this into one of them");
        }
        let page = Page {
            slug: new.slug.clone(),
            title,
            summary: one_line(&new.summary),
            scope,
            commit: commit.filter(|c| stale::valid_commit(c)),
            updated: now,
            body,
        };
        self.mark_project(root)?;
        let dir = self.wiki_dir(root);
        valkyrie_proto::ensure_private_dir(&dir)?;
        write_atomic(&dir.join(format!("{}.md", page.slug)), &render(&page))?;
        Ok(page)
    }

    /// Deletes a page.
    pub fn rm_wiki(&self, root: &Path, slug: &str) -> Result<()> {
        if self.wiki_page(root, slug).is_none() {
            bail!("no wiki page {slug:?} in {}", root.display());
        }
        std::fs::remove_file(self.wiki_dir(root).join(format!("{slug}.md")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    fn store(name: &str) -> (Tmp, Store, PathBuf) {
        let dir =
            Tmp(std::env::temp_dir().join(format!("valk-wiki-{name}-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&dir.0);
        let repo = dir.0.join("repo");
        std::fs::create_dir_all(repo.join("src/daemon")).unwrap();
        std::fs::write(repo.join("src/daemon/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(repo.join("README.md"), "hi\n").unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "init"]);
        let store = Store::new(dir.0.join("context"));
        let repo = project::root(&repo);
        (dir, store, repo)
    }

    fn page(slug: &str, scope: &[&str]) -> NewPage {
        NewPage {
            slug: slug.into(),
            title: format!("The {slug}"),
            summary: "What it does".into(),
            scope: scope.iter().map(|s| (*s).to_owned()).collect(),
            body: "# Daemon\n\nIt runs.".into(),
        }
    }

    #[test]
    fn a_page_reads_back_with_its_commit_and_scope() {
        let (_d, store, repo) = store("roundtrip");
        let head = git(&repo, &["rev-parse", "HEAD"]);
        let p = store
            .write_wiki(
                &repo,
                &page("daemon", &["src/daemon/"]),
                Some(head.clone()),
                7,
            )
            .unwrap();
        assert_eq!(p.scope, vec!["src/daemon"]);
        let back = store.wiki_page(&repo, "daemon").unwrap();
        assert_eq!(back, p);
        assert_eq!(back.commit.as_deref(), Some(head.as_str()));
        // Rewriting replaces it.
        let mut again = page("daemon", &["src/daemon"]);
        again.body = "Changed.".into();
        store.write_wiki(&repo, &again, None, 8).unwrap();
        assert_eq!(store.wiki(&repo).len(), 1);
        assert_eq!(store.wiki_page(&repo, "daemon").unwrap().body, "Changed.");
        store.rm_wiki(&repo, "daemon").unwrap();
        assert!(store.wiki(&repo).is_empty());
    }

    #[test]
    fn names_and_paths_stay_inside() {
        let (_d, store, repo) = store("inside");
        for bad in ["../x", "a/b", "", "-x", "Big", "a.md"] {
            assert!(!valid_slug(bad), "{bad}");
            assert!(store.write_wiki(&repo, &page(bad, &[]), None, 1).is_err());
        }
        for bad in ["../etc", "/etc", ".", "nope/missing"] {
            assert!(
                store
                    .write_wiki(&repo, &page("x", &[bad]), None, 1)
                    .is_err(),
                "{bad}"
            );
        }
        let abs = repo.join("src").display().to_string();
        let p = store
            .write_wiki(&repo, &page("x", &[&abs]), None, 1)
            .unwrap();
        assert_eq!(p.scope, vec!["src"]);
        assert!(store.wiki_page(&repo, "../x").is_none());
    }

    #[test]
    fn overview_leads_and_the_limits_hold() {
        let (_d, store, repo) = store("limits");
        store.write_wiki(&repo, &page("a", &[]), None, 1).unwrap();
        store
            .write_wiki(&repo, &page("overview", &[]), None, 1)
            .unwrap();
        let slugs: Vec<_> = store.wiki(&repo).into_iter().map(|p| p.slug).collect();
        assert_eq!(slugs, ["overview", "a"]);
        let mut big = page("big", &[]);
        big.body = "x".repeat(MAX_PAGE + 1);
        assert!(store.write_wiki(&repo, &big, None, 1).is_err());
        for n in 2..MAX_PAGES {
            store
                .write_wiki(&repo, &page(&format!("p{n}"), &[]), None, 1)
                .unwrap();
        }
        assert!(
            store
                .write_wiki(&repo, &page("one-more", &[]), None, 1)
                .is_err()
        );
        // Rewriting one at the limit still works.
        store.write_wiki(&repo, &page("a", &[]), None, 2).unwrap();
    }

    #[test]
    fn a_page_goes_stale_when_its_code_changes_or_goes() {
        let (_d, store, repo) = store("stale");
        let head = git(&repo, &["rev-parse", "HEAD"]);
        store
            .write_wiki(
                &repo,
                &page("daemon", &["src/daemon"]),
                Some(head.clone()),
                1,
            )
            .unwrap();
        store
            .write_wiki(&repo, &page("readme", &["README.md"]), Some(head), 1)
            .unwrap();
        assert!(staleness(&repo, &store.wiki(&repo)).is_empty());
        let lots: String = (0..STALE_LINES).map(|n| format!("// {n}\n")).collect();
        std::fs::write(repo.join("src/daemon/main.rs"), lots).unwrap();
        git(&repo, &["commit", "-qam", "churn"]);
        let s = staleness(&repo, &store.wiki(&repo));
        assert!(s["daemon"].contains("lines changed"), "{s:?}");
        assert!(!s.contains_key("readme"));
        std::fs::remove_file(repo.join("README.md")).unwrap();
        let s = staleness(&repo, &store.wiki(&repo));
        assert!(s["readme"].contains("no longer exists"), "{s:?}");
    }

    #[test]
    fn the_index_marks_stale_pages_and_holds_its_budget() {
        let pages: Vec<Page> = (0..60)
            .map(|n| Page {
                slug: format!("p{n}"),
                title: "A page about a part of the system".into(),
                summary: "What that part does and how to change it".into(),
                scope: vec![],
                commit: None,
                updated: 0,
                body: String::new(),
            })
            .collect();
        assert!(index(&[], &BTreeMap::new(), "valk").is_empty());
        let stale = BTreeMap::from([("p0".to_owned(), "x".to_owned())]);
        let text = index(&pages, &stale, "valk");
        assert!(text.len() <= INDEX_BUDGET, "{}", text.len());
        assert!(text.contains("- p0 (stale): "));
        assert!(text.contains("more: `valk wiki list`"));
    }

    #[test]
    fn the_brief_maps_everything_or_only_whats_stale() {
        let p = Page {
            slug: "daemon".into(),
            title: "Daemon".into(),
            summary: String::new(),
            scope: vec!["src/daemon".into()],
            commit: None,
            updated: 0,
            body: String::new(),
        };
        let all = brief(std::slice::from_ref(&p), &[], "valk");
        assert!(all.contains("`overview` page") && all.contains("- daemon: Daemon"));
        let some = brief(std::slice::from_ref(&p), &[&p], "valk");
        assert!(some.contains("These pages are stale") && some.contains("- daemon (src/daemon)"));
        assert!(!some.contains("`overview` page"));
    }
}

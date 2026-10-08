//! Reviewing what an agent changed, from a phone (DESIGN §8.8): the session's
//! repo diffed against `HEAD`, staged and unstaged together, plus untracked files
//! as additions. Read-only: git runs with `GIT_OPTIONAL_LOCKS=0` and never touches
//! the index.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::io::Read;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

/// Patch text sent per file; a bigger one is cut, and says so.
const MAX_FILE_PATCH: usize = 200 * 1024;
/// Patch text sent in all.
const MAX_TOTAL: usize = 2 * 1024 * 1024;
/// Untracked files shown as additions.
const MAX_UNTRACKED: usize = 40;
/// An untracked file read whole up to this size.
const MAX_UNTRACKED_BYTES: u64 = 64 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Serialize, PartialEq)]
pub struct Review {
    /// The repo's top directory.
    pub root: String,
    pub branch: Option<String>,
    pub files: Vec<FileDiff>,
    /// Files left out (too many untracked, or over the size budget).
    pub omitted: usize,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct FileDiff {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub status: Change,
    pub added: usize,
    pub removed: usize,
    pub binary: bool,
    /// The hunks, from the first `@@`.
    pub patch: String,
    /// The patch was cut.
    pub cut: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Modified,
    Added,
    Deleted,
    Renamed,
    /// Not added to git yet.
    Untracked,
}

/// The changes in the repo holding `cwd`.
pub async fn review(cwd: &Path) -> Result<Review> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"])
        .await
        .context("not a git repository")?;
    let root = root.trim().to_owned();
    let dir = Path::new(&root);
    let branch = git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .await
        .ok()
        .map(|b| b.trim().to_owned());
    let diff_args = [
        "-c",
        "core.quotepath=off",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--find-renames",
        "--unified=3",
    ];
    // Everything since the last commit; a repo with no commits yet has no HEAD.
    let patch = match git(dir, &[&diff_args[..], &["HEAD", "--"]].concat()).await {
        Ok(patch) => patch,
        Err(_) => git(dir, &[&diff_args[..], &["--cached", "--"]].concat()).await?,
    };
    let mut files = parse(&patch);
    let untracked = git(dir, &["ls-files", "--others", "--exclude-standard", "-z"]).await?;
    let untracked: Vec<&str> = untracked.split('\0').filter(|p| !p.is_empty()).collect();
    let mut omitted = untracked.len().saturating_sub(MAX_UNTRACKED);
    for path in untracked.into_iter().take(MAX_UNTRACKED) {
        files.push(untracked_file(dir, path));
    }
    let mut total = 0;
    for file in &mut files {
        if total + file.patch.len() > MAX_TOTAL {
            file.patch.clear();
            file.cut = true;
            omitted += 1;
        }
        total += file.patch.len();
    }
    Ok(Review {
        root,
        branch,
        files,
        omitted,
    })
}

async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let run = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(GIT_TIMEOUT, run)
        .await
        .context("git took too long")??;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Splits `git diff` output into files.
pub fn parse(patch: &str) -> Vec<FileDiff> {
    let mut files = Vec::new();
    for chunk in patch.split("\ndiff --git ").enumerate().map(|(i, c)| {
        if i == 0 {
            c.strip_prefix("diff --git ").unwrap_or(c)
        } else {
            c
        }
    }) {
        if chunk.trim().is_empty() {
            continue;
        }
        files.push(file(chunk));
    }
    files
}

fn file(chunk: &str) -> FileDiff {
    let (head, body) = match chunk.find("\n@@") {
        Some(at) => (&chunk[..at], &chunk[at + 1..]),
        None => (chunk, ""),
    };
    let header = |prefix: &str| {
        head.lines()
            .find_map(|l| l.strip_prefix(prefix))
            .map(str::to_owned)
    };
    // `a/x b/x` on the first line; the `---`/`+++` lines say it better when present.
    let first = head.lines().next().unwrap_or("");
    let fallback = first
        .rsplit_once(" b/")
        .map(|(_, b)| b.to_owned())
        .unwrap_or_else(|| first.to_owned());
    let new = header("+++ b/");
    let old = header("--- a/");
    let renamed_from = header("rename from ");
    let status = if head.contains("\nnew file mode") {
        Change::Added
    } else if head.contains("\ndeleted file mode") {
        Change::Deleted
    } else if renamed_from.is_some() {
        Change::Renamed
    } else {
        Change::Modified
    };
    let path = match status {
        Change::Deleted => old.clone().unwrap_or(fallback),
        Change::Renamed => header("rename to ").unwrap_or(fallback),
        _ => new.unwrap_or(fallback),
    };
    let (added, removed) = count(body);
    let (patch, cut) = clip(body.trim_end_matches('\n'));
    FileDiff {
        path,
        from: renamed_from,
        status,
        added,
        removed,
        binary: head.contains("\nBinary files ") || head.starts_with("Binary files "),
        patch,
        cut,
    }
}

fn count(body: &str) -> (usize, usize) {
    body.lines().fold((0, 0), |(a, r), line| {
        if line.starts_with('+') && !line.starts_with("+++") {
            (a + 1, r)
        } else if line.starts_with('-') && !line.starts_with("---") {
            (a, r + 1)
        } else {
            (a, r)
        }
    })
}

fn clip(patch: &str) -> (String, bool) {
    if patch.len() <= MAX_FILE_PATCH {
        return (patch.to_owned(), false);
    }
    let mut end = MAX_FILE_PATCH;
    while !patch.is_char_boundary(end) {
        end -= 1;
    }
    // Whole lines only.
    let end = patch[..end].rfind('\n').unwrap_or(end);
    (patch[..end].to_owned(), true)
}

/// A file git doesn't track yet, shown as all added.
fn untracked_file(root: &Path, path: &str) -> FileDiff {
    let full = root.join(path);
    let mut out = FileDiff {
        path: path.to_owned(),
        from: None,
        status: Change::Untracked,
        added: 0,
        removed: 0,
        binary: false,
        patch: String::new(),
        cut: false,
    };
    let Ok(meta) = std::fs::metadata(&full) else {
        return out;
    };
    let mut bytes = Vec::new();
    let read = std::fs::File::open(&full)
        .and_then(|f| f.take(MAX_UNTRACKED_BYTES).read_to_end(&mut bytes));
    if read.is_err() {
        return out;
    }
    if bytes.contains(&0) {
        out.binary = true;
        return out;
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    out.added = lines.len();
    out.cut = meta.len() > MAX_UNTRACKED_BYTES;
    out.patch = format!(
        "@@ -0,0 +1,{} @@\n{}",
        lines.len(),
        lines
            .iter()
            .map(|l| format!("+{l}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/src/a.rs b/src/a.rs
index 1..2 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,3 @@
 fn a() {
-    1
+    2
 }
diff --git a/new.md b/new.md
new file mode 100644
index 0..3
--- /dev/null
+++ b/new.md
@@ -0,0 +1,2 @@
+# New
+text
diff --git a/old.txt b/old.txt
deleted file mode 100644
index 4..0
--- a/old.txt
+++ /dev/null
@@ -1 +0,0 @@
-gone
diff --git a/x.rs b/y.rs
similarity index 90%
rename from x.rs
rename to y.rs
diff --git a/logo.png b/logo.png
index 5..6 100644
Binary files a/logo.png and b/logo.png differ
";

    #[test]
    fn splits_a_diff_into_files() {
        let files = parse(PATCH);
        let summary: Vec<(&str, Change, usize, usize, bool)> = files
            .iter()
            .map(|f| (f.path.as_str(), f.status, f.added, f.removed, f.binary))
            .collect();
        assert_eq!(
            summary,
            [
                ("src/a.rs", Change::Modified, 1, 1, false),
                ("new.md", Change::Added, 2, 0, false),
                ("old.txt", Change::Deleted, 0, 1, false),
                ("y.rs", Change::Renamed, 0, 0, false),
                ("logo.png", Change::Modified, 0, 0, true),
            ]
        );
        assert_eq!(files[3].from.as_deref(), Some("x.rs"));
        assert!(files[0].patch.starts_with("@@ -1,3 +1,3 @@\n fn a() {"));
        assert!(parse("").is_empty());
    }

    #[tokio::test]
    async fn reviews_a_real_repo() {
        let dir = std::env::temp_dir().join(format!("valk-review-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "first"]);
        std::fs::write(dir.join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(dir.join("sub/new.txt"), "hello\n").unwrap();
        // From a subdirectory, as a session's cwd often is.
        let r = review(&dir.join("sub")).await.unwrap();
        assert_eq!(r.branch.as_deref(), Some("main"));
        let got: Vec<(&str, Change, usize, usize)> = r
            .files
            .iter()
            .map(|f| (f.path.as_str(), f.status, f.added, f.removed))
            .collect();
        assert_eq!(
            got,
            [
                ("a.txt", Change::Modified, 1, 1),
                ("sub/new.txt", Change::Untracked, 1, 0)
            ]
        );
        assert_eq!(r.files[1].patch, "@@ -0,0 +1,1 @@\n+hello");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

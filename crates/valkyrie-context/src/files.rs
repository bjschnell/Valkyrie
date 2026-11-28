//! A session's markdown and text files, read beside it (ADR-0021 item 9).
//!
//! The daemon reads a file in a session's repository for the web app or the TUI
//! only through [`check`], which refuses everything item 9 rules out. Every read,
//! every listed file and every chat mention that becomes a link goes through it.

use anyhow::{Context, Result, bail};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use valkyrie_proto::NoWindow;

/// The largest file read, in bytes.
pub const MAX_FILE: u64 = 200 * 1024;

/// The extensions read, compared without case.
const KINDS: &[&str] = &["md", "markdown", "txt"];

/// At most this many files are listed, or mentions checked, at once.
pub const MAX_LISTED: usize = 100;

/// A file that passed [`check`]: where it is, relative to the top level with `/`
/// between folders, and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub rel: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<u64>,
}

/// The canonical git top level of the checkout holding `cwd`, refused when it is
/// the home folder or `/`, as the docs root is.
pub fn top(cwd: &Path) -> Result<PathBuf> {
    top_in(cwd, &valkyrie_proto::home_dir())
}

fn top_in(cwd: &Path, home: &Path) -> Result<PathBuf> {
    let out = git(cwd, &["rev-parse", "--show-toplevel"])
        .with_context(|| format!("{} is not in a git repository", cwd.display()))?;
    let top = valkyrie_proto::canonical(Path::new(out.trim_end_matches(['\n', '\r'])))?;
    let home = valkyrie_proto::canonical(home).ok();
    if Some(&top) == home.as_ref() || top.parent().is_none() {
        bail!(
            "{} is too wide: files are read only from a project's repository",
            top.display()
        );
    }
    Ok(top)
}

/// `path` relative to `top`, if it is a plain one: no `..`, `.`, root or drive,
/// no backslash, and not empty. An absolute path under `top` is made relative.
fn relative(top: &Path, path: &str) -> Result<String> {
    let path = path.trim();
    let given = Path::new(path);
    let rel = if given.is_absolute() {
        given
            .strip_prefix(top)
            .map_err(|_| anyhow::anyhow!("{path} is outside the repository"))?
    } else {
        given
    };
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(p) => {
                let p = p.to_str().context("the path isn't UTF-8")?;
                if p.contains('\\') {
                    bail!("{path} is not a plain path in the repository");
                }
                parts.push(p);
            }
            _ => bail!("{path} is not a plain path in the repository"),
        }
    }
    if parts.is_empty() {
        bail!("no file named");
    }
    Ok(parts.join("/"))
}

/// Whether `rel` names a markdown or text file.
pub fn readable_kind(rel: &str) -> bool {
    Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| KINDS.iter().any(|k| e.eq_ignore_ascii_case(k)))
}

/// The one check (ADR-0021 item 9), on `path` in the repository at `top` (from
/// [`top`]): a `.md`, `.markdown` or `.txt` file, under `top` with no symlink on
/// the way, tracked by git or untracked and not ignored, a regular file of at
/// most [`MAX_FILE`] bytes.
pub fn check(top: &Path, path: &str) -> Result<Checked> {
    let rel = relative(top, path)?;
    if !readable_kind(&rel) {
        bail!("{rel}: only markdown and text files open here");
    }
    // A symlink anywhere along the path, the file included, is refused, so what
    // is read is what the path names inside the repository.
    let mut at = top.to_path_buf();
    let mut meta = None;
    for part in rel.split('/') {
        at.push(part);
        let m = std::fs::symlink_metadata(&at).with_context(|| format!("{rel}: no such file"))?;
        if m.file_type().is_symlink() {
            bail!("{rel}: a symlink is on the way, so it isn't opened");
        }
        meta = Some(m);
    }
    let meta = meta.context("no file named")?;
    if !meta.file_type().is_file() {
        bail!("{rel}: not a regular file");
    }
    if meta.len() > MAX_FILE {
        bail!("{rel}: over {} KB", MAX_FILE / 1024);
    }
    // Belt and braces after the walk: the canonical path is under the canonical
    // top level (decision #69).
    let real = valkyrie_proto::canonical(&at)?;
    if !real.starts_with(top) || real != at {
        bail!("{rel} is outside the repository");
    }
    if !git_sees(top, &rel)? {
        bail!("{rel}: git ignores it, so it isn't opened");
    }
    Ok(Checked {
        rel,
        size: meta.len(),
        modified: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs()),
        path: at,
    })
}

/// Whether git tracks `rel`, or would list it as untracked: not ignored.
fn git_sees(top: &Path, rel: &str) -> Result<bool> {
    let out = git(
        top,
        &[
            "--literal-pathspecs",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            rel,
        ],
    )?;
    Ok(out.split('\0').any(|p| p == rel))
}

/// `path`'s text, after [`check`]: UTF-8, without control characters or bidi
/// overrides, as a doc's body is cleaned.
pub fn read(top: &Path, path: &str) -> Result<(Checked, String)> {
    let checked = check(top, path)?;
    let mut file = open(&checked.path).with_context(|| format!("open {}", checked.rel))?;
    // The handle, not the path: what was opened is still a small regular file.
    let meta = file.metadata()?;
    if !meta.is_file() {
        bail!("{}: not a regular file", checked.rel);
    }
    let mut bytes = Vec::new();
    (&mut file).take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE {
        bail!("{}: over {} KB", checked.rel, MAX_FILE / 1024);
    }
    let text =
        String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("{}: not UTF-8 text", checked.rel))?;
    Ok((checked, crate::shown(&text, true)))
}

/// Opens `path` without following a symlink put there since it was checked, and
/// without waiting on a pipe that replaced it.
fn open(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

/// The markdown and text files the session's checkout changed: on its branch since
/// it left `default` (the default branch's name, if known), and changed or
/// untracked in the working tree. Each passed [`check`]; newest first.
pub fn list(top: &Path, default: Option<&str>) -> Vec<Checked> {
    let mut names: Vec<String> = Vec::new();
    let base = default.and_then(|d| {
        [format!("origin/{d}"), d.to_owned()]
            .into_iter()
            .find_map(|tip| git(top, &["merge-base", "HEAD", &tip]).ok())
    });
    let changed = match &base {
        Some(base) => git(
            top,
            &["diff", "--name-only", "-z", "--no-renames", base.trim()],
        ),
        None => git(top, &["diff", "--name-only", "-z", "--no-renames", "HEAD"]),
    };
    let untracked = git(top, &["ls-files", "-z", "--others", "--exclude-standard"]);
    for out in [changed, untracked].into_iter().flatten() {
        for name in out.split('\0').filter(|n| !n.is_empty()) {
            if readable_kind(name) && !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
    }
    let mut files: Vec<Checked> = names
        .iter()
        .take(MAX_LISTED * 2)
        .filter_map(|n| check(top, n).ok())
        .collect();
    files.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.rel.cmp(&b.rel)));
    files.truncate(MAX_LISTED);
    files
}

/// Of `paths` (mentions in chat or a pane), the ones that pass [`check`], each as
/// given with its relative path.
pub fn passing(top: &Path, paths: &[String]) -> Vec<(String, String)> {
    paths
        .iter()
        .take(MAX_LISTED)
        .filter_map(|p| check(top, p).ok().map(|c| (p.clone(), c.rel)))
        .collect()
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .no_window()
        .args(args)
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .context("run git")?;
    if !out.status.success() {
        bail!("git {} failed", args.join(" "));
    }
    String::from_utf8(out.stdout).context("git's output isn't UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    struct Repo {
        _tmp: TempDir,
        top: PathBuf,
    }

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sh(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn repo() -> Repo {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "valk-files-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = TempDir(dir.clone());
        sh(&dir, &["init", "-q"]);
        std::fs::write(dir.join("README.md"), "# hi\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "secret.md\nbuild/\n").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-qm", "first"]);
        let top = top(&dir).unwrap();
        Repo { _tmp: tmp, top }
    }

    fn refused(r: &Repo, path: &str, why: &str) {
        let e = check(&r.top, path).expect_err(path).to_string();
        assert!(e.contains(why), "{path}: {e}");
    }

    #[test]
    fn reads_a_tracked_markdown_file() {
        let r = repo();
        let (c, text) = read(&r.top, "README.md").unwrap();
        assert_eq!(c.rel, "README.md");
        assert_eq!(text, "# hi\n");
    }

    #[test]
    fn reads_an_untracked_file_in_a_folder() {
        let r = repo();
        std::fs::create_dir(r.top.join("notes")).unwrap();
        std::fs::write(r.top.join("notes/plan.TXT"), "plan").unwrap();
        assert_eq!(read(&r.top, "notes/plan.TXT").unwrap().1, "plan");
    }

    #[test]
    fn takes_an_absolute_path_under_the_top_level() {
        let r = repo();
        let abs = r.top.join("README.md");
        assert_eq!(
            check(&r.top, abs.to_str().unwrap()).unwrap().rel,
            "README.md"
        );
    }

    #[test]
    fn refuses_the_home_folder_and_root_as_top_level() {
        // `/` holds no repository here, and a home folder that is one is too wide.
        assert!(top(Path::new("/")).is_err());
        let r = repo();
        let e = top_in(&r.top, &r.top).unwrap_err().to_string();
        assert!(e.contains("too wide"), "{e}");
        let e = top_in(&r.top.join("README.md"), &r.top);
        assert!(e.is_err());
    }

    #[test]
    fn refuses_other_kinds_of_file() {
        let r = repo();
        std::fs::write(r.top.join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(r.top.join("x.env"), "A=1").unwrap();
        refused(&r, "main.rs", "only markdown and text");
        refused(&r, "x.env", "only markdown and text");
        refused(&r, ".gitignore", "only markdown and text");
    }

    #[test]
    fn refuses_a_path_outside_the_top_level() {
        let r = repo();
        refused(&r, "../outside.md", "not a plain path");
        refused(&r, "./README.md", "not a plain path");
        refused(&r, "/etc/motd.txt", "outside the repository");
        refused(&r, "a\\..\\b.md", "not a plain path");
        refused(&r, "", "no file named");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_anywhere_on_the_path() {
        let r = repo();
        let outside = r.top.parent().unwrap().join(format!(
            "{}-out",
            r.top.file_name().unwrap().to_str().unwrap()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        let _gone = TempDir(outside.clone());
        std::fs::write(outside.join("x.md"), "secret").unwrap();
        std::os::unix::fs::symlink(outside.join("x.md"), r.top.join("link.md")).unwrap();
        std::os::unix::fs::symlink(&outside, r.top.join("dir")).unwrap();
        std::os::unix::fs::symlink(r.top.join("README.md"), r.top.join("inside.md")).unwrap();
        refused(&r, "link.md", "symlink");
        refused(&r, "dir/x.md", "symlink");
        refused(&r, "inside.md", "symlink");
    }

    #[test]
    fn refuses_a_gitignored_file() {
        let r = repo();
        std::fs::write(r.top.join("secret.md"), "key").unwrap();
        std::fs::create_dir(r.top.join("build")).unwrap();
        std::fs::write(r.top.join("build/out.txt"), "x").unwrap();
        refused(&r, "secret.md", "git ignores it");
        refused(&r, "build/out.txt", "git ignores it");
    }

    #[test]
    fn a_glob_in_the_name_matches_only_itself() {
        let r = repo();
        std::fs::write(r.top.join("secret.md"), "key").unwrap();
        // `*.md` would match README.md as a pathspec; literally, no such file.
        refused(&r, "*.md", "no such file");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_file_that_is_not_regular() {
        let r = repo();
        std::fs::create_dir(r.top.join("folder.md")).unwrap();
        refused(&r, "folder.md", "not a regular file");
        let fifo = r.top.join("pipe.md");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        // SAFETY: a valid C string for a new path.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        refused(&r, "pipe.md", "not a regular file");
    }

    #[test]
    fn refuses_a_file_over_the_cap() {
        let r = repo();
        std::fs::write(r.top.join("big.md"), vec![b'a'; MAX_FILE as usize + 1]).unwrap();
        refused(&r, "big.md", "over 200 KB");
        std::fs::write(r.top.join("edge.md"), vec![b'a'; MAX_FILE as usize]).unwrap();
        assert!(read(&r.top, "edge.md").is_ok());
    }

    #[test]
    fn refuses_text_that_is_not_utf8() {
        let r = repo();
        std::fs::write(r.top.join("latin1.txt"), b"caf\xe9").unwrap();
        let e = read(&r.top, "latin1.txt").unwrap_err().to_string();
        assert!(e.contains("not UTF-8"), "{e}");
    }

    #[test]
    fn refuses_a_missing_file() {
        let r = repo();
        refused(&r, "nope.md", "no such file");
    }

    #[test]
    fn cleans_the_text_like_a_doc_body() {
        let r = repo();
        std::fs::write(r.top.join("x.md"), "a\u{1b}[31mb\u{202e}c\td\r\n").unwrap();
        assert_eq!(read(&r.top, "x.md").unwrap().1, "a[31mbc\td\n");
    }

    #[test]
    fn lists_what_the_branch_and_working_tree_changed() {
        let r = repo();
        sh(&r.top, &["checkout", "-qb", "feat"]);
        std::fs::write(r.top.join("DESIGN.md"), "d").unwrap();
        std::fs::write(r.top.join("code.rs"), "c").unwrap();
        sh(&r.top, &["add", "."]);
        sh(&r.top, &["commit", "-qm", "two"]);
        std::fs::write(r.top.join("README.md"), "# changed\n").unwrap();
        std::fs::write(r.top.join("new.txt"), "n").unwrap();
        std::fs::write(r.top.join("secret.md"), "s").unwrap();
        let mut got: Vec<String> = list(&r.top, Some("main"))
            .into_iter()
            .map(|c| c.rel)
            .collect();
        got.sort();
        assert_eq!(got, ["DESIGN.md", "README.md", "new.txt"]);
    }

    #[test]
    fn checks_mentions() {
        let r = repo();
        let got = passing(
            &r.top,
            &["README.md".into(), "secret.md".into(), "../x.md".into()],
        );
        assert_eq!(got, [("README.md".to_owned(), "README.md".to_owned())]);
    }
}

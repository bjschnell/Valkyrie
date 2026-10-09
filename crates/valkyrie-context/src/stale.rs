//! Decisions that may have gone out of date (DESIGN §6.4): ones about things that
//! change get a review date, and ones about files are anchored to them, so a file
//! deleted or rewritten since the decision was confirmed flags it for review.

use std::path::Path;
use std::process::{Command, Stdio};
use valkyrie_proto::{Decision, DecisionStatus};

/// Review interval, in days, for decisions about versions and endpoints.
pub const VOLATILE_DAYS: u32 = 30;
/// Lines changed in a decision's files since it was confirmed that call for a
/// second look.
pub const CHURN_LINES: u64 = 150;

/// Files a decision's text names that exist in the project, relative to `root`:
/// tokens with a slash or an extension, found below `root` (or `cwd`).
pub fn anchors(root: &Path, cwd: &Path, text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Ok(root) = valkyrie_proto::canonical(root) else {
        return out;
    };
    for token in text.split(|c: char| c.is_whitespace() || "`'\"(),;:<>[]{}".contains(c)) {
        let token = token.trim_end_matches(['.', '!', '?']);
        let looks_like_path = token.contains('/')
            || token.rsplit_once('.').is_some_and(|(stem, ext)| {
                !stem.is_empty()
                    && (1..=5).contains(&ext.len())
                    && ext.chars().all(|c| c.is_ascii_alphanumeric())
            });
        if !looks_like_path || token.contains("://") || token.len() > 200 {
            continue;
        }
        let path = Path::new(token);
        let found = [root.join(path), cwd.join(path)]
            .into_iter()
            .chain(path.is_absolute().then(|| path.to_path_buf()))
            .find(|p| p.exists());
        let Some(found) = found else { continue };
        let Ok(found) = valkyrie_proto::canonical(&found) else {
            continue;
        };
        let Ok(rel) = found.strip_prefix(&root) else {
            continue;
        };
        let rel = rel.display().to_string();
        // Decision files can be exported to a repo shared across platforms, and
        // Git pathspecs use forward slashes on Windows too.
        #[cfg(windows)]
        let rel = rel.replace('\\', "/");
        if !rel.is_empty() && !out.contains(&rel) {
            out.push(rel);
        }
    }
    out.truncate(8);
    out
}

/// Whether a decision is about something that changes on its own schedule:
/// versions (`1.2`, `v3`) or URLs.
pub fn volatile(text: &str) -> bool {
    text.contains("://")
        || text
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
            .any(|w| {
                let w = w.strip_prefix('v').unwrap_or(w);
                let parts: Vec<&str> = w.split('.').collect();
                parts.len() >= 2
                    && parts.len() <= 4
                    && parts
                        .iter()
                        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
            })
}

/// When a decision was last confirmed: when a human said it holds, else when it was
/// last changed.
pub fn confirmed(d: &Decision) -> u64 {
    if d.fresh.confirmed > 0 {
        d.fresh.confirmed
    } else {
        d.updated
    }
}

/// Why an active decision is due for review by date, if it is.
pub fn due(d: &Decision, now: u64) -> Option<String> {
    let days = d.fresh.review_every?;
    let at = confirmed(d) + u64::from(days) * 86_400;
    (d.status == DecisionStatus::Active && now >= at).then(|| {
        format!(
            "due for review: it's about something that changes, last confirmed over {days} days ago"
        )
    })
}

/// Whether `commit` names a commit, as `git rev-parse` prints one, and nothing else:
/// it reaches `git diff`'s arguments, and files and clients can set it.
pub fn valid_commit(commit: &str) -> bool {
    (4..=64).contains(&commit.len()) && commit.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Why an active decision's files suggest it may no longer hold: one deleted, or
/// many lines changed since the commit it was confirmed at. Runs `git` in `root`.
pub fn churn(root: &Path, d: &Decision) -> Option<String> {
    if d.status != DecisionStatus::Active || d.fresh.anchors.is_empty() {
        return None;
    }
    if let Some(gone) = d.fresh.anchors.iter().find(|a| !root.join(a).exists()) {
        return Some(format!("{gone} no longer exists"));
    }
    let commit = d.provenance.commit.as_deref().filter(|c| valid_commit(c))?;
    let out = Command::new("git")
        .args([
            "diff",
            "--numstat",
            "--end-of-options",
            commit,
            "HEAD",
            "--",
        ])
        .args(&d.fresh.anchors)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let (mut added, mut removed) = (0u64, 0u64);
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut cols = line.split('\t');
        added += cols.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        removed += cols.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    }
    (added + removed >= CHURN_LINES).then(|| {
        format!(
            "{} changed a lot since it was confirmed (+{added}/-{removed})",
            d.fresh.anchors.join(", ")
        )
    })
}

/// Why `d` should be looked at again, if it should.
pub fn review(root: &Path, d: &Decision, now: u64) -> Option<String> {
    due(d, now).or_else(|| churn(root, d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use valkyrie_proto::{DecisionKind, Freshness, Provenance};

    fn decision(fresh: Freshness, commit: Option<&str>) -> Decision {
        Decision {
            id: 1,
            project: "/r".into(),
            title: "t".into(),
            body: String::new(),
            kind: DecisionKind::Decision,
            status: DecisionStatus::Active,
            created: 0,
            updated: 1000,
            supersedes: None,
            superseded_by: None,
            provenance: Provenance {
                commit: commit.map(str::to_owned),
                ..Provenance::default()
            },
            fresh,
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
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    #[test]
    fn anchors_are_the_files_a_decision_names() {
        let root = std::env::temp_dir().join(format!("valk-anchors-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/sub")).unwrap();
        std::fs::write(root.join("src/sub/auth.rs"), "").unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let text = "Keep `src/sub/auth.rs` small (see Cargo.toml). Not src/missing.rs, \
                    nor https://x.y/z, nor e.g. or v1.2.";
        assert_eq!(
            anchors(&root, &root.join("src"), text),
            ["src/sub/auth.rs", "Cargo.toml"]
        );
        assert_eq!(
            anchors(&root, &root.join("src"), "sub/auth.rs is it"),
            ["src/sub/auth.rs"]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn only_hex_commits_reach_git() {
        assert!(valid_commit("abc1234") && valid_commit(&"f".repeat(40)));
        for bad in ["--output=/tmp/x", "HEAD", "abc", "abc1234 x", "", "ab-cd"] {
            assert!(!valid_commit(bad), "{bad}");
        }
        let d = decision(
            Freshness {
                anchors: vec!["Cargo.toml".into()],
                ..Freshness::default()
            },
            Some("--output=/tmp/valk-pwned"),
        );
        assert!(churn(Path::new(env!("CARGO_MANIFEST_DIR")), &d).is_none());
        assert!(!Path::new("/tmp/valk-pwned").exists());
    }

    #[test]
    fn versions_and_urls_are_volatile() {
        assert!(volatile("Pin tokio to 1.47"));
        assert!(volatile("Use the v2 API at https://api.x.com"));
        assert!(volatile("node v20.11.0 only"));
        assert!(!volatile("Use pnpm, not npm"));
        assert!(!volatile("Keep src/auth.rs small. Really."));
    }

    #[test]
    fn due_after_its_interval_from_the_last_confirmation() {
        let mut d = decision(
            Freshness {
                review_every: Some(30),
                ..Freshness::default()
            },
            None,
        );
        assert!(due(&d, 1000 + 29 * 86_400).is_none());
        assert!(due(&d, 1000 + 30 * 86_400).is_some());
        d.fresh.confirmed = 40 * 86_400;
        assert!(due(&d, 41 * 86_400).is_none());
        d.fresh.review_every = None;
        assert!(due(&d, u64::MAX / 2).is_none());
    }

    #[test]
    fn churn_in_a_decisions_files_flags_it() {
        let root = std::env::temp_dir().join(format!("valk-churn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q"]);
        std::fs::write(root.join("a.rs"), "x\n").unwrap();
        std::fs::write(root.join("b.rs"), "x\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "one"]);
        let at = git(&root, &["rev-parse", "--short", "HEAD"]);
        let anchored = |files: &[&str]| {
            decision(
                Freshness {
                    anchors: files.iter().map(|s| s.to_string()).collect(),
                    ..Freshness::default()
                },
                Some(&at),
            )
        };
        assert!(churn(&root, &anchored(&["a.rs"])).is_none());
        std::fs::write(root.join("a.rs"), "y\n".repeat(200)).unwrap();
        std::fs::remove_file(root.join("b.rs")).unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "two"]);
        let why = churn(&root, &anchored(&["a.rs"])).unwrap();
        assert!(
            why.starts_with("a.rs changed a lot since it was confirmed (+200/-1)"),
            "{why}"
        );
        assert_eq!(
            churn(&root, &anchored(&["b.rs"])).unwrap(),
            "b.rs no longer exists"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}

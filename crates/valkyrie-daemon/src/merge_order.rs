//! The order to merge a repository's open pull requests in (backlog #215). With
//! several agents working at once, many PRs go up together, and which to merge
//! first, and which of them collide, isn't visible until one merges and the rest
//! turn red. The PR poll already reads each one; this tells which pairs would
//! conflict with each other, from `git merge-tree` on their head commits (a real
//! merge, not two lists of files compared), and puts them in an order:
//!
//! 1. ready ones first (green, then pending, red, unread), and one conflicting
//!    with its base last, since it needs the base merged in before anything;
//! 2. within that, one colliding with fewer of the others first: a PR that
//!    collides with three goes after them, so only it needs the base merged in,
//!    instead of all three;
//! 3. then the oldest (lowest number) first.
//!
//! It only suggests: merging stays the user's (ADR-0015). What follows a merge is
//! `watching`'s: a PR predicted to collide with the one that merged is flagged at
//! once, without waiting on GitHub to work its mergeability out again.

use std::path::Path;
use std::process::{Command, Stdio};
use valkyrie_proto::{NoWindow, PrState};

/// Files named in a note; the rest are counted.
const SHOWN: usize = 2;

/// Which files merging commits `a` and `b` would conflict on, from `git
/// merge-tree` in `repo` (any checkout of it: worktrees share their objects).
/// Empty when they merge cleanly; `None` when git can't tell (a commit isn't
/// here, the two share no history, git is older than 2.38).
pub(crate) fn clash(repo: &Path, a: &str, b: &str) -> Option<Vec<String>> {
    let git = |args: &[&str]| {
        Command::new("git")
            .no_window()
            .args(args)
            .current_dir(repo)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
    };
    for head in [a, b] {
        let there = git(&["cat-file", "-e", &format!("{head}^{{commit}}")])?;
        if !there.status.success() {
            return None;
        }
    }
    let out = git(&[
        "merge-tree",
        "--write-tree",
        "--name-only",
        "--no-messages",
        a,
        b,
    ])?;
    match out.status.code() {
        Some(0) => Some(Vec::new()),
        // The tree, then each conflicted file (once per stage).
        Some(1) => {
            let mut files: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .skip(1)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
            files.sort();
            files.dedup();
            Some(files)
        }
        _ => None,
    }
}

/// One open PR, for `plan`.
#[derive(Debug, Clone)]
pub(crate) struct Open {
    /// Its number, when its URL says.
    pub number: Option<u32>,
    pub state: PrState,
}

impl Open {
    fn label(&self) -> String {
        self.number.map_or("a PR".to_owned(), |n| format!("#{n}"))
    }
}

/// Each of `prs`' place in the merge order, from 1, and a line on whom it
/// collides with. `clash(i, j)` is the files PRs `i` and `j` conflict on: empty
/// when they don't, or when that isn't known.
pub(crate) fn plan(
    prs: &[Open],
    clash: impl Fn(usize, usize) -> Vec<String>,
) -> Vec<(u32, Option<String>)> {
    let n = prs.len();
    let files: Vec<Vec<Vec<String>>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| if i == j { Vec::new() } else { clash(i, j) })
                .collect()
        })
        .collect();
    let partners = |i: usize| files[i].iter().filter(|f| !f.is_empty()).count();
    let tier = |s: PrState| match s {
        PrState::Green => 0,
        PrState::Pending => 1,
        PrState::Red => 2,
        PrState::Unknown => 3,
        PrState::Conflicting => 4,
    };
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| {
        (
            tier(prs[i].state),
            partners(i),
            prs[i].number.unwrap_or(u32::MAX),
            i,
        )
    });
    let mut place = vec![0; n];
    for (at, &i) in order.iter().enumerate() {
        place[i] = at;
    }
    (0..n)
        .map(|i| {
            let (mut before, mut after) = (Vec::new(), Vec::new());
            for j in order.iter().copied().filter(|&j| !files[i][j].is_empty()) {
                if place[j] < place[i] {
                    &mut before
                } else {
                    &mut after
                }
                .push(j);
            }
            let note = if !before.is_empty() {
                Some(format!(
                    "merge after {}: conflicts on {}; then it needs the base merged in",
                    labels(prs, &before),
                    named(&files[i], &before)
                ))
            } else if !after.is_empty() {
                Some(format!(
                    "merge before {}: conflicts on {}",
                    labels(prs, &after),
                    named(&files[i], &after)
                ))
            } else {
                None
            };
            (place[i] as u32 + 1, note)
        })
        .collect()
}

/// "#3", "#3 and #5", "#3, #5 and #8".
fn labels(prs: &[Open], which: &[usize]) -> String {
    let names: Vec<String> = which.iter().map(|&j| prs[j].label()).collect();
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => names.concat(),
    }
}

/// The files `with` conflict on, the first few named and the rest counted.
fn named(files: &[Vec<String>], with: &[usize]) -> String {
    let mut all: Vec<&str> = with
        .iter()
        .flat_map(|&j| files[j].iter().map(String::as_str))
        .collect();
    all.sort_unstable();
    all.dedup();
    shown(&all)
}

/// The first few of `files`, and how many more.
pub(crate) fn shown(files: &[&str]) -> String {
    let mut out = files
        .iter()
        .take(SHOWN)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if files.len() > SHOWN {
        out.push_str(&format!(" +{}", files.len() - SHOWN));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(number: u32, state: PrState) -> Open {
        Open {
            number: Some(number),
            state,
        }
    }

    #[test]
    fn ready_first_then_fewer_collisions_then_oldest() {
        let prs = [
            open(10, PrState::Red),
            open(11, PrState::Green),
            open(12, PrState::Green),
            open(13, PrState::Conflicting),
            open(14, PrState::Green),
        ];
        // #12 collides with #11 and #14; #11 and #14 with #12 only.
        let pairs = [(1, 2), (2, 4)];
        let clash = |i: usize, j: usize| {
            if pairs.contains(&(i.min(j), i.max(j))) {
                vec!["src/a.rs".to_owned()]
            } else {
                Vec::new()
            }
        };
        let plan = plan(&prs, clash);
        let places: Vec<u32> = plan.iter().map(|p| p.0).collect();
        // #11, #14 (one collision each, oldest first), #12 (two), red #10, then #13.
        assert_eq!(places, [4, 1, 3, 5, 2]);
        assert_eq!(plan[0].1, None);
        assert_eq!(
            plan[1].1.as_deref(),
            Some("merge before #12: conflicts on src/a.rs")
        );
        assert_eq!(
            plan[2].1.as_deref(),
            Some(
                "merge after #11 and #14: conflicts on src/a.rs; then it needs the base merged in"
            )
        );
    }

    #[test]
    fn many_files_are_counted() {
        assert_eq!(shown(&["a", "b", "c", "d"]), "a, b +2");
        assert_eq!(shown(&["a"]), "a");
    }

    /// A real repository: two branches that change one line differently clash on
    /// it, a third that changes another file doesn't, and an unknown commit can't
    /// be told.
    #[cfg(unix)]
    #[test]
    fn merge_tree_names_the_conflicting_files() {
        let dir = std::env::temp_dir().join(format!("valkyrie-merge-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "init"]);
        let branch = |name: &str, file: &str, text: &str| {
            git(&["checkout", "-q", "-b", name, "main"]);
            std::fs::write(dir.join(file), text).unwrap();
            git(&["add", "."]);
            git(&["commit", "-qm", name]);
            git(&["rev-parse", "HEAD"])
        };
        let a = branch("a", "a.txt", "two\n");
        let b = branch("b", "a.txt", "three\n");
        let c = branch("c", "c.txt", "c\n");
        assert_eq!(clash(&dir, &a, &b), Some(vec!["a.txt".to_owned()]));
        assert_eq!(clash(&dir, &a, &c), Some(vec![]));
        assert_eq!(clash(&dir, &a, &"0".repeat(40)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! The flake call's fitted model (ADR-0019 §2, decision #45). The hand rules in
//! [`crate::flake`] decide first; every red pull request the user answers (rerun:
//! a flake, ci-fix: real) is a labelled example in `supervisor.jsonl`, with the
//! signals the rules read. Once there are [`MIN_EXAMPLES`] of them, both kinds
//! among them, a small decision tree fitted to them replaces the rules. It's
//! deterministic, explains itself in one line, and says nothing where its leaf is
//! mixed, so the rules and the model still decide what it can't.

use crate::flake::{self, Bug, Failure};

/// Labelled answers it takes before a tree replaces the hand rules.
pub const MIN_EXAMPLES: usize = 40;
/// Deepest a tree grows: three questions explain themselves in a line.
const MAX_DEPTH: usize = 3;
/// Fewest examples on either side of a split.
const MIN_LEAF: usize = 3;
/// A leaf answers only when this share of its examples agree.
const SURE: f64 = 0.8;

/// What the flake call reads about a red pull request, as yes-or-no signals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Features {
    /// Every failed check's log names an open bug's panic or test.
    pub bug_match: bool,
    /// A failed check panicked in a file the pull request changes.
    pub touches_diff: bool,
    /// Every failed check passed on another run of the same commit.
    pub passed_before: bool,
    /// Which runners failed, from the checks' names.
    pub windows: bool,
    pub macos: bool,
    pub linux: bool,
}

const NAMES: [&str; 6] = [
    "bug match",
    "panics in the diff",
    "passed before",
    "windows",
    "macos",
    "linux",
];

impl Features {
    /// The signals of a red pull request's failures.
    pub fn of(failures: &[Failure], changed: &[String], bugs: &[Bug]) -> Features {
        let any = !failures.is_empty();
        let named = |words: &[&str]| {
            failures.iter().any(|f| {
                let check = f.check.to_ascii_lowercase();
                words.iter().any(|w| check.contains(w))
            })
        };
        Features {
            bug_match: any
                && failures
                    .iter()
                    .all(|f| flake::matching_bug(&f.log, bugs).is_some()),
            touches_diff: failures
                .iter()
                .any(|f| flake::touches(&f.log, changed).is_some()),
            passed_before: any && failures.iter().all(|f| f.passed_before),
            windows: named(&["windows"]),
            macos: named(&["macos", "mac-os", "darwin"]),
            linux: named(&["ubuntu", "linux"]),
        }
    }

    fn get(&self, i: usize) -> bool {
        [
            self.bug_match,
            self.touches_diff,
            self.passed_before,
            self.windows,
            self.macos,
            self.linux,
        ][i]
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "bug_match": self.bug_match, "touches_diff": self.touches_diff,
            "passed_before": self.passed_before, "windows": self.windows,
            "macos": self.macos, "linux": self.linux,
        })
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Features> {
        let b = |k: &str| v[k].as_bool();
        Some(Features {
            bug_match: b("bug_match")?,
            touches_diff: b("touches_diff")?,
            passed_before: b("passed_before")?,
            windows: b("windows")?,
            macos: b("macos")?,
            linux: b("linux")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Leaf {
        flake: usize,
        real: usize,
    },
    Split {
        feature: usize,
        yes: Box<Node>,
        no: Box<Node>,
    },
}

/// A tree fitted to the user's answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    root: Node,
    pub examples: usize,
}

fn gini(flake: usize, real: usize) -> f64 {
    let n = (flake + real) as f64;
    if n == 0.0 {
        return 0.0;
    }
    let p = flake as f64 / n;
    2.0 * p * (1.0 - p)
}

fn counts(examples: &[&(Features, bool)]) -> (usize, usize) {
    let flake = examples.iter().filter(|(_, f)| *f).count();
    (flake, examples.len() - flake)
}

fn grow(examples: &[&(Features, bool)], depth: usize) -> Node {
    let (flake, real) = counts(examples);
    let leaf = Node::Leaf { flake, real };
    if depth == MAX_DEPTH || flake == 0 || real == 0 {
        return leaf;
    }
    let n = examples.len() as f64;
    let before = gini(flake, real);
    let mut best: Option<(f64, usize)> = None;
    for feature in 0..NAMES.len() {
        let (yes, no): (Vec<_>, Vec<_>) = examples.iter().partition(|(x, _)| x.get(feature));
        if yes.len() < MIN_LEAF || no.len() < MIN_LEAF {
            continue;
        }
        let (yf, yr) = counts(&yes);
        let (nf, nr) = counts(&no);
        let after = (yes.len() as f64 * gini(yf, yr) + no.len() as f64 * gini(nf, nr)) / n;
        let gain = before - after;
        // The first feature wins a tie, so the same answers always fit the same tree.
        if gain > 1e-9 && best.is_none_or(|(g, _)| gain > g + 1e-12) {
            best = Some((gain, feature));
        }
    }
    let Some((_, feature)) = best else {
        return leaf;
    };
    let (yes, no): (Vec<_>, Vec<_>) = examples.iter().partition(|(x, _)| x.get(feature));
    Node::Split {
        feature,
        yes: Box::new(grow(&yes, depth + 1)),
        no: Box::new(grow(&no, depth + 1)),
    }
}

/// Fits a tree to labelled answers (`true`: a flake); `None` until there are
/// [`MIN_EXAMPLES`], with both kinds among them.
pub fn fit(examples: &[(Features, bool)]) -> Option<Tree> {
    let all: Vec<&(Features, bool)> = examples.iter().collect();
    let (flake, real) = counts(&all);
    if all.len() < MIN_EXAMPLES || flake == 0 || real == 0 {
        return None;
    }
    Some(Tree {
        root: grow(&all, 0),
        examples: all.len(),
    })
}

impl Tree {
    /// `Some((flake, why))` where the leaf `x` lands in is sure; `None` where it's
    /// mixed or holds too few answers to go by.
    pub fn predict(&self, x: &Features) -> Option<(bool, String)> {
        let mut path = Vec::new();
        let mut node = &self.root;
        loop {
            match node {
                Node::Split { feature, yes, no } => {
                    let on = x.get(*feature);
                    path.push(if on {
                        NAMES[*feature].to_owned()
                    } else {
                        format!("no {}", NAMES[*feature])
                    });
                    node = if on { yes } else { no };
                }
                Node::Leaf { flake, real } => {
                    let n = flake + real;
                    let top = (*flake).max(*real);
                    if n < MIN_LEAF || (top as f64) < SURE * n as f64 {
                        return None;
                    }
                    let is_flake = flake > real;
                    let how = if path.is_empty() {
                        "always".to_owned()
                    } else {
                        path.join(", ")
                    };
                    let what = if is_flake { "a flake" } else { "real" };
                    return Some((
                        is_flake,
                        format!(
                            "fitted to {} answers: {how}, {what} {top} times of {n}",
                            self.examples
                        ),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(bug_match: bool, passed_before: bool, windows: bool) -> Features {
        Features {
            bug_match,
            passed_before,
            windows,
            linux: !windows,
            ..Features::default()
        }
    }

    #[test]
    fn reads_the_signals_the_rules_read() {
        let bugs = vec![(17, "panicked at crates/a/src/x.rs:12".to_owned())];
        let fail = |check: &str, log: &str, passed_before| Failure {
            check: check.into(),
            run: Some(1),
            passed_before,
            log: log.into(),
        };
        let f = Features::of(
            &[
                fail(
                    "ci / windows-latest",
                    "thread 'x' panicked at crates/a/src/x.rs:12:5",
                    true,
                ),
                fail("ci / ubuntu", "panicked at crates/b/src/y.rs:3:1", false),
            ],
            &["crates/b/src/y.rs".to_owned()],
            &bugs,
        );
        assert_eq!(
            f,
            Features {
                bug_match: false,
                touches_diff: true,
                passed_before: false,
                windows: true,
                macos: false,
                linux: true,
            }
        );
        assert_eq!(Features::from_json(&f.to_json()), Some(f));
        assert_eq!(Features::of(&[], &[], &bugs), Features::default());
    }

    #[test]
    fn no_tree_until_there_are_enough_answers_of_both_kinds() {
        let flakes = vec![(x(true, false, true), true); MIN_EXAMPLES];
        assert_eq!(fit(&flakes), None);
        let mut few = vec![(x(true, false, true), true); 10];
        few.extend(vec![(x(false, false, false), false); 10]);
        assert_eq!(fit(&few), None);
    }

    #[test]
    fn a_fitted_tree_splits_on_what_tells_them_apart_and_abstains_where_mixed() {
        let mut answers = Vec::new();
        // Windows failures matching a bug were flakes; on Linux without one, real.
        answers.extend(vec![(x(true, false, true), true); 20]);
        answers.extend(vec![(x(false, false, false), false); 20]);
        // Passed before but no bug: half and half.
        answers.extend(vec![(x(false, true, false), true); 4]);
        answers.extend(vec![(x(false, true, false), false); 4]);
        let tree = fit(&answers).unwrap();
        assert_eq!(tree.examples, 48);
        let (flake, why) = tree.predict(&x(true, false, true)).unwrap();
        assert!(flake, "{why}");
        assert!(why.starts_with("fitted to 48 answers: bug match"), "{why}");
        assert!(!tree.predict(&x(false, false, false)).unwrap().0);
        assert_eq!(tree.predict(&x(false, true, false)), None);
        // The same answers fit the same tree.
        assert_eq!(fit(&answers), Some(tree));
    }
}

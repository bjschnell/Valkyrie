//! The merge guard (ADR-0015 §1): a shell command an agent is about to run that
//! merges a pull request or pushes to the default branch. Merging is the user's,
//! so `valk hook` answers such a command with a confirm the user answers at their
//! own terminal (Claude Code) or a refusal (Codex).
//!
//! It reads command text, so it catches mistakes and the obvious forms, not an
//! agent set on getting around it (a script that calls the API gets through).

use std::path::{Path, PathBuf};

/// What a command would do that is the user's to decide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    /// `gh pr merge`, or a `gh api` call to a pull request's merge endpoint.
    Merge,
    /// `git push` of the default branch, or onto it; holds the branch's name.
    Push(String),
}

impl Hit {
    pub fn kind(&self) -> &'static str {
        match self {
            Hit::Merge => "merge",
            Hit::Push(_) => "push",
        }
    }

    /// What the command does, for the confirm and the refusal.
    pub fn what(&self) -> String {
        match self {
            Hit::Merge => "This command merges a pull request.".to_owned(),
            Hit::Push(branch) => format!("This command pushes to the default branch ({branch})."),
        }
    }
}

/// The git a check needs, so tests can stand in for a repository.
pub trait Repo {
    fn default_branch(&self, dir: &Path) -> Option<String>;
    fn current_branch(&self, dir: &Path) -> Option<String>;
}

/// The real repository on disk.
pub struct Git;

impl Repo for Git {
    fn default_branch(&self, dir: &Path) -> Option<String> {
        valkyrie_daemon::work::default_branch_at(dir)
    }
    fn current_branch(&self, dir: &Path) -> Option<String> {
        valkyrie_daemon::work::current_branch(dir)
    }
}

/// The first part of `command` that merges or pushes to the default branch, run
/// from `cwd`. A `cd` earlier in the same command line moves where later parts run.
pub fn check(command: &str, cwd: &Path, repo: &dyn Repo) -> Option<Hit> {
    check_in(command, cwd, repo, 0)
}

fn check_in(command: &str, cwd: &Path, repo: &dyn Repo, depth: u8) -> Option<Hit> {
    let mut dir = cwd.to_path_buf();
    let deeper = depth < 3;
    for words in split(command) {
        // `"$(gh pr merge 81)"`: a substitution inside double quotes is one word.
        if deeper
            && let Some(hit) = words
                .iter()
                .filter_map(|w| w.find("$(").map(|i| &w[i + 2..]))
                .find_map(|inner| check_in(inner, &dir, repo, depth + 1))
        {
            return Some(hit);
        }
        let words = strip_prefixes(&words);
        let Some((program, args)) = words.split_first() else {
            continue;
        };
        let program = program.rsplit('/').next().unwrap_or(program);
        let hit = match program {
            "cd" => {
                if let Some(to) = args.first() {
                    dir = join(&dir, to);
                }
                None
            }
            "gh" => gh(args),
            "git" => git(args, &dir, repo),
            "eval" if deeper => check_in(&args.join(" "), &dir, repo, depth + 1),
            // `sh -c '…'` (or `-ec`, `-lc`): the string is another command line.
            "sh" | "bash" | "zsh" | "dash" | "fish" if deeper => args
                .iter()
                .position(|a| {
                    a.len() > 1 && a.starts_with('-') && !a.starts_with("--") && a.ends_with('c')
                })
                .and_then(|i| args.get(i + 1))
                .and_then(|inner| check_in(inner, &dir, repo, depth + 1)),
            _ => None,
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
}

fn join(dir: &Path, to: &str) -> PathBuf {
    if let Some(rest) = to.strip_prefix("~/") {
        return valkyrie_proto::home_dir().join(rest);
    }
    dir.join(to)
}

/// Leading `VAR=value`s, shell keywords, and wrappers that run the rest as a
/// command.
fn strip_prefixes(words: &[String]) -> &[String] {
    let mut words = words;
    loop {
        match words.first().map(String::as_str) {
            Some(w) if is_assignment(w) => words = &words[1..],
            Some("if" | "then" | "else" | "elif" | "do" | "while" | "until" | "{" | "!") => {
                words = &words[1..]
            }
            Some("timeout") => {
                words = &words[1..];
                while let Some(w) = words.first()
                    && w.starts_with('-')
                {
                    // `-k 5`, `-s KILL` take a value.
                    let takes_value = matches!(w.as_str(), "-k" | "-s");
                    words = &words[if takes_value { 2 } else { 1 }.min(words.len())..];
                }
                // The duration.
                words = &words[1.min(words.len())..];
            }
            Some("env" | "command" | "exec" | "nohup" | "time" | "sudo" | "xargs") => {
                words = &words[1..];
                // Their own options (`env -u X`, `sudo -E`), roughly.
                while let Some(w) = words.first()
                    && w.starts_with('-')
                {
                    let takes_value = matches!(w.as_str(), "-u" | "-C" | "-S");
                    words = &words[if takes_value { 2 } else { 1 }.min(words.len())..];
                }
            }
            _ => return words,
        }
    }
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.starts_with(|c: char| c.is_ascii_digit())
    })
}

fn gh(args: &[String]) -> Option<Hit> {
    // `gh -R owner/repo pr merge`: global flags may come first.
    let mut rest = args.iter().map(String::as_str).peekable();
    let mut positional = Vec::new();
    while let Some(arg) = rest.next() {
        if matches!(arg, "-R" | "--repo") {
            rest.next();
        } else if !arg.starts_with('-') {
            positional.push(arg);
            if positional.len() == 2 {
                break;
            }
        }
    }
    let has = |flag: &str| args.iter().any(|a| a == flag);
    match positional.as_slice() {
        // `--disable-auto` takes back an auto-merge rather than merging.
        ["pr", "merge"] => (!has("--disable-auto")).then_some(Hit::Merge),
        ["api", ..] => {
            // `-X GET` on the endpoint only asks whether it was merged.
            let get = args.windows(2).any(|w| {
                matches!(w[0].as_str(), "-X" | "--method") && w[1].eq_ignore_ascii_case("GET")
            }) || args.iter().any(|a| a.eq_ignore_ascii_case("--method=GET"));
            (!get
                && args
                    .iter()
                    .any(|a| merge_endpoint(a) || a.contains("mergePullRequest")))
            .then_some(Hit::Merge)
        }
        _ => None,
    }
}

/// `repos/o/r/pulls/12/merge`, with a placeholder or a query string allowed.
fn merge_endpoint(arg: &str) -> bool {
    let path = arg.split(['?', '#']).next().unwrap_or(arg);
    let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    parts
        .windows(3)
        .any(|w| w[0] == "pulls" && !w[1].is_empty() && w[2] == "merge")
        && parts.last() == Some(&"merge")
}

/// Git's own options before the subcommand; the ones here take a value.
const GIT_VALUE_OPTS: &[&str] = &["-C", "-c", "--git-dir", "--work-tree", "--namespace"];
/// `git push` options that take a value as the next word.
const PUSH_VALUE_OPTS: &[&str] = &["-o", "--push-option", "--repo", "--receive-pack", "--exec"];

fn git(args: &[String], cwd: &Path, repo: &dyn Repo) -> Option<Hit> {
    let mut dir = cwd.to_path_buf();
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        if !arg.starts_with('-') {
            break;
        }
        if arg == "-C"
            && let Some(to) = args.get(i + 1)
        {
            dir = join(&dir, to);
        }
        i += if GIT_VALUE_OPTS.contains(&arg.as_str()) {
            2
        } else {
            1
        };
    }
    if args.get(i).map(String::as_str) != Some("push") {
        return None;
    }
    let mut positional = Vec::new();
    let mut everything = false;
    // `--tags` or `--delete` with no refspec doesn't push the branch checked out.
    let mut not_the_branch = false;
    let mut rest = args[i + 1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--all" | "--mirror" | "--branches" => everything = true,
            // Pushes nothing.
            "-n" | "--dry-run" => return None,
            "--tags" | "--delete" | "-d" => not_the_branch = true,
            a if PUSH_VALUE_OPTS.contains(&a) => {
                rest.next();
            }
            a if a.starts_with('-') => {}
            a => positional.push(a),
        }
    }
    // A folder git can't read (`cd -`, a typo): a refspec naming main or master
    // still counts.
    let defaults = match repo.default_branch(&dir) {
        Some(default) => vec![default],
        None => vec!["main".to_owned(), "master".to_owned()],
    };
    let current = || repo.current_branch(&dir);
    let is_default = |branch: Option<&str>| branch.and_then(|b| defaults.iter().find(|d| *d == b));
    let onto_default = |refspec: &str| {
        let refspec = refspec.trim_start_matches('+');
        let (src, dst) = refspec.split_once(':').unwrap_or((refspec, refspec));
        let dst = dst.strip_prefix("refs/heads/").unwrap_or(dst);
        is_default(Some(dst)).cloned().or_else(|| {
            // `HEAD` or `@` alone pushes the branch checked out to its own name.
            (!refspec.contains(':') && matches!(src, "HEAD" | "@"))
                .then(|| is_default(current().as_deref()).cloned())
                .flatten()
        })
    };
    let hit = match positional.split_first() {
        _ if everything => Some(defaults[0].clone()),
        // No refspec: the branch checked out (push.default simple/current).
        None | Some((_, [])) if !not_the_branch => is_default(current().as_deref()).cloned(),
        None | Some((_, [])) => None,
        Some((_, refspecs)) => refspecs.iter().find_map(|r| onto_default(r)),
    };
    hit.map(Hit::Push)
}

/// `command` as simple commands, each a list of words: quotes and backslashes
/// resolved, split at `;`, `&`, `|`, newlines and parentheses. Not a shell, just
/// enough to read the obvious forms.
fn split(command: &str) -> Vec<Vec<String>> {
    let mut commands = vec![Vec::new()];
    let mut word: Option<String> = None;
    let mut chars = command.chars().peekable();
    let end_word = |word: &mut Option<String>, commands: &mut Vec<Vec<String>>| {
        if let Some(w) = word.take() {
            commands.last_mut().unwrap().push(w);
        }
    };
    while let Some(c) = chars.next() {
        match c {
            // A comment runs to the end of the line.
            '#' if word.is_none() => {
                while chars.peek().is_some_and(|&c| c != '\n') {
                    chars.next();
                }
            }
            '\'' => {
                let w = word.get_or_insert_with(String::new);
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    w.push(c);
                }
            }
            '"' => {
                let w = word.get_or_insert_with(String::new);
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => {
                            if let Some(next) = chars.next() {
                                w.push(next);
                            }
                        }
                        c => w.push(c),
                    }
                }
            }
            '\\' => {
                if let Some(next) = chars.next()
                    && next != '\n'
                {
                    word.get_or_insert_with(String::new).push(next);
                }
            }
            ';' | '&' | '|' | '\n' | '(' | ')' | '`' => {
                end_word(&mut word, &mut commands);
                if !commands.last().unwrap().is_empty() {
                    commands.push(Vec::new());
                }
            }
            c if c.is_whitespace() => end_word(&mut word, &mut commands),
            c => word.get_or_insert_with(String::new).push(c),
        }
    }
    end_word(&mut word, &mut commands);
    commands.retain(|c| !c.is_empty());
    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository whose default branch is `main`, on `branch` everywhere except
    /// under `/other`, where `main` is checked out.
    struct Fake {
        branch: &'static str,
    }

    impl Repo for Fake {
        fn default_branch(&self, dir: &Path) -> Option<String> {
            // `/nowhere`: not a repository git can read.
            (!dir.starts_with("/nowhere")).then(|| "main".into())
        }
        fn current_branch(&self, dir: &Path) -> Option<String> {
            if dir.starts_with("/nowhere") {
                return None;
            }
            Some(
                if dir.starts_with("/other") {
                    "main"
                } else {
                    self.branch
                }
                .into(),
            )
        }
    }

    fn on(branch: &'static str, command: &str) -> Option<Hit> {
        check(command, Path::new("/repo"), &Fake { branch })
    }

    #[test]
    fn merges_are_caught() {
        for command in [
            "gh pr merge 81",
            "gh pr merge --squash --delete-branch",
            "gh -R bjschnell/valkyrie pr merge 81 --auto",
            "/usr/bin/gh pr merge 81",
            "gh api -X PUT repos/bjschnell/valkyrie/pulls/81/merge",
            "gh api --method PUT /repos/{owner}/{repo}/pulls/81/merge -f merge_method=squash",
            "gh api graphql -f query='mutation { mergePullRequest(input: {}) { clientMutationId } }'",
            "cargo test && gh pr merge 81",
            "GH_TOKEN=x gh pr merge 81",
            "bash -c 'gh pr merge 81'",
            "echo $(gh pr merge 81)",
            "echo \"$(gh pr merge 81)\"",
            "if true; then gh pr merge 81; fi",
            "for n in 81; do gh pr merge $n; done",
            "{ gh pr merge 81; }",
            "timeout 60 gh pr merge 81",
            "timeout -k 5 60 gh pr merge 81",
            "eval 'gh pr merge 81'",
            "bash -ec 'gh pr merge 81'",
        ] {
            assert_eq!(on("feat/x", command), Some(Hit::Merge), "{command}");
        }
    }

    #[test]
    fn pushes_onto_the_default_branch_are_caught() {
        let main = Some(Hit::Push("main".into()));
        for command in [
            "git push origin main",
            "git push origin HEAD:main",
            "git push origin +main",
            "git push -f origin feat/x:main",
            "git push origin HEAD:refs/heads/main",
            "git push origin :main",
            "git push --all",
            "git push --mirror origin",
            "git -C /repo push origin main",
            "git push -u origin feat/x main",
            "git push --force-with-lease origin 'HEAD:main'",
            "git push --delete origin main",
            "cd - && git push origin main",
            "cd /nowhere && git push origin main",
        ] {
            assert_eq!(on("feat/x", command), main, "{command}");
        }
    }

    #[test]
    fn a_push_from_the_default_branch_is_caught() {
        let main = Some(Hit::Push("main".into()));
        for command in [
            "git push",
            "git push origin",
            "git push -u origin HEAD",
            "git push origin @",
        ] {
            assert_eq!(on("main", command), main, "{command}");
        }
        // Following a `cd` or `-C` into a checkout of the default branch.
        assert_eq!(on("feat/x", "cd /other && git push"), main);
        assert_eq!(on("feat/x", "git -C /other push"), main);
    }

    #[test]
    fn everything_else_passes() {
        for command in [
            "git push",
            "git push origin feat/x",
            "git push -u origin HEAD",
            "git push origin feat/main-thing",
            "git push origin main-fix:main-fix",
            "git push --tags",
            "git push -o main origin feat/x",
            "gh pr view 81",
            "gh pr create --title 'merge guard'",
            "gh pr list --search 'is:merged'",
            "gh api repos/o/r/pulls/81",
            "gh api repos/o/r/pulls/81/merge-check",
            "git merge main",
            "git log origin/main..HEAD",
            "echo 'gh pr merge 81'",
            "grep -rn \"git push origin main\" docs",
            "cd /other && git status",
            "gh pr merge 81 --disable-auto",
            "gh api -X GET repos/o/r/pulls/81/merge",
            "git status # then gh pr merge 81",
            "cd /nowhere && git push origin feat/x",
        ] {
            assert_eq!(on("feat/x", command), None, "{command}");
        }
    }

    #[test]
    fn split_reads_quotes_and_operators() {
        assert_eq!(
            split("a 'b c' \"d \\\" e\" f\\ g && h | i; (j)\nk"),
            vec![
                vec!["a", "b c", "d \" e", "f g"],
                vec!["h"],
                vec!["i"],
                vec!["j"],
                vec!["k"],
            ]
        );
    }
}

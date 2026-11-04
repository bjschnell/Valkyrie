//! Learning a repository's facts from what runs in it (ADR-0016 §3): its `checks`,
//! the commands an agent ran before a push that went through, and `ci: agent`,
//! when agents check CI with their own tools after pushing. Each is proposed for
//! the user once seen in two sessions, with the evidence. A repository's files
//! only recognise a command as a check; nothing is drafted that wasn't run.
//! This is the pure part; the daemon watches the tool calls.

use crate::policy;
use std::collections::HashSet;
use std::path::Path;
use valkyrie_proto::{Decision, DecisionKind, DecisionStatus};

/// Sessions a fact is seen in before it's offered.
pub const SESSIONS: usize = 2;
/// How long a sighting counts.
pub const FACT_DAYS: u64 = 90;
/// A rejected fact isn't offered again for this long (ADR-0013 §4).
pub const REJECTED_DAYS: u64 = 30;
/// Most checks remembered between two pushes.
pub const MAX_RAN: usize = 20;
const MAX_COMMAND: usize = 200;

/// Tools whose run is a check whatever its arguments, by the first word (or two).
const TOOLS: &[&str] = &[
    "cargo fmt",
    "cargo clippy",
    "cargo test",
    "cargo nextest",
    "cargo check",
    "pytest",
    "python -m pytest",
    "python3 -m pytest",
    "go test",
    "go vet",
    "golangci-lint",
    "ruff",
    "mypy",
    "eslint",
    "tsc",
    "prettier --check",
    "tox",
    "shellcheck",
    "mvn test",
    "mvn verify",
    "./gradlew check",
    "./gradlew test",
    "gradle check",
    "gradle test",
    "npm test",
    "pnpm test",
    "yarn test",
    "bun test",
    "npx tsc",
    "npx eslint",
    "npx vitest",
];

/// The checks in a shell command run in `checkout`: each `&&`/`;` part that is a
/// check, without its output plumbing (`| tail`, `2>&1`, `> file`).
pub fn checks_in(command: &str, checkout: &Path) -> Vec<String> {
    command
        .split(['\n', ';'])
        .flat_map(|part| part.split("&&"))
        .filter_map(|part| {
            let part = plain(part)?;
            is_check(&part, checkout).then_some(part)
        })
        .collect()
}

/// A part of a command with its plumbing cut off and its spacing normalised;
/// `None` when what's left is nothing or too long to be one check.
fn plain(part: &str) -> Option<String> {
    let mut words = Vec::new();
    for word in part.split_whitespace() {
        if word == "|"
            || word == "||"
            || word.starts_with('>')
            || word.starts_with("2>")
            || word.starts_with("1>")
            || word.starts_with("&>")
        {
            break;
        }
        // Leading variables (`RUST_LOG=x cargo test`) aren't the command.
        if words.is_empty() && word.contains('=') && !word.starts_with('-') {
            continue;
        }
        words.push(word);
    }
    let text = words.join(" ");
    (!text.is_empty() && text.len() <= MAX_COMMAND).then_some(text)
}

fn is_check(part: &str, checkout: &Path) -> bool {
    let words: Vec<&str> = part.split_whitespace().collect();
    let starts = |prefix: &str| {
        let p: Vec<&str> = prefix.split_whitespace().collect();
        words.len() >= p.len() && words[..p.len()] == p[..]
    };
    if TOOLS.iter().any(|t| starts(t)) {
        return true;
    }
    match words.as_slice() {
        // A package script, as the repository names it.
        [pm, "run", script, ..] if matches!(*pm, "npm" | "pnpm" | "yarn" | "bun") => {
            package_script(checkout, script) && checky(script)
        }
        [pm, script, ..] if matches!(*pm, "pnpm" | "yarn") => {
            package_script(checkout, script) && checky(script)
        }
        // A make or just target the repository has.
        ["make", target, ..] => {
            !target.starts_with('-') && checky(target) && file_has(checkout, "Makefile", target)
        }
        ["just", recipe, ..] => checky(recipe) && file_has(checkout, "justfile", recipe),
        // A script of the repository's own.
        [script, ..] => {
            let path = script.trim_start_matches("./");
            path.contains('/')
                && !path.starts_with('/')
                && !path.contains("..")
                && checkout.join(path).is_file()
                && checky(path.rsplit('/').next().unwrap_or(path))
        }
        _ => false,
    }
}

/// Whether a script or target's name says it checks: tests, lints, formats.
fn checky(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "test",
        "check",
        "lint",
        "fmt",
        "format",
        "clippy",
        "verify",
        "typecheck",
        "ci",
    ]
    .iter()
    .any(|w| name.contains(w))
}

fn package_script(checkout: &Path, script: &str) -> bool {
    std::fs::read_to_string(checkout.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .is_some_and(|v| v["scripts"].get(script).is_some())
}

fn file_has(checkout: &Path, file: &str, target: &str) -> bool {
    std::fs::read_to_string(checkout.join(file)).is_ok_and(|t| {
        t.lines()
            .any(|l| l.split(':').next().is_some_and(|n| n.trim() == target) && l.contains(':'))
    })
}

/// Whether a shell command pushed a branch.
pub fn is_push(command: &str) -> bool {
    command
        .split(['\n', ';'])
        .flat_map(|p| p.split("&&"))
        .any(|part| {
            let words: Vec<&str> = part.split_whitespace().collect();
            (words.first() == Some(&"git"))
                .then_some(0)
                .is_some_and(|i| {
                    // `git -C dir push`, `git push -u origin x`.
                    let rest = &words[i + 1..];
                    let verb = rest.iter().enumerate().find(|(j, w)| {
                        !w.starts_with('-') && (*j == 0 || !matches!(rest[j - 1], "-C" | "-c"))
                    });
                    verb.is_some_and(|(_, w)| *w == "push")
                        && !rest
                            .iter()
                            .any(|w| matches!(*w, "--dry-run" | "-n" | "--delete" | "-d"))
                })
        })
}

/// Words that say a call is about CI builds.
const CI_WORDS: &[&str] = &[
    "bamboo",
    "jenkins",
    "buildkite",
    "circleci",
    "teamcity",
    "azure-pipelines",
    "pipelines",
    "pipeline",
    "build_status",
    "buildstatus",
    "builds",
];

/// Whether a tool call checks CI with the agent's own tools (ADR-0016 §3): an MCP
/// tool or a web request about builds. `gh` is left out: a PR's checks on GitHub
/// are `ci: gh`, read by code.
pub fn is_ci_check(tool: &str, input: &serde_json::Value) -> bool {
    let about = |text: &str| {
        let text = text.to_ascii_lowercase();
        CI_WORDS.iter().any(|w| text.contains(w))
    };
    if let Some(server_tool) = tool.strip_prefix("mcp__") {
        return about(server_tool) || about(&input.to_string());
    }
    let Some(command) = shell_command(input) else {
        return false;
    };
    let words: Vec<&str> = command.split_whitespace().collect();
    let web = words
        .first()
        .is_some_and(|w| matches!(*w, "curl" | "wget" | "http" | "xh"));
    let cli = words
        .first()
        .is_some_and(|w| matches!(*w, "bamboo" | "jenkins" | "circleci" | "buildkite"));
    cli || (web && about(&command))
}

/// The shell command in a tool call's input: Claude's `command` string, or Codex's
/// argument list.
pub fn shell_command(input: &serde_json::Value) -> Option<String> {
    match &input["command"] {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(a) => {
            let words: Vec<&str> = a.iter().filter_map(|w| w.as_str()).collect();
            // `bash -lc "<script>"`: the script is the command.
            match words.as_slice() {
                [_, flag, script] if flag.starts_with('-') && flag.ends_with('c') => {
                    Some((*script).to_owned())
                }
                _ => Some(words.join(" ")),
            }
        }
        _ => None,
    }
}

/// What was seen (`facts.jsonl` in the project's store).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen {
    /// A push that went through, and the checks run before it in the session.
    Push { checks: Vec<String> },
    /// CI checked with the agent's own tools, or the user asking for it: what.
    Ci { how: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    pub at: u64,
    pub session: String,
    pub seen: Seen,
}

impl Sighting {
    pub fn to_json(&self) -> serde_json::Value {
        match &self.seen {
            Seen::Push { checks } => serde_json::json!({
                "at": self.at, "session": self.session, "push": checks,
            }),
            Seen::Ci { how } => serde_json::json!({
                "at": self.at, "session": self.session, "ci": how,
            }),
        }
    }

    pub fn from_json(v: &serde_json::Value) -> Option<Sighting> {
        let seen = if let Some(a) = v["push"].as_array() {
            Seen::Push {
                checks: a
                    .iter()
                    .filter_map(|c| Some(c.as_str()?.to_owned()))
                    .collect(),
            }
        } else {
            Seen::Ci {
                how: v["ci"].as_str()?.to_owned(),
            }
        };
        Some(Sighting {
            at: v["at"].as_u64()?,
            session: v["session"].as_str()?.to_owned(),
            seen,
        })
    }
}

/// A fact to propose: the settings line and the evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub title: String,
    pub body: String,
}

/// What the sightings make worth offering for a project, given its decisions:
/// `checks` run before pushes in two sessions, and `ci: agent` seen in two. A
/// setting the project already has, waits on the user, or was turned down lately
/// isn't offered.
pub fn offers(sightings: &[Sighting], decisions: &[Decision], now: u64) -> Vec<Offer> {
    let recent: Vec<&Sighting> = sightings
        .iter()
        .filter(|s| now.saturating_sub(s.at) <= FACT_DAYS * 86_400)
        .collect();
    let mut out = Vec::new();
    if open(decisions, now, |s| s.checks.is_some()) {
        // Each command, in the order first run, with the sessions it ran in.
        let mut order: Vec<(&str, HashSet<&str>)> = Vec::new();
        for s in &recent {
            if let Seen::Push { checks } = &s.seen {
                for c in checks {
                    match order.iter_mut().find(|(o, _)| o == c) {
                        Some((_, sessions)) => {
                            sessions.insert(&s.session);
                        }
                        None => order.push((c, HashSet::from([s.session.as_str()]))),
                    }
                }
            }
        }
        let learned: Vec<(&str, usize)> = order
            .iter()
            .filter(|(_, s)| s.len() >= SESSIONS)
            .map(|(c, s)| (*c, s.len()))
            .collect();
        if !learned.is_empty() {
            let checks: Vec<&str> = learned.iter().map(|(c, _)| *c).collect();
            let mut body = format!(
                "template: ship\nchecks: {}\n\n> Run before pushes that went through:\n",
                checks.join(" && ")
            );
            for (c, n) in &learned {
                body.push_str(&format!("> `{c}` ({n} sessions)\n"));
            }
            out.push(Offer {
                title: "Run these checks before shipping".into(),
                body,
            });
        }
    }
    if open(decisions, now, |s| s.ci.is_some()) {
        let mut sessions: Vec<(&str, &str)> = Vec::new();
        for s in &recent {
            if let Seen::Ci { how } = &s.seen
                && !sessions.iter().any(|(n, _)| *n == s.session)
            {
                sessions.push((&s.session, how));
            }
        }
        if sessions.len() >= SESSIONS {
            let mut body =
                "template: ship\nci: agent\n\n> CI was checked with the agent's own tools:\n"
                    .to_owned();
            for (_, how) in sessions.iter().take(5) {
                body.push_str(&format!("> {how}\n"));
            }
            out.push(Offer {
                title: "Check CI with your own tools after pushing".into(),
                body,
            });
        }
    }
    out
}

/// Whether a setting is still open to offer: no active, waiting, or lately
/// rejected policy sets it.
fn open(decisions: &[Decision], now: u64, sets: impl Fn(&policy::Settings) -> bool) -> bool {
    !decisions
        .iter()
        .filter(|d| d.kind == DecisionKind::Policy)
        .filter(|d| policy::parse(&d.body).is_ok_and(|s| sets(&s)))
        .any(|d| match d.status {
            DecisionStatus::Active | DecisionStatus::Proposed => true,
            DecisionStatus::Rejected => now.saturating_sub(d.updated) < REJECTED_DAYS * 86_400,
            _ => false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("valk-facts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"lint":"eslint .","build":"vite build","test:unit":"vitest"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("Makefile"),
            "check: lint\n\tcargo test\ndeploy:\n\t./ship\n",
        )
        .unwrap();
        std::fs::write(dir.join("scripts/check"), "#!/bin/sh\n").unwrap();
        std::fs::write(dir.join("scripts/release"), "#!/bin/sh\n").unwrap();
        dir
    }

    #[test]
    fn a_check_is_a_known_tool_or_one_the_repository_names() {
        let dir = repo("checks");
        assert_eq!(
            checks_in(
                "cd x && cargo fmt --all --check && cargo clippy -- -D warnings 2>&1 | tail -5",
                &dir
            ),
            ["cargo fmt --all --check", "cargo clippy -- -D warnings"]
        );
        assert_eq!(
            checks_in("RUST_LOG=debug cargo test -q", &dir),
            ["cargo test -q"]
        );
        assert_eq!(
            checks_in("npm run lint; npm run build", &dir),
            ["npm run lint"]
        );
        assert_eq!(checks_in("pnpm test:unit", &dir), ["pnpm test:unit"]);
        assert_eq!(checks_in("make check", &dir), ["make check"]);
        assert!(checks_in("make deploy", &dir).is_empty());
        assert_eq!(
            checks_in("./scripts/check > out.txt", &dir),
            ["./scripts/check"]
        );
        assert!(checks_in("scripts/release", &dir).is_empty());
        // A script the repository doesn't have, or a package script it doesn't.
        assert!(checks_in("scripts/missing-check", &dir).is_empty());
        assert!(checks_in("npm run typecheck", &dir).is_empty());
        assert!(checks_in("ls -la && git status", &dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_push_is_git_push_that_sends_something() {
        assert!(is_push("git push -u origin fix/x"));
        assert!(is_push("cargo test && git push"));
        assert!(is_push("git -C ../wt push origin HEAD"));
        assert!(!is_push("git push --dry-run"));
        assert!(!is_push("git push origin --delete old"));
        assert!(!is_push("git status"));
        assert!(!is_push("echo git push"));
    }

    #[test]
    fn checking_ci_is_an_mcp_tool_or_a_request_about_builds_but_not_gh() {
        assert!(is_ci_check("mcp__bamboo__get_build", &json!({})));
        assert!(is_ci_check(
            "mcp__atlassian__fetch",
            &json!({"url":"https://ci.x/bamboo/rest/api/latest/result/A-1"})
        ));
        assert!(is_ci_check(
            "Bash",
            &json!({"command":"curl -s https://jenkins.corp/job/app/lastBuild/api/json"})
        ));
        assert!(!is_ci_check(
            "Bash",
            &json!({"command":"gh pr checks 12 --watch"})
        ));
        assert!(!is_ci_check(
            "Bash",
            &json!({"command":"curl https://example.com"})
        ));
        assert!(!is_ci_check("mcp__github__get_issue", &json!({"n":1})));
        assert_eq!(
            shell_command(&json!({"command":["bash","-lc","cargo test && git push"]})),
            Some("cargo test && git push".into())
        );
    }

    fn push(at: u64, session: &str, checks: &[&str]) -> Sighting {
        Sighting {
            at,
            session: session.into(),
            seen: Seen::Push {
                checks: checks.iter().map(|c| c.to_string()).collect(),
            },
        }
    }

    fn ci(at: u64, session: &str) -> Sighting {
        Sighting {
            at,
            session: session.into(),
            seen: Seen::Ci {
                how: "mcp__bamboo__get_build".into(),
            },
        }
    }

    fn policy(status: DecisionStatus, updated: u64, body: &str) -> Decision {
        Decision {
            id: 1,
            project: PathBuf::from("/r"),
            title: "p".into(),
            body: body.into(),
            kind: DecisionKind::Policy,
            status,
            created: updated,
            updated,
            supersedes: None,
            superseded_by: None,
            pinned: false,
            fresh: Default::default(),
            provenance: Default::default(),
        }
    }

    #[test]
    fn checks_run_before_pushes_in_two_sessions_are_offered_in_order() {
        let now = 1_000_000;
        let one = [push(now, "a", &["cargo fmt --check", "cargo test"])];
        assert!(offers(&one, &[], now).is_empty());
        let two = [
            push(now - 9, "a", &["cargo fmt --check", "cargo test"]),
            push(now - 5, "a", &["cargo test -p x"]),
            push(now - 1, "b", &["cargo test", "cargo fmt --check"]),
        ];
        let got = offers(&two, &[], now);
        assert_eq!(got.len(), 1);
        assert!(
            got[0]
                .body
                .starts_with("template: ship\nchecks: cargo fmt --check && cargo test\n"),
            "{}",
            got[0].body
        );
        assert!(policy::check(&got[0].body, false).is_ok());
        // The project has checks already, one waits, or one was turned down lately.
        for status in [
            DecisionStatus::Active,
            DecisionStatus::Proposed,
            DecisionStatus::Rejected,
        ] {
            let has = [policy(status, now - 10, "template: ship\nchecks: make")];
            assert!(offers(&two, &has, now).is_empty(), "{status:?}");
        }
        // A policy setting only `until` leaves checks open.
        let until = [policy(
            DecisionStatus::Active,
            now,
            "template: ship\nuntil: pr",
        )];
        assert_eq!(offers(&two, &until, now).len(), 1);
    }

    #[test]
    fn ci_checked_in_two_sessions_is_offered_as_ci_agent() {
        let now = 1_000_000;
        assert!(offers(&[ci(now, "a"), ci(now, "a")], &[], now).is_empty());
        let got = offers(&[ci(now - 1, "a"), ci(now, "b")], &[], now);
        assert_eq!(got.len(), 1);
        assert!(got[0].body.starts_with("template: ship\nci: agent\n"));
        let gh = [policy(
            DecisionStatus::Active,
            now,
            "template: ship\nci: gh",
        )];
        assert!(offers(&[ci(now - 1, "a"), ci(now, "b")], &gh, now).is_empty());
    }

    #[test]
    fn sightings_round_trip() {
        for s in [push(5, "a", &["make check"]), ci(6, "b")] {
            assert_eq!(Sighting::from_json(&s.to_json()), Some(s));
        }
    }
}

//! The supervisor's flake call (ADR-0019 §2): is a red pull request's failure a
//! known flake, to rerun without waking the agent, or real, for `ci-fix`? Hand
//! rules over signals that can be read decide first; a model is asked only about
//! what they can't place. Pure: the daemon gathers the signals.

use crate::backlog::one_line;

/// What's known about one failed check.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Failure {
    /// As GitHub shows it: "ci / windows".
    pub check: String,
    /// Its GitHub Actions run; `None` for CI outside Actions, which can't be rerun.
    pub run: Option<u64>,
    /// The same check passed on another run of the same commit.
    pub passed_before: bool,
    /// The tail of its failed log (`gh run view --log-failed`), empty if unread.
    pub log: String,
}

/// An open bug in the backlog: its number and its title and body.
pub type Bug = (u32, String);

/// What the rules make of one failed check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Flake(String),
    Real(String),
    Unsure,
}

/// What the rules make of the whole pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Every failed check is a known flake: rerun.
    Rerun(String),
    /// At least one is real, or can't be rerun: wake the agent.
    CiFix(String),
    /// Some aren't placed and none is real: ask the model, or ci-fix without one.
    Unsure,
}

/// Where Rust panicked in `log`: `path:line`, with `/` for separators.
pub fn panics(log: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (at, m) in log.match_indices("panicked at ") {
        let line = log[at + m.len()..].lines().next().unwrap_or("");
        if let Some(at) = location(line)
            && !found.contains(&at)
        {
            found.push(at);
        }
    }
    found
}

/// The first `<file>.rs:<line>` in `text`.
fn location(text: &str) -> Option<String> {
    let mut text = text.replace('\\', "/");
    // An escaped Windows path in a log (`crates\\x`) reads as one separator.
    while text.contains("//") {
        text = text.replace("//", "/");
    }
    let end = text.find(".rs:")?;
    let start = text[..end]
        .rfind(|c: char| c.is_whitespace() || "'\"(,".contains(c))
        .map_or(0, |i| i + 1);
    let digits: String = text[end + 4..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (!digits.is_empty()).then(|| format!("{}.rs:{digits}", &text[start..end]))
}

/// The tests `log` says failed (`test a::b ... FAILED`), by their full path.
pub fn failed_tests(log: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in log.lines() {
        let Some(at) = line.find("test ") else {
            continue;
        };
        let rest = &line[at + 5..];
        if let Some(name) = rest.trim_end().strip_suffix(" ... FAILED") {
            let name = name.trim().to_owned();
            if !name.is_empty() && !name.contains(' ') && !found.contains(&name) {
                found.push(name);
            }
        }
    }
    found
}

/// An open bug that names where `log` failed: a panic's `file:line` (#17 lists
/// them), or a failed test's name, when it's long enough not to match by chance.
pub(crate) fn matching_bug<'a>(log: &str, bugs: &'a [Bug]) -> Option<&'a Bug> {
    let wheres = panics(log);
    let tests: Vec<String> = failed_tests(log)
        .into_iter()
        .map(|t| t.rsplit("::").next().unwrap_or(&t).to_owned())
        .filter(|t| t.len() >= 12)
        .collect();
    bugs.iter().find(|(_, text)| {
        let text = text.replace('\\', "/");
        wheres.iter().any(|w| text.contains(w.as_str()))
            || tests.iter().any(|t| text.contains(t.as_str()))
    })
}

/// The rules for one failed check. `changed`: the files the pull request changes.
pub fn call(f: &Failure, changed: &[String], bugs: &[Bug]) -> Call {
    let Some(_) = f.run else {
        return Call::Real(format!(
            "{} isn't GitHub Actions, so it can't be rerun",
            f.check
        ));
    };
    if let Some((n, _)) = matching_bug(&f.log, bugs) {
        return Call::Flake(format!("{} matches #{n}", f.check));
    }
    if let Some(w) = touches(&f.log, changed) {
        return Call::Real(format!("{} fails at {w}, which this PR changes", f.check));
    }
    if f.passed_before {
        return Call::Flake(format!("{} passed on another run of this commit", f.check));
    }
    Call::Unsure
}

/// The first place `log` panicked in a file the pull request changes.
pub(crate) fn touches(log: &str, changed: &[String]) -> Option<String> {
    panics(log).into_iter().find(|w| {
        let file = w.rsplit_once(':').map_or(w.as_str(), |(f, _)| f);
        changed
            .iter()
            .any(|c| file.ends_with(c.as_str()) || c.ends_with(file))
    })
}

/// The rules over every failed check: all flakes rerun; any real one, ci-fix.
pub fn verdict(calls: &[Call]) -> Verdict {
    if let Some(Call::Real(why)) = calls.iter().find(|c| matches!(c, Call::Real(_))) {
        return Verdict::CiFix(why.clone());
    }
    if !calls.is_empty() && calls.iter().all(|c| matches!(c, Call::Flake(_))) {
        let whys: Vec<&str> = calls
            .iter()
            .filter_map(|c| match c {
                Call::Flake(w) => Some(w.as_str()),
                _ => None,
            })
            .collect();
        return Verdict::Rerun(whys.join("; "));
    }
    Verdict::Unsure
}

/// The model's instructions: one JSON answer, nothing else.
pub const SYSTEM: &str = "You decide whether a pull request's failed CI checks are a flake (a failure \
unrelated to the change: a flaky test, a broken runner, a network error) or a real failure the \
change caused. You get each failed check with the end of its log, the files the pull request \
changes, and the project's open bugs. Answer with one JSON object and nothing else: \
{\"flake\": true|false, \"bug\": <number of the open bug it matches, or null>, \"reason\": \
\"<one short line>\"}. Say flake only when you're fairly sure; when in doubt, it's real.";

/// Longest log tail the model gets per check.
const LOG_CHARS: usize = 3000;

/// The question for the model: the checks the rules couldn't place, the PR's
/// files and the open bugs. `redact` scrubs anything that leaves the machine.
pub fn prompt(
    failures: &[Failure],
    changed: &[String],
    bugs: &[Bug],
    redact: impl Fn(&str) -> String,
) -> String {
    let mut text = String::from("# Failed checks\n");
    for f in failures {
        let log: String = {
            let chars: Vec<char> = f.log.chars().collect();
            chars[chars.len().saturating_sub(LOG_CHARS)..]
                .iter()
                .collect()
        };
        text.push_str(&format!("\n## {}\n```\n{}\n```\n", f.check, redact(&log)));
    }
    text.push_str("\n# Files this pull request changes\n");
    for c in changed.iter().take(60) {
        text.push_str(&format!("- {c}\n"));
    }
    text.push_str("\n# Open bugs\n");
    for (n, body) in bugs.iter().take(30) {
        let body: String = body.chars().take(600).collect();
        text.push_str(&format!("\n## #{n}\n{}\n", redact(&body)));
    }
    text
}

/// The model's answer: `Some((flake, why))`, or `None` when it isn't one. A bug it
/// names that isn't open is dropped from the reason.
pub fn parse_reply(reply: &str, bugs: &[Bug]) -> Option<(bool, String)> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    let v: serde_json::Value = serde_json::from_str(reply.get(start..=end)?).ok()?;
    let flake = v["flake"].as_bool()?;
    let mut why: String = one_line(v["reason"].as_str().unwrap_or(""))
        .trim()
        .chars()
        .take(140)
        .collect();
    if let Some(n) = v["bug"].as_u64()
        && bugs.iter().any(|(b, _)| u64::from(*b) == n)
    {
        why = if why.is_empty() {
            format!("matches #{n}")
        } else {
            format!("matches #{n}: {why}")
        };
    }
    if why.is_empty() {
        why = if flake {
            "looks like a flake"
        } else {
            "looks real"
        }
        .into();
    }
    Some((flake, format!("the model: {why}")))
}

/// `merge-base`'s text (ADR-0019 §2, skill #25): typed into the idle session of a
/// pull request that conflicts with its base.
pub fn merge_base(pr: &str, base: &str, checks: Option<&str>) -> String {
    let number = pr
        .rsplit_once("/pull/")
        .map(|(_, n)| format!("#{}", n.trim_end_matches('/')))
        .unwrap_or_else(|| pr.to_owned());
    let run = match checks {
        Some(c) => format!("run `{c}`"),
        None => "run the project's checks".into(),
    };
    format!(
        "Valkyrie supervisor step merge-base, taken by the user: PR {number} conflicts with \
         `{base}`. Merge `{base}` into your branch (`git fetch` first), resolve the conflicts, \
         {run}, push, and say what you kept from each side. Never rebase a pushed branch, \
         never force-push, and never merge the pull request."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "windows\tTest\t2026-11-04T01:02:03Z test daemon::tests::a_red_or_conflicting_pr_flags_its_idle_session ... FAILED
windows\tTest\t2026-11-04T01:02:03Z thread 'x' panicked at crates\\valkyrie-daemon\\src\\pty.rs:412:9:
windows\tTest\t2026-11-04T01:02:03Z assertion failed";

    fn red(log: &str) -> Failure {
        Failure {
            check: "ci / windows".into(),
            run: Some(7),
            passed_before: false,
            log: log.into(),
        }
    }

    #[test]
    fn reads_panics_and_failed_tests_from_a_log_on_any_runner() {
        assert_eq!(panics(LOG), ["crates/valkyrie-daemon/src/pty.rs:412"]);
        assert_eq!(
            panics("thread 'main' panicked at 'boom', src/lib.rs:3:5"),
            ["src/lib.rs:3"]
        );
        assert_eq!(
            failed_tests(LOG),
            ["daemon::tests::a_red_or_conflicting_pr_flags_its_idle_session"]
        );
        assert!(panics("all good").is_empty());
        // A log that escaped its backslashes.
        assert_eq!(
            panics(r"panicked at crates\\x\\src\\a.rs:7:1:"),
            ["crates/x/src/a.rs:7"]
        );
    }

    #[test]
    fn a_failure_an_open_bug_names_is_a_flake() {
        let bugs = vec![(
            17,
            "Flaky on Windows: crates/valkyrie-daemon/src/pty.rs:412".into(),
        )];
        assert_eq!(
            call(&red(LOG), &[], &bugs),
            Call::Flake("ci / windows matches #17".into())
        );
        // By the failed test's name, too.
        let bugs = vec![(
            9,
            "a_red_or_conflicting_pr_flags_its_idle_session hangs".into(),
        )];
        assert!(matches!(call(&red(LOG), &[], &bugs), Call::Flake(_)));
    }

    #[test]
    fn a_failure_in_what_the_pr_changes_is_real_unless_a_bug_names_it() {
        let changed = vec!["crates/valkyrie-daemon/src/pty.rs".to_owned()];
        assert!(
            matches!(call(&red(LOG), &changed, &[]), Call::Real(w) if w.contains("pty.rs:412"))
        );
        // Even when it passed on this commit before: the diff decides.
        let mut f = red(LOG);
        f.passed_before = true;
        assert!(matches!(call(&f, &changed, &[]), Call::Real(_)));
        assert!(matches!(call(&f, &[], &[]), Call::Flake(w) if w.contains("another run")));
        assert_eq!(call(&red(LOG), &[], &[]), Call::Unsure);
        // Outside Actions there's nothing to rerun.
        let mut f = red(LOG);
        f.run = None;
        assert!(matches!(call(&f, &[], &[]), Call::Real(_)));
    }

    #[test]
    fn every_check_must_be_a_flake_to_rerun() {
        let flake = Call::Flake("a".into());
        let real = Call::Real("b".into());
        assert_eq!(
            verdict(&[flake.clone(), flake.clone()]),
            Verdict::Rerun("a; a".into())
        );
        assert_eq!(verdict(&[flake.clone(), Call::Unsure]), Verdict::Unsure);
        assert_eq!(verdict(&[Call::Unsure, real]), Verdict::CiFix("b".into()));
        assert_eq!(verdict(&[]), Verdict::Unsure);
    }

    #[test]
    fn the_models_answer_is_read_and_its_bug_checked() {
        let bugs = vec![(17, String::new())];
        assert_eq!(
            parse_reply(
                r#"ok {"flake": true, "bug": 17, "reason": "the pty race"}"#,
                &bugs
            ),
            Some((true, "the model: matches #17: the pty race".into()))
        );
        assert_eq!(
            parse_reply(r#"{"flake": false, "bug": 99, "reason": ""}"#, &bugs),
            Some((false, "the model: looks real".into()))
        );
        assert_eq!(parse_reply("no idea", &bugs), None);
    }

    #[test]
    fn merge_base_names_the_pr_base_and_checks() {
        let text = merge_base(
            "https://github.com/o/r/pull/98",
            "main",
            Some("scripts/check"),
        );
        assert!(text.contains("PR #98 conflicts with `main`"), "{text}");
        assert!(text.contains("run `scripts/check`"), "{text}");
        assert!(text.contains("Never rebase a pushed branch"), "{text}");
    }
}

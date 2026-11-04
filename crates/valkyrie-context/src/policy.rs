//! Policies (ADR-0013 §3): a decision of kind `policy` picks a template that
//! Valkyrie's hooks enforce. Its body is settings, one `key: value` a line; the
//! text an agent gets is the template's, written here, with only the user's
//! accepted settings filled in. Settings come from two scopes (ADR-0016 §1): the
//! user's habits, kept once for every project, and each project's own, laid over
//! them.

use valkyrie_proto::{Decision, DecisionKind, DecisionStatus};

/// How far `ship` takes work before an agent may stop: each step includes those
/// before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Until {
    Commit,
    Push,
    Pr,
}

impl Until {
    fn parse(s: &str) -> Option<Until> {
        match s {
            "commit" => Some(Until::Commit),
            "push" => Some(Until::Push),
            "pr" => Some(Until::Pr),
            _ => None,
        }
    }

    /// `commit`, `push` or `pr`.
    pub fn parse_str(s: &str) -> Option<Until> {
        Until::parse(s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Until::Commit => "commit",
            Until::Push => "push",
            Until::Pr => "pr",
        }
    }

    fn steps(self) -> &'static str {
        match self {
            Until::Commit => "commit your work, then stop",
            Until::Push => "commit and push the branch",
            Until::Pr => {
                "commit, push the branch and open a pull request; if you're on a backlog \
                 item, record it with `valk todo take N --pr URL`"
            }
        }
    }
}

/// Where CI runs, as far as Valkyrie can see it (ADR-0016 §3): a repository fact,
/// for the `ci-fix` template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ci {
    /// Checks on the branch's GitHub PR, read with `gh`.
    Gh,
    /// Only the agent can reach it, with its own tools.
    Agent,
    None,
}

impl Ci {
    fn parse(s: &str) -> Option<Ci> {
        match s {
            "gh" => Some(Ci::Gh),
            "agent" => Some(Ci::Agent),
            "none" => Some(Ci::None),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Ci::Gh => "gh",
            Ci::Agent => "agent",
            Ci::None => "none",
        }
    }
}

/// One `ship` policy decision's settings, each optional: the user's habit sets
/// `until`, a repository's facts set `checks` and `ci` (ADR-0016 §1).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    pub until: Option<Until>,
    /// The project's checks, run before anything is committed.
    pub checks: Option<String>,
    pub ci: Option<Ci>,
}

/// The format a policy's body takes, for error messages.
pub const FORMAT: &str = "a policy's body is `template: ship`, then one or more of \
     `until: commit|push|pr`, `checks: <command>` and `ci: gh|agent|none`, one a line";

/// Reads a policy's body. Unknown keys and templates are refused, so a typo can't
/// accept a policy that silently does nothing. Lines starting `>` are evidence
/// quoted for the reviewer (a learned policy's replies), never settings.
pub fn parse(body: &str) -> Result<Settings, String> {
    let (mut template, mut out) = (None, Settings::default());
    for line in body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('>'))
    {
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("{line:?} isn't `key: value`; {FORMAT}"));
        };
        let value = value.trim();
        match key.trim() {
            "template" => template = Some(value.to_owned()),
            "until" => {
                out.until = Some(
                    Until::parse(value)
                        .ok_or_else(|| format!("until is {value:?}, not commit, push or pr"))?,
                )
            }
            "checks" if !value.is_empty() => out.checks = Some(value.to_owned()),
            "checks" => return Err(format!("checks is empty; {FORMAT}")),
            "ci" => {
                out.ci = Some(
                    Ci::parse(value)
                        .ok_or_else(|| format!("ci is {value:?}, not gh, agent or none"))?,
                )
            }
            other => return Err(format!("unknown setting {other:?}; {FORMAT}")),
        }
    }
    match template.as_deref() {
        Some("ship") if out == Settings::default() => Err(format!("no settings; {FORMAT}")),
        Some("ship") => Ok(out),
        Some(other) => Err(format!(
            "unknown template {other:?}; only `ship` exists so far"
        )),
        None => Err(format!("no `template`; {FORMAT}")),
    }
}

/// Reads a policy's body for the scope it's recorded in: the user's habits are
/// how far they ship, never a repository's facts (ADR-0016 §1).
pub fn check(body: &str, user: bool) -> Result<Settings, String> {
    let settings = parse(body)?;
    if user && (settings.checks.is_some() || settings.ci.is_some()) {
        return Err(
            "`checks` and `ci` are facts about a repository: record them in it, not in \
             your habits"
                .into(),
        );
    }
    if user && settings.until.is_none() {
        return Err("a habit says how far you ship: `until: commit|push|pr`".into());
    }
    Ok(settings)
}

/// Which decision a setting came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The user's habit, by its id in the user scope.
    User(u32),
    /// This project's decision.
    Project(u32),
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Source::User(id) => write!(f, "your habit #{id}"),
            Source::Project(id) => write!(f, "this repository's #{id}"),
        }
    }
}

/// The `ship` template as it applies in a project: work left short of `until`
/// doesn't end the turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ship {
    pub until: (Until, Source),
    /// Empty: whatever the project uses.
    pub checks: Option<(String, Source)>,
    pub ci: Option<(Ci, Source)>,
}

impl Ship {
    /// The decisions it's made of, the one setting `until` first.
    pub fn sources(&self) -> Vec<Source> {
        let mut out = vec![self.until.1];
        for s in [self.checks.as_ref().map(|c| c.1), self.ci.map(|c| c.1)]
            .into_iter()
            .flatten()
        {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }

    /// Each setting and where it came from, a line each, for `valk decisions show`.
    pub fn describe(&self) -> String {
        let mut out = format!("until: {}  ({})\n", self.until.0.as_str(), self.until.1);
        match &self.checks {
            Some((c, from)) => out.push_str(&format!("checks: {c}  ({from})\n")),
            None => out.push_str("checks: none recorded (the agent runs what the project uses)\n"),
        }
        match &self.ci {
            Some((Ci::Gh, from)) => out.push_str(&format!(
                "ci: gh  ({from}); ci-fix keeps an agent on a red pull request, {CI_FIX_LIMIT} \
                 times at most\n"
            )),
            Some((c, from)) => out.push_str(&format!("ci: {}  ({from})\n", c.as_str())),
            None => out.push_str("ci: none recorded\n"),
        }
        out
    }
}

/// Every active `ship` setting in a scope, the newest decision setting each.
fn settings(decisions: &[Decision]) -> Vec<(u32, Settings)> {
    let mut found: Vec<(&Decision, Settings)> = decisions
        .iter()
        .filter(|d| d.kind == DecisionKind::Policy && d.status == DecisionStatus::Active)
        .filter_map(|d| Some((d, parse(&d.body).ok()?)))
        .collect();
    found.sort_by_key(|(d, _)| std::cmp::Reverse((d.updated, d.id)));
    found.into_iter().map(|(d, s)| (d.id, s)).collect()
}

/// The `ship` policy in a project (ADR-0016 §1): the user's habit, with the
/// project's settings laid over it, so a project that sets `until` itself still
/// decides. No `until` in either: no policy.
pub fn ship(project: &[Decision], user: &[Decision]) -> Option<Ship> {
    let (project, user) = (settings(project), settings(user));
    let scopes: [Scope; 2] = [(&project, Source::Project), (&user, Source::User)];
    Some(Ship {
        until: pick(&scopes, |s| s.until)?,
        checks: pick(&scopes, |s| s.checks.clone()),
        ci: pick(&scopes, |s| s.ci),
    })
}

/// A scope's settings by decision, newest first, and how to name their source.
type Scope<'a> = (&'a [(u32, Settings)], fn(u32) -> Source);

/// A setting from the first scope with a decision that sets it.
fn pick<T>(scopes: &[Scope], get: impl Fn(&Settings) -> Option<T>) -> Option<(T, Source)> {
    scopes.iter().find_map(|(found, source)| {
        found
            .iter()
            .find_map(|(id, s)| Some((get(s)?, source(*id))))
    })
}

/// How far a stop's work got (`valk pings`' states), as the step it reached.
fn reached(state: &str) -> Option<Option<Until>> {
    match state {
        "uncommitted" => Some(None),
        "unpushed" => Some(Some(Until::Commit)),
        "no-pr" => Some(Some(Until::Push)),
        _ => None,
    }
}

/// Whether a stop in `state` is short of what `ship` asks for.
pub fn short(ship: &Ship, state: &str) -> bool {
    reached(state).is_some_and(|got| got < Some(ship.until.0))
}

/// How a policy's decisions are named to the agent: `#N`, or `your habit #N`.
fn names(sources: &[Source]) -> String {
    let names: Vec<String> = sources
        .iter()
        .map(|s| match s {
            Source::Project(id) => format!("#{id}"),
            user => user.to_string(),
        })
        .collect();
    names.join(" with ")
}

/// What the agent is told when `ship` keeps its turn going.
pub fn reason(ship: &Ship, state: &str, branch: &str) -> String {
    let left = match state {
        "uncommitted" => "has changes that aren't committed",
        "unpushed" => "has commits that aren't pushed",
        _ => "is pushed with no pull request",
    };
    let checks = match &ship.checks {
        None => "Run the project's checks (format, lint, tests: what its CI runs).".to_owned(),
        Some((checks, _)) => format!("Run `{checks}`."),
    };
    format!(
        "Valkyrie policy {names}, accepted by the user: work isn't done until it's \
         shipped to `{until}`. Branch `{branch}` {left}. {checks} If they pass, {steps}. \
         If they fail and you can't fix them, stop and say why. Never merge, and never \
         push to the default branch.",
        names = names(&ship.sources()),
        until = ship.until.0.as_str(),
        steps = ship.until.0.steps(),
    )
}

/// The `ci-fix` template (ADR-0013 §3): how many times it keeps an agent going on
/// one pull request before its stops go through to the user.
pub const CI_FIX_LIMIT: u32 = 2;

/// Where CI runs in a project, whatever says how far to ship: its newest `ci`
/// setting. `None`: nothing recorded, so `ci-fix` is off. Only the project's own
/// decisions count: it's a fact about the repository (ADR-0016 §3), so a stray
/// one in the user scope can't turn `ci-fix` on everywhere.
pub fn ci(project: &[Decision]) -> Option<(Ci, Source)> {
    let project = settings(project);
    pick(&[(&project, Source::Project)], |s| s.ci)
}

/// The project's `checks`, the same way: for `ci-fix` where no `until` is set.
pub fn checks(project: &[Decision]) -> Option<(String, Source)> {
    let project = settings(project);
    pick(&[(&project, Source::Project)], |s| s.checks.clone())
}

/// A failed check on a pull request, named the way GitHub shows it, with the link
/// to its logs (ADR-0013 §3): a check run's `detailsUrl`, or a commit status's
/// `targetUrl` for CI that runs outside GitHub Actions (Jenkins, Bamboo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    /// Empty when the CI gave none.
    pub url: String,
    /// The GitHub Actions run, when the check is one: `gh run view` reads it.
    pub run: Option<u64>,
}

/// Longest list of failed checks an agent is given; the rest are counted.
const MAX_CHECKS: usize = 10;

/// What a pull request whose checks failed tells the agent (`ci: gh`): each
/// failed check with where its logs are, then fix, push and wait, or stop and say
/// why. `block` counts from 1 to [`CI_FIX_LIMIT`].
pub fn ci_fix(
    ci: Source,
    checks: Option<&(String, Source)>,
    pr: &str,
    branch: &str,
    failed: &[Check],
    block: u32,
) -> String {
    let mut sources = vec![ci];
    if let Some((_, from)) = checks
        && *from != ci
    {
        sources.push(*from);
    }
    let number = pr
        .rsplit_once("/pull/")
        .map(|(_, n)| format!("#{}", n.trim_end_matches('/')))
        .unwrap_or_else(|| pr.to_owned());
    let mut list = String::new();
    for c in failed.iter().take(MAX_CHECKS) {
        let name = if c.name.is_empty() {
            "a check"
        } else {
            &c.name
        };
        let how = match (c.run, c.url.is_empty()) {
            (Some(run), _) => format!("`gh run view {run} --log-failed`"),
            (None, false) => format!("{} (not GitHub Actions: open the link)", c.url),
            (None, true) => "no link: run the same check locally".to_owned(),
        };
        list.push_str(&format!("\n- `{name}`: {how}"));
    }
    if failed.len() > MAX_CHECKS {
        list.push_str(&format!("\n- and {} more", failed.len() - MAX_CHECKS));
    }
    let run = match checks {
        Some((checks, _)) => format!("run `{checks}`"),
        None => "run the project's checks".to_owned(),
    };
    format!(
        "Valkyrie policy {names}, accepted by the user: a pull request isn't done while \
         its CI is red. PR {number} (branch `{branch}`) has failed checks:{list}\n\
         Read the failing logs, fix the cause, {run}, push, and wait for the checks \
         (`gh pr checks --watch`). For a check outside GitHub Actions, reach its logs \
         through the link (or its CI's API), or run the same check locally if you \
         can't. If the failure is in code you didn't change (a flaky test, a broken \
         runner), look for it in `valk todo list` and file it with `valk todo add -k \
         bug` if it's new, then rerun the failed jobs once (`gh run rerun <run> \
         --failed`) and wait. If you can't make it pass, stop and say why. Never merge, \
         and never push to the default branch. This is block {block} of {CI_FIX_LIMIT} \
         for this PR; after that your stops go to the user.",
        names = names(&sources),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn reads_ship_settings_and_refuses_anything_else() {
        let s = parse("template: ship\nuntil: pr\nchecks: cargo test\nci: gh\n\n> “ship it”\n")
            .unwrap();
        assert_eq!(
            s,
            Settings {
                until: Some(Until::Pr),
                checks: Some("cargo test".into()),
                ci: Some(Ci::Gh),
            }
        );
        // A repository's facts alone, with no `until`.
        assert_eq!(parse("template: ship\nchecks: make").unwrap().until, None);
        for bad in [
            "",
            "until: pr",
            "template: ship",
            "template: ship\nuntil: deploy",
            "template: ship\nci: jenkins",
            "template: ship\nchecks:",
            "template: merge\nuntil: pr",
            "template: ship\nuntil: pr\nmerge: yes",
            "ship it please",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_habit_is_how_far_you_ship_and_nothing_about_a_repository() {
        assert!(check("template: ship\nuntil: pr", true).is_ok());
        assert!(check("template: ship\nuntil: pr\nchecks: make", true).is_err());
        assert!(check("template: ship\nuntil: pr\nci: gh", true).is_err());
        assert!(check("template: ship\nchecks: make", true).is_err());
        assert!(check("template: ship\nchecks: make", false).is_ok());
    }

    fn policy(id: u32, updated: u64, body: &str) -> Decision {
        Decision {
            id,
            project: PathBuf::from("/r"),
            title: format!("p{id}"),
            body: body.into(),
            kind: DecisionKind::Policy,
            status: DecisionStatus::Active,
            created: updated,
            updated,
            supersedes: None,
            superseded_by: None,
            fresh: Default::default(),
            provenance: Default::default(),
        }
    }

    #[test]
    fn the_projects_settings_lie_over_the_users_habit() {
        let habit = [policy(1, 5, "template: ship\nuntil: pr")];
        let facts = [policy(21, 9, "template: ship\nchecks: cargo test\nci: gh")];
        // A habit alone ships everywhere, with whatever checks the project uses.
        let ship = ship(&[], &habit).unwrap();
        assert_eq!(ship.until, (Until::Pr, Source::User(1)));
        assert_eq!((ship.checks, ship.ci), (None, None));
        // Facts alone aren't a policy: nothing says how far to ship.
        assert_eq!(super::ship(&facts, &[]), None);
        // Both: the habit's until, the repository's facts.
        let both = super::ship(&facts, &habit).unwrap();
        assert_eq!(both.until.1, Source::User(1));
        assert_eq!(
            both.checks,
            Some(("cargo test".into(), Source::Project(21)))
        );
        assert_eq!(both.ci, Some((Ci::Gh, Source::Project(21))));
        assert_eq!(both.sources(), [Source::User(1), Source::Project(21)]);
        assert!(both.describe().contains("until: pr  (your habit #1)"));
        // A project's own until still wins, so an existing policy keeps working.
        let own = [policy(3, 1, "template: ship\nuntil: commit")];
        let mine = super::ship(&own, &habit).unwrap();
        assert_eq!(mine.until, (Until::Commit, Source::Project(3)));
        // Within a scope, the newest decision setting it.
        let newer = [
            facts[0].clone(),
            policy(4, 10, "template: ship\nchecks: make"),
        ];
        let ship = super::ship(&newer, &habit).unwrap();
        assert_eq!(ship.checks, Some(("make".into(), Source::Project(4))));
        assert_eq!(ship.ci, Some((Ci::Gh, Source::Project(21))));
    }

    fn at(until: Until) -> Ship {
        Ship {
            until: (until, Source::Project(1)),
            checks: None,
            ci: None,
        }
    }

    #[test]
    fn stops_short_of_the_step_the_user_chose() {
        let blocked = |until, state| short(&at(until), state);
        // Someone who only ever says "commit it".
        assert!(blocked(Until::Commit, "uncommitted"));
        assert!(!blocked(Until::Commit, "unpushed"));
        assert!(!blocked(Until::Commit, "no-pr"));
        // Someone who wants a pull request every time.
        assert!(blocked(Until::Pr, "uncommitted"));
        assert!(blocked(Until::Pr, "unpushed"));
        assert!(blocked(Until::Pr, "no-pr"));
        // Never: shipped, nothing to ship, or the CI's business.
        for state in ["clean", "pr-red", ""] {
            assert!(!blocked(Until::Pr, state), "{state}");
        }
        assert!(blocked(Until::Push, "unpushed") && !blocked(Until::Push, "no-pr"));
    }

    #[test]
    fn the_agent_hears_the_template_and_the_accepted_settings_only() {
        let ship = super::ship(
            &[policy(
                7,
                1,
                "template: ship\nuntil: push\nchecks: make check",
            )],
            &[],
        )
        .unwrap();
        let text = reason(&ship, "uncommitted", "fix/x");
        assert!(text.starts_with("Valkyrie policy #7, accepted by the user"));
        assert!(text.contains("Run `make check`."));
        assert!(text.contains("commit and push the branch."));
        assert!(text.contains("Never merge"));
        let habit = super::ship(
            &[policy(7, 1, "template: ship\nchecks: make check")],
            &[policy(2, 1, "template: ship\nuntil: pr")],
        )
        .unwrap();
        assert!(reason(&habit, "no-pr", "b").starts_with("Valkyrie policy your habit #2 with #7,"));
    }

    #[test]
    fn ci_is_read_without_anything_saying_how_far_to_ship() {
        let facts = [policy(39, 9, "template: ship\nchecks: make\nci: gh")];
        assert_eq!(ci(&facts), Some((Ci::Gh, Source::Project(39))));
        assert_eq!(checks(&facts), Some(("make".into(), Source::Project(39))));
        assert_eq!(ci(&[policy(1, 1, "template: ship\nuntil: pr")]), None);
        let off = [policy(40, 10, "template: ship\nci: none")];
        assert_eq!(ci(&[facts[0].clone(), off[0].clone()]).unwrap().0, Ci::None);
    }

    #[test]
    fn ci_fix_names_each_failed_check_and_where_its_logs_are() {
        let failed = [
            Check {
                name: "CI / test".into(),
                url: "https://github.com/o/r/actions/runs/7/job/8".into(),
                run: Some(7),
            },
            Check {
                name: "bamboo/PLAN".into(),
                url: "https://bamboo.example.com/browse/PLAN-1".into(),
                run: None,
            },
            Check {
                name: String::new(),
                url: String::new(),
                run: None,
            },
        ];
        let checks = ("make check".to_owned(), Source::Project(39));
        let text = ci_fix(
            Source::Project(39),
            Some(&checks),
            "https://github.com/o/r/pull/93",
            "fix/x",
            &failed,
            1,
        );
        assert!(
            text.starts_with("Valkyrie policy #39, accepted by the user"),
            "{text}"
        );
        assert!(text.contains("PR #93 (branch `fix/x`)"), "{text}");
        assert!(
            text.contains("- `CI / test`: `gh run view 7 --log-failed`"),
            "{text}"
        );
        assert!(
            text.contains("- `bamboo/PLAN`: https://bamboo.example.com/browse/PLAN-1 (not GitHub"),
            "{text}"
        );
        assert!(text.contains("- `a check`: no link"), "{text}");
        assert!(text.contains("run `make check`, push"), "{text}");
        assert!(text.contains("gh run rerun <run> --failed"), "{text}");
        assert!(text.contains("Never merge"), "{text}");
        assert!(text.contains("block 1 of 2"), "{text}");
        // Many failures: the first few, and a count.
        let many = vec![failed[0].clone(); MAX_CHECKS + 3];
        let text = ci_fix(Source::User(1), None, "u", "b", &many, 2);
        assert!(text.contains("and 3 more"), "{text}");
        assert!(text.contains("run the project's checks"), "{text}");
    }
}

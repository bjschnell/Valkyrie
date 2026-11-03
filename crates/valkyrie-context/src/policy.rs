//! Policies (ADR-0013 §3): a decision of kind `policy` picks a template that
//! Valkyrie's hooks enforce. Its body is settings, one `key: value` a line; the
//! text an agent gets is the template's, written here, with only the user's
//! accepted settings filled in.

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

/// The `ship` template: work left short of `until` doesn't end the turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ship {
    pub until: Until,
    /// The project's checks, run before anything is committed; empty: whatever the
    /// project uses.
    pub checks: String,
}

/// The format a policy's body takes, for error messages.
pub const FORMAT: &str = "a policy's body is `template: ship`, then `until: commit|push|pr` \
     and optionally `checks: <command>`, one a line";

/// Reads a policy's body. Unknown keys and templates are refused, so a typo can't
/// accept a policy that silently does nothing.
pub fn parse(body: &str) -> Result<Ship, String> {
    let (mut template, mut until, mut checks) = (None, None, String::new());
    for line in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("{line:?} isn't `key: value`; {FORMAT}"));
        };
        let value = value.trim();
        match key.trim() {
            "template" => template = Some(value.to_owned()),
            "until" => {
                until = Some(
                    Until::parse(value)
                        .ok_or_else(|| format!("until is {value:?}, not commit, push or pr"))?,
                )
            }
            "checks" => checks = value.to_owned(),
            other => return Err(format!("unknown setting {other:?}; {FORMAT}")),
        }
    }
    match template.as_deref() {
        Some("ship") => Ok(Ship {
            until: until.ok_or_else(|| format!("no `until`; {FORMAT}"))?,
            checks,
        }),
        Some(other) => Err(format!(
            "unknown template {other:?}; only `ship` exists so far"
        )),
        None => Err(format!("no `template`; {FORMAT}")),
    }
}

/// The project's active `ship` policy, newest first if somehow several are.
pub fn ship(decisions: &[Decision]) -> Option<(u32, Ship)> {
    decisions
        .iter()
        .filter(|d| d.kind == DecisionKind::Policy && d.status == DecisionStatus::Active)
        .filter_map(|d| Some((d, parse(&d.body).ok()?)))
        .max_by_key(|(d, _)| (d.updated, d.id))
        .map(|(d, s)| (d.id, s))
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
    reached(state).is_some_and(|got| got < Some(ship.until))
}

/// What the agent is told when `ship` keeps its turn going.
pub fn reason(id: u32, ship: &Ship, state: &str, branch: &str) -> String {
    let left = match state {
        "uncommitted" => "has changes that aren't committed",
        "unpushed" => "has commits that aren't pushed",
        _ => "is pushed with no pull request",
    };
    let checks = if ship.checks.is_empty() {
        "Run the project's checks (format, lint, tests: what its CI runs).".to_owned()
    } else {
        format!("Run `{}`.", ship.checks)
    };
    format!(
        "Valkyrie policy #{id}, accepted by the user: work isn't done until it's \
         shipped to `{until}`. Branch `{branch}` {left}. {checks} If they pass, {steps}. \
         If they fail and you can't fix them, stop and say why. Never merge, and never \
         push to the default branch.",
        until = ship.until.as_str(),
        steps = ship.until.steps(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_ship_policy_and_refuses_anything_else() {
        let ship = parse("template: ship\nuntil: pr\nchecks: cargo test\n").unwrap();
        assert_eq!(
            (ship.until, ship.checks.as_str()),
            (Until::Pr, "cargo test")
        );
        assert_eq!(parse("template: ship\nuntil: commit").unwrap().checks, "");
        for bad in [
            "",
            "until: pr",
            "template: ship",
            "template: ship\nuntil: deploy",
            "template: merge\nuntil: pr",
            "template: ship\nuntil: pr\nmerge: yes",
            "ship it please",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn stops_short_of_the_step_the_user_chose() {
        let at = |until| Ship {
            until,
            checks: String::new(),
        };
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
        let ship = parse("template: ship\nuntil: push\nchecks: make check").unwrap();
        let text = reason(7, &ship, "uncommitted", "fix/x");
        assert!(text.starts_with("Valkyrie policy #7, accepted by the user"));
        assert!(text.contains("Run `make check`."));
        assert!(text.contains("commit and push the branch."));
        assert!(text.contains("Never merge"));
    }
}

# Policies

A policy is a decision of kind `policy` that picks a template Valkyrie's hooks
enforce ([ADR-0013](adr/0013-learned-policies.md)). The agent gets the
template's fixed text, never your body as prose.

## ship: finishing the job

When a Claude agent is about to end its turn with work short of
where you ship it, the hook keeps it going instead, with fixed text that has it
run your checks and then commit, push, or open a pull request. It never merges
and never pushes the default branch. Its settings live in two places
([ADR-0016](adr/0016-learn-habits-and-repo-facts.md)). How far you ship is your habit, recorded once for every project
with `--user`. Each repository's checks are its own:

```sh
printf '%s\n' 'template: ship' 'until: pr' |
  valk decide --user -k policy "Ship finished work as a pull request" -
printf '%s\n' 'template: ship' 'ci: gh' \
  'checks: cargo fmt --check && cargo clippy && cargo test' |
  valk decide -k policy "Checks to run before shipping" -
```

(The body comes on stdin with `-`, which works the same in bash, zsh and fish.)

## Learned, not written

You rarely need to write the habit yourself. When an agent stops short of
shipping and you answer with a routine "commit it" or "ship it", a small model
(under `valk decisions auto`, short replies only) notes how far you sent the
work, as the next stop shows it got there. After that happens in two
sessions, in any repositories, the habit is proposed for you to accept, with
your replies quoted. Reject it and it isn't offered again for 30 days.

A repository's facts are learned the same way, from what runs in it. The
check commands an agent runs before a push that goes through (a known test or
lint tool, or a script, package script or make target the repository has) are
proposed as its `checks` once two sessions ran them. CI looked up with the
agent's own tools after a push (an MCP tool or a request about builds), or
you asking "is CI green?" after a clean stop with a PR, in two sessions,
proposes `ci: agent`. Nothing here asks a model: it's counting.

## Settings and precedence

`until` is `commit`, `push` or `pr`; `checks` and `ci` (`gh`, `agent` or
`none`) are optional. A repository's settings lie over your habit, and one that
sets `until` itself wins there. `valk decisions show <id>` on a policy prints
what's in effect in the project and where each setting came from; `valk
decisions --user` lists your habits. The template reaches the agent only once
a turn. The queue doesn't show *finished* for a
turn a policy kept going, and `valk pings` counts those stops apart. Retire the
policy, or turn Context off, to stop it.

## ci-fix: staying on a red pull request

With `ci: gh` set for a repository, a Claude agent about to stop with a pull
request whose checks failed is kept going: it reads the failing logs, fixes
them, pushes and waits for the checks. If the failure is in code it didn't
change, it files a bug instead of retrying. This happens at most twice per pull
request; after that the stop goes through and the queue marks it as needing
you ([ADR-0013](adr/0013-learned-policies.md)).

## The merge guard

Merging is yours. When an agent is about to run `gh pr merge` (or the API call
behind it), or push to the default branch, Valkyrie's hook asks you at your own
terminal before it runs; Codex is refused outright. While that confirm is on
screen, input from other agents is dropped, so one agent can't approve another's
merge ([ADR-0015](adr/0015-merge-is-the-review.md)).

The [supervisor](supervisor.md) handles what happens after an agent's last stop.

# The supervisor

Policies act while an agent's turn is still open. The supervisor watches the
work after the agent has stopped: pull requests that go red or fall behind,
and backlog items that are ready to start
([ADR-0019](adr/0019-supervisor.md)).

## Red and conflicting pull requests

The daemon checks each session's pull request every five minutes. When one goes
red or conflicts with its base after its agent stopped, the session comes back
to the top of the queue as *blocked*, with the reason ("PR #142 conflicts with
main") and one step offered:

- `merge-base`: the agent merges the base in, resolves conflicts, runs your
  checks and pushes. It never rebases a pushed branch.
- `ci-fix`: the agent reads the failing logs and fixes them.
- `rerun`: when every failed check is a known flake, the failed jobs are rerun
  once, without waking the agent.

Take the same step on the same kind of
flag in two sessions, with no decline of it since, and a policy such as
`template: merge-base` / `on: conflicting` is proposed. Once you accept it, that
step runs without asking, and `valk decisions health` flags it if you keep
prompting those sessions differently before the PR comes back clean. `valk
pings` counts the steps it took.

It watches sessions with no backlog item too: an agent session whose branch has
an open PR you opened is flagged the same way (someone else's PR, checked out
to review, never is). `valk decisions health` compares how often you leave
those flags, and override the steps on them, with flags on backlog items, so
you can tell whether it reaches too far.

## Keeping the backlog moving

When an accepted item nobody's on has every
blocker shipped or dropped, and fewer than `max_running` agent sessions run in
its repository (3 unless `settings.toml` says `max_running = N`; yours count
too), the board offers to start it on its last agent and model: `R` starts it
and `X` leaves it in the TUI, or Start and Leave on the web board. When nothing
is ready, a model is asked once whether the first waiting item really needs its
unfinished blocker. Start two items this way and a `template: launch` / `on:
ready` policy is proposed, and once accepted it starts them on its own.

`valk todo plan` asks for a build order for a repository's open items: what to
do first, why, and the priorities and after-links that make the board show it.
Nothing changes until you accept it, all of it or only some items
(`--accept ID…`). An order is never turned into a policy: priority stays your
call each time.

## Turning it off

```sh
valk supervise off    # no steps offered or taken; the flags still show
valk supervise on
```

Turning Context off in the settings stops only the automatic steps.

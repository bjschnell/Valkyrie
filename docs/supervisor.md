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
- `ci-fix`: the agent reads the failing logs and fixes them. With `ci: gh`
  set for the repository ([ci-fix](policies.md#ci-fix-staying-on-a-red-pull-request)),
  this one isn't offered: the agent is woken with it right away, up to ci-fix's
  two tries per pull request, and offered to you after that.
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

## Which PR to merge first

With several agents working at once, several PRs go up together. After each
poll the daemon checks which of a repository's open PRs would conflict with
each other, by test-merging their branches locally (`git merge-tree`, which
touches neither branch nor any checkout), and the supervisor page lists them
in the order to merge them:

- ready ones first (green, then pending, red), and one already conflicting
  with main last, since it needs main merged in before anything;
- a PR that collides with others goes after them, so only it has to merge
  main in, not all of them;
- then the oldest first.

Each one says whom it collides with: "merge after #236: conflicts on
src/foo.rs; then it needs the base merged in". It only suggests: merging stays
yours.

When you merge one, every PR it was found to collide with is flagged as
conflicting right away, without waiting for GitHub to work it out again or for
the next poll, and offered `merge-base` as above. With the
`merge-base` / `conflicting` policy accepted, its agent is told to merge main
in within seconds of your merge. Once GitHub settles the PR's mergeability its
own answer wins, and the agent pushing a new head ends the prediction.

## When a PR merges

The agent whose PR merged doesn't wait for you to tell it. If it's idle, it gets
a short wrap-up typed in: the PR merged and its item shipped, so leave a handoff
note if anything's left, say what was filed, and stop. If it's mid-turn or
waiting on you, it gets the same text on its next prompt instead. Every other
agent in the repository hears on its next prompt that the base moved, without
being woken. For a PR with no backlog item, only one you opened, and that
Valkyrie saw open before it merged, counts. With the supervisor or Context off,
nothing is typed and the wrap-up waits for the next prompt.

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
call each time. On the web board a waiting order is one line above the cards;
Review opens it on a page of its own, where you untick what to leave and
Accept or Decline. In the TUI's backlog, `O` opens it over the page: `␣` ticks,
`t` ticks or unticks every change, `g`/`G` and PgUp/PgDn move through a long
one, `↩` accepts and `d` declines.

## The supervisor page

Everything the supervisor waits on you for, and what it did, in one place.
Press `s` on the TUI's home screen; the header counts what waits ("3 for the
supervisor: s"). On the web, what it waits on you for is in the Inbox's Needs
you queue with everything else, and the page is History's Supervisor tab (`s`
from the Inbox's keyboard).

- **Waiting on you** (TUI): flagged pull requests with the step offered, ready
  items it offers to start, build orders to review, and step policies it learned
  from your answers. Each is answered right there (`a` takes, `o` the other
  step, `d` leaves or rejects; ↩ on a build order opens it). They still show
  where they always did, on the session and the board, too. On the web a flag is
  on its session's card, and the rest are Decide rows in the Inbox.
- **Watching:** each open pull request of yours the poll read on its last
  pass, each repository's in the order to merge them (numbered when there are
  two or more), with why, whom each collides with, and when it reads them next. One
  that merges, closes, or stops being read (its item shipped, its session
  ended) drops off.
- **What it did:** the last 90 days, newest first: each step you answered, each
  one a policy took without asking (named by the policy, so you can see what it
  does on its own), and each build order you answered.

The page is opened on purpose and closed again; it never stays up beside your
sessions.

## The supervisor session

Some things no fixed step covers: an agent that stopped short of its PR, two
agents whose work meets, or simply "what's everyone doing?". For those you can
open one Claude session that oversees all the others
([ADR-0019 §10](adr/0019-supervisor.md)):

```sh
valk supervise session            # start it (or find the one running); prints its id
valk supervise session --attach   # and open it in the TUI
```

Or press `c` on the TUI's supervisor page, or Session on the web one.

- There's only ever one. Starting it again opens the one already running, and
  only you can start it, not an agent.
- Each time you prompt it, it gets the roster of every live agent session in
  every repository: state, what it's doing, any PR flag and the step on offer.
  Its first answer is a status, and it waits for you before prompting anyone.
- It reads sessions (`valk pane read`, `valk digest`, `valk todo`) and prompts
  them with `valk pane prompt`. The daemon lets a prompt through only when that
  agent is idle or done and the supervisor is on. A working agent, one waiting
  on you, or your own shell refuses its typing.
- Like every agent, it can't merge, ship, accept or drop anything.
- Every prompt it sends shows under "What it did" (`supervisor session to api:
  "…"`) and in the session's event log.

It runs only while you keep it open, so it costs tokens only then. For now it's
Claude only.

## Turning it off

```sh
valk supervise off    # no steps offered or taken; the flags still show
valk supervise on
```

Off also stops the supervisor session from prompting other sessions, and from
being started.

Turning Context off in the settings stops only the automatic steps.

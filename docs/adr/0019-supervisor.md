# ADR-0019: The supervisor watches work after its session stops, proposes the next step, and learns which steps to take on its own

Status: proposed (2026-11-04)
Date: 2026-11-04
Amends: DESIGN §7 (what the supervisor is), [ADR-0013](0013-learned-policies.md) §7 (what it leaves to the supervisor)

## Context
ADR-0013 moved the routine part of an agent's stop into the hooks: a `ship` or `ci-fix` policy keeps the agent going while its turn is still open. It left to a supervisor what a hook can't see: work after its session's last stop, work across sessions, and anything that needs judgement rather than a template. Backlog #69 kept a tally of cases as they came up:

1. **Build order (2026-11-03).** Items filed in a burst depend on each other (#60). Something has to choose what runs next and start it. `valk todo next` now names the first ready item, but nothing acts on it.
2. **CI red after the last stop (2026-11-04).** PR #101's Windows job failed (flake #17) after its session's last stop, and nothing flagged it. #67 fixed the detection: the daemon's 5-minute PR poll (ADR-0015 §2) shows the item's idle session as blocked, "CI red on PR #N". What to do about it is still left to the user. They choose between waking the agent with ci-fix's text, rerunning a known flake, or looking at it themselves.
3. **A pushed PR conflicting after main moved (2026-11-04).** Six merges landed on main in one night, and #98 conflicted until the user noticed. What fixes it is known: send the session to merge main in (skill #25).

All three cases split the same way. **Detecting** the problem is deterministic: a PR's checks, its mergeability, an item's blockers. **Choosing** the response sometimes needs judgement: is this failure a known flake, does this item really need its blocker first. DESIGN §7 imagined a supervisor session that reads the queue and acts. The policy work (ADR-0013, ADR-0016) showed a better order. Code reads the state, a fixed template is the only thing that reaches an agent, the user accepts each automation once, and health measures it afterwards.

## Decision
1. **Watchers detect, in code, and only flag.** Each watcher is daemon code that turns a state it can read into a flag on the session it concerns. A flag shows in the queue as *blocked* with a one-line reason. It never types anything into an agent.
   - **PR red** (#67): failed checks on an item's open PR.
   - **PR conflicting** (this ADR's phase 1): GitHub reports the item's open PR `CONFLICTING` with its base. The flag reads "PR #N conflicts with main", or "PR #N conflicts with main; CI red: <checks>" when both are true. `UNKNOWN` counts as nothing yet, because GitHub works mergeability out lazily after the base moves, so the next poll picks it up.
   - **Item ready** (phase 4): an open item nobody's on whose blockers have all shipped or been dropped.

   Both PR watchers ride the existing 5-minute poll, which already makes one `gh pr view` call per item. They share its rules: only sessions of this daemon boot, only `idle` or `done`, and a flag that was seen isn't raised again until the session goes idle again or the reason changes. A session still showing the poll's own flag takes the new reason (red CI, then main moves), and it shows as news again. A session blocked by anything else keeps what it shows.
2. **The supervisor proposes one step per flag, from a fixed list.** For each flag, the supervisor picks one action. Each is a template with parameters, and no free text reaches an agent (as in ADR-0013 §3):
   - **`merge-base`** (on *conflicting*): types skill #25's instructions into the idle session: "PR #N conflicts with `<base>`. Merge `<base>` into your branch, resolve the conflicts, run `<checks>`, push, and say what you kept from each side. Never rebase a pushed branch."
   - **`ci-fix`** (on *red*): types ci-fix's existing text into the idle session (ADR-0013 §3). Its twice-per-PR limit is shared with the Stop policy's.
   - **`rerun`** (on *red*, when every failed check matches a known flake): `gh run rerun <run> --failed`, once per PR head, without waking the agent. ci-fix's own text already reruns a flake once when the agent is awake. This is the same rerun for an agent that's done, and the two share the once-per-head limit.
   - **`launch`** (on *item ready*): starts the item's last agent and model on it, as the board's Launch does.
   - **`leave`**: does nothing and keeps the flag, for the user.

   The rule that picks the action is code where code can decide: *conflicting* gets `merge-base`, and *ready* gets `launch` in `valk todo next` order. A model is asked only for two calls code can't make:
   - **Flake or real:** whether a failed check matches an open bug in the backlog (by name and log excerpt), which picks `rerun` over `ci-fix`.
   - **Needs its blocker:** whether a ready item that lists a blocker really needs it (DESIGN §8.12 makes waiting advisory).

   It's the extractor's locked-down model, under the same consent (`valk decisions auto`) and hourly limits. With `auto off`, red picks `ci-fix` and the blocker question isn't asked.
3. **Ask, then learn.** At first, every step is a proposal: the flagged session's row in the queue offers the chosen action, and one tap runs it. Declining leaves the flag. Each answer is recorded (`supervisor.jsonl` in the project's state dir, 90 days: trigger, action, accepted or not). After the user has accepted the same action on the same trigger in two different sessions, the supervisor proposes a `policy` for it (the ADR-0013 §4 kind, e.g. `template: merge-base, on: conflicting`), and the user accepts it in the review queue like any proposal. From then on that action runs without asking. Retiring the policy stops it, and `valk decisions health` measures it (ADR-0013 §6). Handled means the next state the watcher reads is clean. Overridden means the user prompted the session differently before it got there.
4. **What it never does.** It doesn't merge, ship, accept, reopen or drop: those stay the user's (ADR-0007, ADR-0015). The merge guard applies to everything it starts. It acts only on a session that is idle (it never interrupts a turn). It launches only accepted items. A proposed item a session filed and took itself (ADR-0018) is flagged and offered steps like any other, since that session is already on it. It doesn't launch more than `max_launched` sessions (default 1) of its own at once.
5. **Every step is logged and can be stopped.** Each step taken, asked or automatic, is a line in the target session's event log (`supervisor`: trigger, action, policy if any) and in `supervisor.jsonl`. `valk pings` counts the steps the supervisor took. Turning off the Context setting stops every automatic step, as it stops policies, and `valk supervise off` stops the supervisor alone.
6. **Sessions without a backlog item stay uncovered for now.** The watchers read PRs from items (`Item.pr`). A session with a PR but no item gets no flag, as ADR-0013 §7 already notes. ADR-0012 already asks ad-hoc work to be filed with `valk todo add --take`. Covering those sessions by reading their branch's PR is phase 5, once phases 1–4 show the work is worth the extra `gh` calls.

## Phases
1. **The conflict watcher** (backlog #69): `gh pr view` also reads `mergeable,baseRefName`, and the poll flags *conflicting* next to *red*.
2. **Proposals in the queue:** a flagged session offers its action (TUI, web). This is a wire change, so `web/src/proto.ts` changes with it (decision #42). The `merge-base`, `ci-fix` and `rerun` templates; the flake call.
3. **Learning:** `supervisor.jsonl`, policy proposals from accepted steps, automatic steps, health.
4. **Dispatch:** the *item ready* watcher, `launch`, `max_launched`, and the blocker call.
5. **Sessions with a PR and no item**, if the numbers ask for it.

## Consequences
- **No standing supervisor session.** No agent sits in a loop reading the queue. It's watchers and a picker in the daemon, plus a few model calls on flags. That's cheaper, can be audited, and can't drift, at the price of DESIGN §7's open-ended "dispatch tasks" being only what the action list names. A case that needs more joins the list as a new template, as policies do.
- **A conflicting PR shows up within five minutes of GitHub working it out**, not when the user happens to look. The poll already makes that call, so the extra JSON fields cost nothing more.
- **Typing into an idle session resumes it**, even when that's only after the user taps. For an agent that already ended its work, that's what the user would have typed. For one the user left idle on purpose, the step stays a proposal until they've accepted it twice.
- **The model's two calls can be wrong.** A wrong flake call reruns a real failure once, and the next poll flags it red again, with `ci-fix` next. A wrong blocker call starts an item too early, and its brief still names the unfinished blocker.

## Open questions
- Should `launch` reuse the item's last agent and model, or the project's default? (Proposed: the last, as the board's Launch does.)
- Should `max_launched` count only the supervisor's launches, or every session in the repository?
- Should a flag that sits unseen for long enough also push a notification (DESIGN §5.4)?

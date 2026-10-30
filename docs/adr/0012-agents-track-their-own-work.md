# ADR-0012: Agents file, take and track their own work in the backlog; the one gate is a launch

Status: accepted (2026-10-30)
Date: 2026-10-30
Amends: [ADR-0007](0007-context-store.md) §4 as it applies to the backlog (DESIGN §8.12)

## Context
The backlog started as the user's list: an agent could write an item down, but it stayed *proposed* until the user accepted it, an agent could take only accepted items, and the session-start block and launch brief told agents to put anything they found "in the user's backlog, not into your change". In practice that restricted agents more than it protected the user. An agent that finds a small bug next to its task should fix it, and an agent working outside the backlog, or starting from one item and fixing a few unscheduled things along the way, should still leave a trail the user can follow and ship. Under the old rules that work was either not done, or done and untracked: a proposed item couldn't be taken, so it never got a branch, a pull request, a lane or notes, and `ship --merged` never saw it.

What ADR-0007 §4 actually protects against is agent-written text reaching an agent as instructions without the user reading it. In the backlog, that happens in one place only: a **launch**, which puts the item's title, body and notes into a prompt the user starts.

## Decision
1. **Agents may file, take and track their own work freely.** `valk todo add --take [--branch B] [--pr URL]` (`ClientMsg::AddItem { take }`, protocol 18) writes an item down and takes it for the caller's session in one step. An agent's item is still `proposed`. The daemon records which session filed it (`Item.filed_by`, with the daemon's boot), and that session may take it again (to add the pull request) while it's proposed. Any other proposed item still needs accepting before anyone takes it. Its card follows the session like any taken item (running, needs you, review, PR open), marked *agent-filed* until accepted or shipped. Its notes are marked as from a take, as before.
2. **The one gate is a launch.** A proposed item can't be launched ("accept it first"), since a launch makes the item's text a prompt the user starts. Its notes never reach a brief, because they're from a take. Nothing else an agent writes to the backlog reaches another agent as instructions.
3. **Shipping stays the user's, and shipping an agent-filed item is the acceptance.** `ship --merged` (dry run and real) also judges proposed items that an agent filed and took, if they have a branch. The confirm list marks each one *agent-filed, not yet accepted* (CLI `--yes`, the TUI's `S S`, the web's two taps). Shipping one moves it straight from proposed to shipped, because the user's confirm is the acceptance. That accepts the work, not the text as a prompt: an item shipped or dropped while still proposed is marked `unaccepted`, and the user's reopen makes it proposed again, not open, so reopening never opens the launch gate. Accepting and shipping stay `vouch`ed: an agent can do neither.
4. **The instructions change with it.** The session-start block and the launch brief tell agents to fix small related things they find, in this work or in a pull request of their own, and to record each with `valk todo add --take`. Plain `valk todo add` is for findings they won't fix now. They're also told never to ship or accept.

## Consequences
- Agents' unplanned work shows on the board as it happens, and lands in the same merged-branch sweep as planned work. The user sees an agent's fix before it ships, but no longer has to accept it before it can be done.
- An agent can file and take as many items as it likes. Review stays the user's at ship time, where the branch and its marks are listed before anything ships.
- The web bridge may not add-and-take (`valk web` itself may run in a session), as it may not take.
- ADR-0007's caller check is unchanged. What moves is the line it guards in the backlog: from "an agent's item waits for acceptance before any work" to "an agent's text waits for acceptance before it becomes a prompt".

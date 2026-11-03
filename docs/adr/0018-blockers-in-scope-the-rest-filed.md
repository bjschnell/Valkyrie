# ADR-0018: Agents fix what blocks their task in the change; the rest is filed for the user to accept

Status: accepted (2026-11-02)
Date: 2026-11-02
Amends: [ADR-0012](0012-agents-track-their-own-work.md) §4 (what the instructions tell agents to do with what they find)

## Context
ADR-0012 told agents to "fix small related things they find, in this work or in a pull request of their own", tracked with `valk todo add --take`, and to file with plain `valk todo add` only what they "won't fix now". That wording pulls two ways:

- **Against the model's default.** Coding agents keep to the task's scope: they fix what blocks the task and mention the rest. That's a sound default. A change set that grows with unrelated fixes is harder to review, and a side pull request the user never asked for is work they didn't choose to have done. Telling agents to fix small related things invites both.
- **Against itself.** "Won't fix now" gave no threshold, so a filed card read as a fix an agent skipped. That made the user worry the backlog was filling with debt that agents should have fixed (backlog #33, a panic with a few-line fix, looked like one).

The backlog should work with the agent's default, not against it: hold what's out of the task's scope, with the user choosing what gets done and nothing stopping an agent from doing it once chosen.

## Decision
1. **Blocking fixes stay in scope.** An agent fixes what it has to in order to deliver its task, as part of that change.
2. **Everything else is filed, not fixed.** Anything else it finds broken stays out of the change and goes in with `valk todo add` (proposed), its body saying what, how to reproduce it, and how much it matters. The instructions say this is the right call, not a fix skipped.
3. **Acceptance is the user's choice of work, and it doesn't stall the agent.** Once the user accepts an item it's open, and the agent that filed it (or any other) takes it with `valk todo take N` and carries on. Telling the filing session that its item was accepted is backlog #53.
4. **`add --take` tracks a session's own work.** A session working without a backlog item (an ad-hoc one) still records its task with `valk todo add --take`, as ADR-0012 §1 allows. It isn't the path for side findings.
5. **Nothing here is enforced.** It's wording in the session-start block and the launch brief. The daemon's rules from ADR-0012 are unchanged.

## Consequences
- A bare agent-filed card means "found, not in this task's scope, needs your yes", not "should have been fixed". It's work waiting on a choice, not debt.
- The backlog fills only as fast as agents find things. What keeps it from turning into debt is the user accepting or dropping proposed cards, and #53 closing the loop back to the agent.
- Pull requests stay the size of their task.
- Skill #17 is revised to match (step 4: file, don't fold in; take it after acceptance).

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
3. **The user's yes in the session is enough for the agent's own finds.** When its task is done, the agent says in a line what it filed. It offers to fix a find there only when the find is worth acting on now (it blocks the user's next step, or it's a real bug they'd want dealt with before moving on); otherwise the find waits on the board and the agent doesn't ask. Until 2026-11-08 it offered every time, and about a quarter of the turn ends that asked the user something were that offer (`valk pings`, backlog #108). Where it does offer, context argues both ways: the session that found it already knows it, which suits a small fix, while a big one deserves a fresh agent in its own worktree. If the user says go ahead, the agent takes its find (`valk todo take N --branch B`) and fixes it on its own branch and pull request. The user doesn't have to go to the board. Shipping it is the acceptance, as ADR-0012 §3 already has it.
4. **The daemon records the filer on every add.** `Item.filed_by` is set for an agent's plain `valk todo add` in a session, not only for `--take`, so the filer's take of its own proposed item (already allowed by ADR-0012 §1) works for finds too. This adds no new exposure. An agent could already file and take in one step with `add --take`; this is the version where it asks first. A card follows its session's lane only once it's taken (`Item::agent_filed` needs a take); filed and untaken, it waits in Proposed.
5. **Other sessions still need the accept.** Another session (another agent, or a new one the user tells to "fix #N") can't take a proposed item it didn't file until the user accepts it. That's ADR-0007's gate, where another agent's unread text would become this agent's instructions. Accepted items are taken by anyone, as before. When the user accepts an item later, for example from the phone, the session that filed it is told on its next prompt if it is still running and nobody has taken the item (backlog #53).
6. **`add --take` tracks a session's own work.** A session working without a backlog item (an ad-hoc one) still records its task with `valk todo add --take`, as ADR-0012 §1 allows. Asked for work, it first checks `valk todo list --json` for an open item that matches and takes that instead.
7. **An agent drops or rewords only its own cards (backlog #54).** Before this, `change_item` let any caller drop any item, and reword any proposed one. So an agent could drop the user's accepted work or another agent's card, and rewrite another agent's proposal before the user read it. The rule: an agent may drop or reword an item only when `filed_by` is its own session (same `boot` and id). Anything else is `vouch`ed: the user does it on the board. Rewording stays limited to proposals, since an accepted item's text is what the user launches. Notes:
   - The daemon can't read the chat, so "the user told me to drop #N" can't be checked. A drop of someone else's card stays a board action. If that proves too strict, the PreToolUse "ask" answer that ADR-0015 §1 uses for merges could relax it later.
   - Accepted and own: allowed. The filer reporting "this turned out not to be a bug" is its own work state. Shipped and own: refused, since shipping was the user's call.
   - A daemon restart changes `boot`, so an agent loses its own cards across one; that fails closed.
   - Covered by `agents_drop_and_reword_only_their_own_items` in `crates/valkyrie/tests/backlog.rs`: one stand-in agent rewords and drops its own cards, another is refused dropping the user's item and dropping or rewording the first agent's proposal.

## Consequences
- A bare agent-filed card means "found, not in this task's scope, needs your yes", not "should have been fixed". It's work waiting on a choice, not debt.
- The common case closes in the terminal: the agent offers, the user says go, and the agent takes it. What keeps the backlog from turning into debt is the user accepting or dropping the cards left over.
- Pull requests stay the size of their task.
- Skill #17 is revised to match (step 4: file, don't fold in; offer it at the end, and take it on the user's go-ahead).

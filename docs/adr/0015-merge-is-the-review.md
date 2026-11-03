# ADR-0015: A merged pull request ships its backlog item, and merging stays the user's

Status: proposed (2026-11-02)
Date: 2026-11-02
Amends: [ADR-0012](0012-agents-track-their-own-work.md) §3 (what counts as the user's confirm)

## Context
The backlog is Valkyrie's work board: like a sprint board, it shows what's open, what's in progress, what's waiting for review and what's done, across every agent working at once. Valkyrie's point is running many agents, and the board is how the user sees what they're all doing. The board isn't where the work gets reviewed. The pull request is. The user's last look at an item comes when the item and its PR are both waiting on them, and merging is what that look decides.

ADR-0012 §3 asks the user to confirm again after that: `valk todo ship --merged` lists the items whose branches were merged, and the user ships them. That second confirm adds nothing to the review, and items pile up in Review after their PRs merge. On 2026-11-02, four items (#26, #34, #35, #36) stayed open after their PRs merged.

The confirm did protect one thing. An agent runs `gh` with the user's own credentials, so GitHub can't tell whether the user merged a PR or an agent ran `gh pr merge`. Today, if an agent merged its own work, the user would still have to confirm before the item shipped. If a merge alone ships the item, that confirm is gone, and merging has to be the user's in fact, not just by instruction. So far agents have only merged when the user told them to, and policy #21 says "never merge". Both are habits, though, not checks.

## Decision
1. **Merging is the user's, enforced in a hook.** Valkyrie's `PreToolUse` hook refuses an agent's command that merges a PR or pushes to the default branch: `gh pr merge`, `gh api` calls to a PR's merge endpoint, and `git push` of the default branch or onto it (`HEAD:main`, `+main`, a push from the default branch with no refspec). The refusal tells the agent that merging is the user's and to say the PR is ready instead. This applies to every agent session, whether or not it has a backlog item. It's a guard against mistakes, not against an agent set on getting around it (a script that calls the API would get through), the same limit as the Stop policy's "never push to the default branch".
   - **When the user asks for the merge: the hook asks, it doesn't refuse (amended 2026-11-02).** The user sometimes tells an agent to merge ("merge 81 please"), and their written go-ahead should be enough without a trip to GitHub. For Claude Code, the hook answers a merge or default-branch push with `permissionDecision: "ask"` rather than `"deny"`, so Claude Code shows the user a confirm at their own terminal. The user approving that prompt is the consent, checked by the harness rather than inferred from prompt text. This replaces the earlier plan of reading the last `UserPromptSubmit` for "merge", which needed the daemon to tell the user's typing from an agent's `valk send` and matched on a word. If the user declines, the agent says the PR is ready instead.
     - For whoever builds it (backlog #40): check that an "ask" still prompts in Claude Code's bypass-permissions and auto-accept modes. If it doesn't, fall back to "deny" in those modes with the "say the PR is ready" reason. A `valk send` that types into a session could answer the confirm, the same hole the prompt-text plan had: refuse `valk send` from an agent caller while the target session shows a permission prompt (its state is already `needs input`), or note it as a known limit. For Codex, which may have no "ask" (backlog #43), keep "deny".
     - #41 (ship on merge) lands after this, never before: until merging is the user's in fact, a merge mustn't ship anything on its own.
2. **A merged PR ships its item, done by the daemon.** For each open or proposed item with a PR (`Item.pr`), the daemon checks the PR's state with `gh pr view` (the code in `work.rs`, with a timeout), at most every five minutes and only while some item's PR is open. When GitHub reports it `MERGED`, the daemon ships the item exactly as `ship` does: worktree and branch removed by the same rules, and a note on the card saying it was shipped because PR <url> merged. A PR that was closed without merging changes nothing; the item stays for the user to drop or relaunch. No agent ships anything: the daemon is doing what the user's merge already decided.
   - An agent-filed item that is still proposed ships as `unaccepted`, as ADR-0012 §3 already does. The merge accepts the work, not the item's text as a prompt, so the launch gate doesn't move.
   - Without `gh`, or for a PR on a host it can't read, nothing ships on its own and `ship --merged` works as before.
3. **Branches merged without a PR keep the confirm.** `ship --merged` still finds items whose branch reached the default branch by a local merge or push, and still asks the user. A local merge leaves no trace of who did it, so it doesn't count as the user's review.
4. **Undo is a reopen.** The card and `valk todo show` say the item shipped because its PR merged. `valk todo reopen` brings it back as before.

## Consequences
- The board stays current with no extra step: an item moves from Review to Done when its PR merges.
- Every agent session loses the ability to merge or push to the default branch unless the user just asked for it. That holds even in sessions with no backlog item, so it also protects repositories that never use the backlog.
- Shipping now has two paths, the daemon's (on a merged PR) and the user's (`ship`, `ship --merged`). They share one function so their rules can't drift.
- Reading PR states costs a `gh` call per open PR every five minutes, and only while PRs are open.
- The merge guard reads command text, so it catches mistakes and the obvious forms, not a determined workaround. That's the same bar the Stop policy sets.

## Open questions
- Can the daemon reliably tell a prompt the user typed from one `valk send` typed into the session (Decision §1)?
- Does Codex's `PreToolUse` hook (0.164.0 here) see shell commands and allow them to be refused? If not, Codex sessions keep the merge habit and policy text only, as with the Stop policy (ADR-0013).
- Should a session that has no item get an item filed for it when its PR merges, so the board also shows ad-hoc work? Not for now: ad-hoc work is already filed with `valk todo add --take` (ADR-0012).

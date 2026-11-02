# ADR-0013: Valkyrie learns your routine replies to agents' stops and, once you accept, answers them through the hooks

Status: proposed (2026-11-01)
Date: 2026-11-01
Amends: [ADR-0011](0011-learning-loops.md) §5 (a new signal: your replies), DESIGN §7 (the supervisor's first jobs move into the hooks)

## Context
The week's trial of the backlog (2026-10-28 to 11-01) answered its question: the backlog became the place work starts from, and GitHub issue import isn't needed. It also showed where the user still gets pulled in for nothing:

- **Shipping.** Over 30 days, 60 of the 114 turn ends that asked the user something offered to commit, push or open a PR. The miner learned decision #11 from those replies ("ship finished work as a PR without asking, once the user approves it or an independent verifier passes it"), and the user accepted it on 10-28. On 11-01, 4 of the 5 sessions started on one backlog bug each still stopped before a PR. Most ended on a statement, not a question ("committed as 6120299… I haven't opened a PR or pushed"), so `valk pings` didn't count them. One said: "I held off because you didn't ask for it."

  Those agents never saw #11. The session-start block ranks patterns last and used to cut whole entries at its budget, and with thirteen active decisions every pattern was cut (fixed in #64). So this trial doesn't show that a rule written as text fails. It shows that text depends on a delivery path nobody was checking. Even delivered, #11 waited on a verifier that nothing started.
- **CI.** PRs #57 and #60 went red and needed the user to tell the agent to fix them. #60's failure was `cargo fmt --check`, which the agent could have run before pushing.

Two lessons:
1. **A rule written as text is advice, and its delivery can silently fail.** ADR-0011 learns well: the miner found the shipping habit. But what it learns is text an agent may never get, and may not follow when it does. Nothing measured either.
2. **A rule one user writes by hand isn't the product.** Everyone using Valkyrie should get fewer trivial stops because Valkyrie watched how they answer them, not because they found the right setting.
3. **How far to go is the user's preference, not Valkyrie's.** This author wants agents to go all the way to a PR. Someone else answers every stop with "commit it" and opens PRs themselves. Both should get their own workflow automatically. Valkyrie shouldn't pick a default for them, or take a stand on public repositories for them.

The user's line for what an agent may decide: anything code, scripts or infrastructure can settle deterministically is settled there. The supervisor (DESIGN §7) handles only what's left.

## Decision
1. **Every stop has a work state, read by code.** On each `Stop`, the daemon reads the session's git state in its directory and labels the stop with one of these states:
   - **`unshipped`:** a branch other than the default one has uncommitted work, or commits not on its upstream, and has no open PR. The state carries how far the work got: `uncommitted`, `unpushed` (committed, not on its upstream), or `no-pr` (pushed, no PR).
   - **`pr-red`:** the branch's PR has failed checks. This needs `gh`. Without it, this state is never set.
   - **`clean`:** anything else, including any stop on the default branch. Nothing is ever pushed to the default branch.

   The item a session took (ADR-0012) supplies its PR when it has one. Otherwise `gh pr view` on the branch does, cached per branch for a minute. `valk pings` counts `unshipped` and `pr-red` stops as asks whatever the reply says, which fixes the undercount above.
2. **Your next prompt is paired with that state.** When the user's next prompt in a session follows a stop that isn't `clean`, the pair is recorded in `replies.jsonl` in the project's state dir, kept for 90 days like `failures.jsonl`.
   - **Classifying the reply:** it's either *routine* for that state ("ship it", "yes", "commit and PR", "fix CI") or *anything else*. A routine reply to `unshipped` also says how far the user sent the work: commit, push, or PR. That's read from the reply ("just commit it") and confirmed by the next stop's state. When the agent committed and the user then pushed or opened the PR themselves, the reply meant commit. The extractor's locked-down model does this, under the same consent (`valk decisions auto`) and hourly limits, on the redacted prompt alone. With `auto off`, nothing is classified and nothing below is proposed.
   - **What is stored:** the state, the classification and a hash of the prompt. The prompt's text is kept only for routine replies, to quote as evidence, clipped to 120 characters.
3. **A `policy` is a decision whose body picks a template.** Code holds a fixed set of templates. A policy names one, plus the few parameters that template takes. Nothing in a policy is free text that reaches an agent. Version 1 has two templates:
   - **`ship`** (on `unshipped`, short of `<until>`): "Run `<checks>`. If they pass, `<steps>`. If they fail and you can't fix them, stop and say why. Never merge." `<until>` is `commit`, `push` or `pr`, and `<steps>` is fixed text for each:
     - **`commit`:** "commit your work, then stop."
     - **`push`:** "commit and push the branch."
     - **`pr`:** "commit, push and open a PR, and record it on your item with `valk todo take N --pr URL`."

     A stop that has already reached `<until>` is `clean` to this policy. `<until>` is learned: it's the furthest step the user's routine replies sent work to, among the replies the proposal quotes. When they disagree, the nearest step wins and the proposal says so. Raising it later is one more routine reply away, because a reply that sends work further than the policy did counts toward a new proposal that supersedes it.
   - **`ci-fix`** (on `pr-red`): "PR #N's checks failed. Read the failing logs (`gh run view --log-failed`), fix it, push, and wait for the checks (`gh pr checks --watch`). If the failure is in code you didn't change, file it with `valk todo add -k bug` and say so instead of retrying." It fires at most twice per PR. After that the stop goes through and the queue marks it as needing you.

   **`<checks>`** is the project's check command, the only free parameter, and it is part of what the user accepts. `<until>` is a choice from a fixed list, not text.
4. **Policies are proposed early, from evidence.** As the user chose on 2026-11-01, an offer comes early rather than after a long wait.
   - **When:** after the second routine reply to the same state in two different sessions, the daemon proposes that state's template.
   - **What the proposal shows:** the replies it quotes, the template's full text, and a `<checks>` the model drafted from the project's CI configuration (`.github/workflows/*`, `.gitlab-ci.yml` and the like). If there is none, it drafts from the commands that passed before the user's last approvals (ADR-0011 §5). The user can edit `<checks>` while accepting.
   - **Review:** it appears in the review queue like any proposal. Agents can propose a policy, and only the user can accept one (ADR-0007).
   - **Limits:** one active policy per template per project. A rejected template isn't offered again in that project for 30 days.
5. **The observer hook enforces it, on `Stop`** (changed in phase 2 from "the context hook": see below). On Claude's main-thread `Stop`, `valk hook claude` reads the project's active `ship` policy and the work state in the hook's `cwd` itself.
   - **When it blocks:** only when a policy matches and the payload's `stop_hook_active` is false. It then prints Claude Code's block output with the template's text, so the agent keeps going instead of stopping. That's one nudge per turn, so it can't loop.
   - **What the daemon hears:** the same Stop, marked `valk_continued` (policy, state, until). The tracker doesn't count it as a turn end, so the queue never shows *finished* for a turn the agent didn't take, no `work` line is written, and the extractor doesn't read a turn that hasn't ended.
   - **Why the observer, not the context hook:** Claude runs a stop's hooks in parallel. With the context hook deciding, the observer's Stop would usually reach the daemon first and the queue would flash *finished* (and sound) before the block took effect, and `pings` would count the stop as a stall. One process that both reports and decides has no race. ADR-0005's "the observer prints nothing" gets one exception: the user's own accepted policy.
   - **Waiting on `gh`:** only for `until: pr` on a pushed branch, at most 3 s (Claude gives the hook 5). No answer lets the stop through.
   - **When it stays quiet:** outside a Valkyrie session, with the Context setting off, on a subagent's stop, or when nothing matches.
   - **Codex:** policies reach Codex only as a title in the decisions block until its `Stop` hook is shown to support blocking.
6. **Every action is logged and measured.**
   - **Log:** each blocked stop is in the session's event log, marked `valk_continued`, and `valk pings` counts them as *kept going by a policy*. (Phase 2 dropped the separate `policy.jsonl`: the event log already has the session, policy, state and time.)
   - **Handled:** the session's next stop was `clean`, or a PR opened, with no prompt from the user in between.
   - **Overridden:** the user interrupted, or answered the block's outcome with something that isn't routine.
   - **Health:** `valk decisions health` reports a policy overridden in three or more sessions, to reword or retire, as ADR-0011 §6 does for decisions.
   - **Stopping it:** retiring the policy (`valk decisions retire N`) stops it, and turning off the Context setting stops all of them.
7. **What this leaves to others.**
   - **Decision #19** (which superseded #11 on 11-01: open a PR once the checks pass, no verifier) stays as text for agents the hooks don't reach. The `ship` policy is the same rule enforced, for any user, at whatever step their replies set. The PR is the review, and merging stays the user's.
   - **The supervisor** (DESIGN §7) is for what a hook can't see: a PR that goes red after its session ended, work across sessions, and anything that needs judgement rather than a template.

## Phases
1. Stop states, and `pings` counting `unshipped` and `pr-red` stops.
2. The `policy` kind, the `ship` template and enforcement on `Stop`. Policies come from `valk decide --kind policy` for now.
3. Reply pairs, classification and early proposals.
4. The `ci-fix` template (`gh`).
5. Measurement in `health`.

[ADR-0016](0016-onboarding-habits-and-repo-facts.md) splits a policy's settings into the user's habits and each repository's facts, and adds `ci: agent`. Its phases 1–2 come before phases 3–4 here.

## Consequences
- **Every stop costs a `git status` and an upstream check** in the session's directory, inside the daemon rather than the hook. A huge repository should make this skip, not slow the hook. The 300 ms limit holds either way.
- **Public repositories.** `ship` with `<until>` at `push` or `pr` sends code to the remote before the user has read it. Valkyrie doesn't decide whether that's fine. It goes only as far as the user's own replies did, and the proposal says when the remote is public.
- **Coverage is only as wide as the templates.** GitLab, Gerrit and repos without PRs get `ship` without PR detection (`unshipped` falls back to "commits not on upstream") and no `ci-fix` until each gets a template. A narrow set code can check was chosen over a model writing rules, which nobody could audit.
- **A misread reply costs a rejected proposal, not an action.** Nothing acts until the user accepts, and the hook acts on state, never on what the agent's text says.
- **Claude Code's `Stop` block is verified** (2.1.316, 2026-11-01). A `Stop` hook printing `{"decision": "block", "reason": …}` kept a `claude -p` session going: the agent followed the reason, and the next `Stop` arrived with `stop_hook_active: true`, so a single nudge can't loop.
- **To verify before phase 2:**
  - Codex's `Stop` hook (0.164.0 here): whether it can block.
  - Whether a session started outside Valkyrie, which has no hooks (DESIGN §7's blind spot), should get a hint to start with `valk new`.

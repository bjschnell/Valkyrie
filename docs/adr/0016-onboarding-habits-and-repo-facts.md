# ADR-0016: Policies split into the user's habits and each repository's facts; Valkyrie asks for what it can't detect

Status: proposed (2026-11-02)
Date: 2026-11-02
Amends: [ADR-0013](0013-learned-policies.md) §3 (where a policy's settings live), §4 (cold start), and the `ci-fix` template

## Context
ADR-0013 phase 2 made a `ship` policy one decision in one project's store. The author's #21 is:

```
template: ship
until: pr
checks: cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

Those settings mix two kinds of thing:
- **`until` is the user's habit.** The author ships to a PR at home and at work, in every repository. Asking for it again in each one is noise.
- **`checks` is a fact about the repository.** It differs per repository and can usually be read from its files.

Because the two are stored together per project, each new repository starts with no policy until the user writes one, or until phase 3 has seen two stops there. The author's work machine shows the cost: the same habit in every work repository, and no policy in any of them.

CI setups vary too. At the author's work, the code is on GitHub and Bamboo builds it. Bamboo posts build status to the PR, so `gh` sees it and `pr-red` works, and agents read the build logs through Bamboo's API (PR #73's links in the `ci-fix` reason cover that). Other teams' CI posts nothing to the forge, and only the agent can reach it, through an MCP server, an API or a CLI. There `pr-red` never fires and `ci-fix` never runs. The author also wrote a skill at work that does the ship-and-watch-CI loop by hand. That's what this ADR and ADR-0013 together should make unnecessary.

Learning alone (ADR-0013 phase 3) is slow at the start: nothing happens until two matching stops. Asking alone goes stale and asks things code could find out. So Valkyrie detects what it can, asks once for what it can't, and keeps learning from replies to correct both.

## Decision
1. **Two scopes for policy settings.**
   - **The user's habits** are stored once, in a user scope next to the projects (`<state>/context/user/decisions`), with the same files, review and `vouch` rules as a project's decisions. Version 1 has one habit: `ship` with `until`. "Ask me each time" is also an answer, and means no `ship` policy.
   - **A repository's facts** stay in its project store, as a `policy` decision with no `until`: `checks` (the commands to run before shipping) and `ci` (below).
   - **How they combine.** For a stop, Valkyrie takes the user's habit, then lays the project's settings over it. A project policy that sets `until` itself (like #21) still overrides the habit, so existing policies keep working. If there's no habit and no project `until`, there's no `ship` policy, as before.
2. **Valkyrie detects a repository's facts, then asks to confirm.**
   - **`checks`** is drafted from the repository's own files, in this order: the `run` steps of its CI configuration that are commands a developer can run locally, and otherwise conventional entry points (`Cargo.toml` gives `cargo fmt --check`, `cargo clippy` and `cargo test`; `package.json` gives its `lint` and `test` scripts; a `Makefile` or `justfile` gives its `check` or `test` target). If nothing is found, the user types the checks or leaves them out.
   - **`ci`** is one of these:
     - `gh`: the branch's PR has checks on GitHub. Valkyrie reads them itself, as ADR-0013's `pr-red` does today. It's detected when `gh pr checks` on the repository's recent PRs finds any checks.
     - `agent`: CI runs somewhere Valkyrie can't read, but the agent can, through an MCP server, an API or a CLI. The user picks this, because it can't be detected.
     - `none`: no CI.
3. **Valkyrie asks in two places, never in the agent's terminal.**
   - **First run.** The first time the TUI or the web app opens, a one-screen question: "When an agent finishes work, how far should it take it?" The answers are commit, push, open a PR, or ask me each time. The answer becomes the user's `ship` habit, already accepted, because the user chose it.
   - **A repository's first session.** When a session starts in a repository that has no facts recorded, the TUI and the web inbox show a card with the drafted `checks` and the detected `ci`, with three actions: accept, edit, or skip. The session doesn't wait for the answer. Until the card is answered, the user's habit applies with no checks. Skipping is remembered, so the card never comes back for that repository.
   - Agents can't answer either question (`vouch`). Both are only shortcuts for writing a decision: `valk decide -k policy` and `valk decisions edit` stay available.
4. **`ci: agent` adds a mode to the `ci-fix` template.** Valkyrie can't read CI's state, so it doesn't try. After a stop whose work state is `clean` with an open PR, and whose branch head was pushed since the last such block, Valkyrie blocks once with: "Check this branch's CI with the tools you use for it in this project. If it fails, fix it, push and check again. If you can't fix it, stop and say why." It never blocks twice for the same head commit, and at most twice per PR, the same cap as `ci-fix` on `gh`. Valkyrie takes the agent's word for the result, which is weaker than the `gh` mode but is what the user does by hand today. `valk pings` counts these blocks as *kept going by a policy*, like the others.
5. **Learning corrects both scopes.** ADR-0013 phase 3 proposes a change to the user's habit when the user's replies keep going further or stopping shorter than it says, across repositories. It proposes a change to a repository's facts when it sees, in that repository, a check the agent ran by hand or a failure the checks missed. It proposes, the user decides, as before.

## Phases
1. The user scope, how the scopes combine, and the first-run question. Split #21 into a habit (`until: pr`) and this repository's facts (`checks`, `ci: gh`).
2. Detecting `checks` and `ci`, and the repository card in the TUI and the web inbox.
3. `ci-fix` with `ci: gh` (backlog #38, with PR #73's links) and with `ci: agent`.
4. ADR-0013 phase 3 (#37), learning into both scopes.

## Consequences
- A new repository gets the user's habit at once, and its checks after one card.
- The author's work repositories need one first-run answer and one card each, instead of a hand-written policy each. They're `ci: gh`, so the work skill that ships and watches CI can be retired once `ci-fix` lands (#38). `ci: agent` is for teams whose CI doesn't report to the forge.
- Two scopes means "where does this setting come from?" has two answers. `valk decisions show` for a project prints the combined policy and says where each setting came from.
- Drafting `checks` from CI configuration can pick steps that need secrets or services. The card shows the draft before anything runs, and a check that fails for reasons outside the code is the first thing phase 3 should learn from.
- `ci: agent` relies on the agent's tools and its honesty about the result. It's capped and logged, and the merge stays the user's (ADR-0015).

## Open questions
- Is there a dependable signal that `ci: agent` is right, such as an MCP server whose name says CI, or the agent itself checking CI after pushes? For now the user picks it on the card.
- Does Codex need anything different for the first-session card? The card belongs to the TUI and the web app, so probably not, but its enforcement waits on #43 either way.

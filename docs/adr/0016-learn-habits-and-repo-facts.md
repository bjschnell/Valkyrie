# ADR-0016: Policy settings split into the user's habits and each repository's facts, all learned from what people do; no onboarding

Status: accepted (2026-11-11); all four phases shipped (#46, #37, #47, #38 and #48)
Date: 2026-11-02
Amends: [ADR-0013](0013-learned-policies.md) §2 and §4 (what is learned, and in which scope), §3 (where a policy's settings live), and the `ci-fix` template

## Context
ADR-0013 phase 2 made a `ship` policy one decision in one project's store. The author's #21 is:

```
template: ship
until: pr
checks: cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

Those settings mix two kinds of thing:
- **`until` is the user's habit.** The author ships to a PR at home and at work, in every repository.
- **`checks` is a fact about the repository.** It differs from one repository to the next.

Because the two are stored together per project, each new repository starts with no policy, and ADR-0013 phase 3 would have to learn the author's habit again in each one, two stops at a time. The author's work machine shows the cost: the same habit in every work repository, and no policy in any of them.

CI setups vary too. At the author's work, the code is on GitHub and Bamboo builds it. Bamboo posts build status to the PR, so `gh` sees it and `pr-red` works, and agents read the build logs through Bamboo's API (PR #73's links in the `ci-fix` reason cover that). Other teams' CI posts nothing to the forge, and only the agent can reach it, through an MCP server, an API or a CLI. There `pr-red` never fires and `ci-fix` never runs. The author also wrote a skill at work that does the ship-and-watch-CI loop by hand. That's what this ADR and ADR-0013 together should make unnecessary.

A first draft of this ADR added onboarding: a question on first run about how far to ship, and a card for each repository's first session. The author turned it down (2026-11-02). Valkyrie should get smarter by watching how each person works, not by asking them to describe it. A configuration is what someone says they do, and it goes stale. Behaviour is what they actually do. A little friction at the start is an acceptable price for that.

## Decision
1. **Two scopes for policy settings.**
   - **The user's habits** are stored once, in a user scope next to the projects (`<state>/context/user/decisions`), with the same files, review and `vouch` rules as a project's decisions. Version 1 has one habit: `ship` with `until`.
   - **A repository's facts** stay in its project store, as a `policy` decision with no `until`: `checks` (the commands to run before shipping) and `ci` (below).
   - **How they combine.** For a stop, Valkyrie takes the user's habit, then lays the project's settings over it. A project policy that sets `until` itself (like #21) still overrides the habit, so existing policies keep working. If there's no habit and no project `until`, there's no `ship` policy, as before.
2. **Habits are learned across every repository.** ADR-0013 phase 3's reply pairs (a stop's work state, the user's next prompt, how far the work then went) count toward the user's habit wherever they happen. Its early offer, after the second matching pair in two sessions, now applies across repositories, not within one. So a new user's first offer comes after their second routine shipping stop anywhere, and every repository after that gets the habit at once.
3. **A repository's facts are learned from what runs in it.**
   - **`checks`:** a command run in the repository, by an agent (`PostToolUse` on its shell tool) or by the user in a Valkyrie shell, before a push that went through, in two or more sessions. Examples are `cargo test`, `npm run lint`, and a `make` target. A command that a failed CI run and the fix after it point to also counts. The repository's files are only used to recognise a command as a check, not to invent one Valkyrie hasn't seen run.
   - **`ci`:**
     - `gh`: set by code, not learned. It applies when the branch's PR has checks on GitHub (`gh pr checks`), and is read as ADR-0013's `pr-red` is today.
     - `agent`: learned when, in two or more sessions, an agent checks CI through its own tools after a push (an MCP tool or API call whose name or arguments are about builds), or the user tells it to after a stop that was `clean` with an open PR ("check the build", "is CI green?").
     - `none`: the default until one of these is seen.
4. **Everything learned is a proposal; there are no questions.** Each habit and fact reaches the user the way decisions do, in the review queue (TUI, web inbox, `valk decisions`), with the evidence: the replies or commands, from which sessions. One tap accepts it. That is ADR-0007's gate and stays the only step the user takes. Nothing asks on first run, and nothing asks in an agent's terminal. `valk decide -k policy` stays available for anyone who wants to write one by hand.
5. **`ci: agent` adds a mode to the `ci-fix` template.** Valkyrie can't read CI's state, so it doesn't try. After a stop whose work state is `clean` with an open PR, and whose branch head was pushed since the last such block, Valkyrie blocks once with: "Check this branch's CI with the tools you use for it in this project. If it fails, fix it, push and check again. If you can't fix it, stop and say why." It never blocks twice for the same head commit, and at most twice per PR, the same cap as `ci-fix` on `gh`. Valkyrie takes the agent's word for the result. That's weaker than the `gh` mode, but it's what the user does by hand today. `valk pings` counts these blocks as *kept going by a policy*, like the others.
6. **Learning keeps correcting.** When replies keep going further or stopping shorter than the habit says, across repositories, Valkyrie proposes a change to the habit. When a repository's CI keeps failing on something its checks don't run, it proposes adding it.

## Phases
1. The user scope and how the scopes combine. Split #21 into a habit (`until: pr`) and this repository's facts (`checks`, `ci: gh`).
2. Reply pairs and the habit proposal, across repositories (ADR-0013 phase 3, #37).
3. Learning `checks` and `ci: agent` from what runs in a repository, as proposals.
4. `ci-fix` with `ci: gh` (#38, with PR #73's links; built 2026-11-03, see ADR-0013 §3); with `ci: agent` (#48, built 2026-11-08).

## Consequences
- No setup. A new user's first couple of routine shipping stops still reach them. After that, the habit holds in every repository.
- A new repository has no `checks` until two sessions have run some before pushing, so its first PRs ship without a local check, as they do today, and CI still runs.
- `checks` is only ever a command Valkyrie saw run, so it can't pick a step that needs secrets or services nobody has locally.
- Two scopes means "where does this setting come from?" has two answers. `valk decisions show` for a project prints the combined policy and says where each setting came from.
- `ci: agent` relies on the agent's tools and its honesty about the result. It's capped and logged, and the merge stays the user's (ADR-0015).

## Rejected
- **Onboarding** (a question on first run, and a card for each repository's first session): faster on day one, but it asks people to describe their workflow instead of watching it. Turned down by the author on 2026-11-02.
- **Drafting `checks` from CI configuration or manifests:** it's a guess, it can pick steps that need secrets, and what's actually run is a better signal.

## Open questions
- What counts as "an MCP tool or API call about builds" for `ci: agent`? Matching names and arguments is crude. The proposal shows the calls it matched, so the user can judge.

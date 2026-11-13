# ADR-0022: Valkyrie is the harness over the agents, not an editor or an agent manager

Status: accepted (2026-11-11), the direction decision #62 records
Date: 2026-11-10

## Context

Valkyrie started as a way to run several agents side by side: one queue of who is waiting, on a terminal and a phone. The README still pitches it that way ("Many agents, one focus").

What has been built since is wider than a multi-agent view:

- Decisions, skills, the context store and agent notes (ADR-0007, 0014, 0017) give an agent the repo's history and the user's rules at the start of a session.
- The backlog and the supervisor (ADR-0012, 0018, 0019) move work along after an agent stops: build order, CI flakes, PRs that conflict.
- Learned policies and habits (ADR-0011, 0013, 0016) tighten the loop from how each person works, without setup.
- The web app, the phone and the desktop window (ADR-0009, 0020) let the user steer from wherever they are.
- Merge is the review (ADR-0015): agents never ship, accept or merge on their own.

None of this depends on how many agents run at once. A single agent gets the same benefit. The agents themselves (Claude Code, Codex, Copilot and the rest) are supplied by their vendors and keep improving. The gap they leave is everything around the agent.

## Decision

1. **Valkyrie is the harness around the agents.** It sits over whichever agent the user runs and adds what the agent lacks: memory of the project and the person, a queue of work and a supervisor for it, learning from behaviour, and reach beyond the terminal. Running many agents is one feature of that, not the product.
2. **Not an editor.** Valkyrie has no code editor, file viewer or diff reviewer of its own, and does not grow toward one. People edit and read code in the tools they already use. This is the same line ADR-0020 draws for the desktop app: needs go into the web app, not into a second UI.
3. **Agent-neutral.** A feature should work for any agent Valkyrie can launch. Where an agent exposes a hook or an integration point (Claude Code's hooks, for example), Valkyrie uses it, but nothing core may depend on one vendor.
4. **Build on what the agents give.** When an agent gains a capability, Valkyrie should use it or get out of its way, not rebuild it. Valkyrie's own work is what an agent can't do for itself: remember across sessions, see across sessions, and act after a session ends.
5. **The user stays the decision-maker.** Agents propose, the user accepts, merges and ships (ADR-0015, ADR-0018). Learning only automates what the user has repeatedly chosen.
6. **How to pitch it.** The README and docs lead with what the harness does that a user can see working (agents that already know the repo, a backlog that runs itself up to the merge, a queue on the phone). They do not lead with "AI IDE" or "control center" language, which promises an editor or a dashboard Valkyrie doesn't have.

## Consequences

- Features are judged by whether they improve the agent's work or the user's control over it, not by whether they add to a multi-agent view. Work that only polishes the multi-agent view ranks below work that improves memory, supervision or learning.
- The README tagline and opening paragraph no longer fit and need rewording. That is a separate change; the wording is the user's call.
- Agent-neutrality costs something: Claude Code gets the deepest integration first, and other agents get the same features later or in a reduced form. Each gap should be filed, not hidden.
- Anything that starts to look like an editor (a file tree, a code pane, inline diffs) is out of scope under this ADR, and a proposal for one needs to amend it.
- The ideas here are not what makes Valkyrie hard to copy. The code is AGPL-3.0 and public. What is specific to a user is the learned data (decisions, habits, policies), and it stays on their machine.

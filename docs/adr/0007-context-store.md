# ADR-0007: Project decisions live in the daemon's state dir, are reviewed in the queue, and reach agents through hooks

Status: accepted (M2, first slice)
Date: 2026-10-08

## Context
DESIGN §6 (the project context layer) left storage open (§12: repo-local vs daemon-local). Its threat model (§11.6) names injected context as a prompt-injection vector, and §6.3 makes human review mandatory. The author uses Valkyrie on work repos, where extra files in every checkout are unwelcome, and runs several agents in worktrees of one repo at once.

## Decision
1. **Storage: the daemon's state dir, export on request.** Each project keeps one markdown file per decision under `<state dir>/context/<name>-<hash>/decisions/NNNN-slug.md`, with a small `key: value` front matter. `valk decisions export` copies the active ones into `<repo>/.valkyrie/decisions/` for anyone who wants them in git. No SQLite yet: a project holds tens of decisions, and FTS arrives with search and extraction.
2. **A project is a git repository, not a checkout.** Every worktree of a repo resolves to the main worktree's root (read from `.git` / `commondir` on disk, no `git` process), so parallel agents in `~/repos/x-feature` share `~/repos/x`'s decisions. Outside git, the directory itself is the project.
3. **The daemon is the only writer.** `valk decide`, the TUI and the web app all send `ClientMsg`s; the daemon writes atomically (temp file + rename) and pushes the pending proposals to queue watchers. Readers that must be fast (the injection hook) read the files directly.
4. **Agents can only propose.** A decision recorded from a session whose program is an agent (by its adapter, which also covers agents started from a shell) is `proposed`, whatever flags it passed. It becomes `active` only when a human accepts it in the TUI, the phone or `valk decisions accept`. Only `active` decisions are ever injected, so nothing an agent (or a file it read) writes reaches another agent's context without a human seeing it.
5. **Lifecycle:** `proposed → active | rejected`, `active → retired`, and accepting a decision that `supersedes` another marks the old one `superseded`. Rejected ones are kept so the same proposal can be recognised later.
6. **Injection for Claude Code: a `SessionStart` hook.** The per-session `--settings` (ADR-0005) gets a second `SessionStart` command, `valk context hook`, which prints `hookSpecificOutput.additionalContext`: the project's active decisions, ranked (constraints first, then newest) and cut to a budget of 6,000 characters (~1.5k tokens), plus one line telling the agent how to propose. It fires on startup, resume, `/clear` and compaction, so the block survives all four. Nothing is written to CLAUDE.md. Like `valk hook`, it never fails and prints nothing when there is nothing to say.

## Consequences
- Sessions spawned before this build keep their old `--settings`, so they get no block until they are restarted (a reboot's restore re-prepares them).
- Codex has no per-session settings; its injection is left for the next slice (via `valk setup codex`).
- Two machines don't share decisions unless they're exported to the repo. That's acceptable for a local-first tool and keeps the work-laptop story clean.
- The hook reads every active decision file on each session start; at tens of files this is well under the 50 ms hook budget.

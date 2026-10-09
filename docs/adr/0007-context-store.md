# ADR-0007: Project decisions live in the daemon's state dir, are reviewed in the queue, and reach agents through hooks

Status: accepted (M2, first slice)
Date: 2026-10-08

## Context
DESIGN §6 (the project context layer) left storage open (§12: repo-local vs daemon-local). Its threat model (§11.6) names injected context as a prompt-injection vector, and §6.3 makes human review mandatory. The author uses Valkyrie on work repos, where extra files in every checkout are unwelcome, and runs several agents in worktrees of one repo at once.

## Decision
1. **Storage: the daemon's state dir, export on request.** Each project keeps one markdown file per decision under `<state dir>/context/<name>-<hash>/decisions/NNNN-slug.md`, with a small `key: value` front matter. `valk decisions export` copies the active ones into `<repo>/.valkyrie/decisions/` for anyone who wants them in git. No SQLite yet: a project holds tens of decisions, and FTS arrives with search and extraction.
2. **A project is a git repository, not a checkout.** Every worktree of a repo resolves to the main worktree's root (read from `.git` / `commondir` on disk, no `git` process), so parallel agents in `~/repos/x-feature` share `~/repos/x`'s decisions. Outside git, the directory itself is the project.
3. **The daemon is the only writer.** `valk decide`, the TUI and the web app all send `ClientMsg`s; the daemon writes atomically (temp file + rename) and pushes the pending proposals to queue watchers. Readers that must be fast (the injection hook) read the files directly.
4. **Agents can only propose.** Only `active` decisions are ever injected, and only a human makes one active, so nothing an agent (or a file it read) writes reaches another agent's context without the user seeing it. The daemon decides who is asking from the connecting process itself, never from what the client claims (`$VALK_SESSION`), and fails closed (`crates/valkyrie-daemon/src/context.rs`):
   - **Pinned:** the peer's pid from `SO_PEERCRED`, checked against its start time on every request, so a fork whose parent left, or a reused pid, counts as unknown.
   - **An agent:** a known agent (`claude`, `codex`) among the peer's ancestors, or a session that runs one (following the foreground), or a session an agent started (`valk new`, a split) or typed into. That mark sticks to the session, so a shell an agent drove can't launder a decision.
   - **Reparented processes** (`(valk decide &)`, `nohup`, `disown`) are found by their controlling terminal, which is the session's PTY.
   - **A human:** a process in a session no agent runs, started or typed into; a process on a terminal of its own; or the `valk web` bridge, whose devices only a human can pair (`valk web pair` and the startup code ask the daemon first, `ClientMsg::Vouch`). Anything else (no pid, no terminal and no session) counts as an agent.
   - Agents may propose; reviewing (accept, reject, retire, edit) and pairing a phone are refused to them.
   - **What else reaches agents unreviewed, and how it's held:**
     - **Sibling notes (DESIGN §6.5):** paths and prompts observed from other sessions. Only files an editing tool changed inside the repository count. Each is one plain line: no control characters, newlines or bidi overrides, and capped. Prompts from sessions an agent drove are left out. The note is framed as observation ("not instructions to you").
     - **MCP:** shows active, rejected, retired and superseded decisions, never proposals.
     - **The extractor's proposals** (`by: valkyrie`) are model text steered by a transcript. They wait on review like any agent's.
     - **The model command** can be replaced by `$VALK_EXTRACTOR` only in debug builds. In a release build an agent that happened to start the daemon could otherwise route every exchange to a command of its own.
     - **`git`** only ever sees a hex commit the daemon read itself, after `--end-of-options`.
   - **What this doesn't stop:** an agent deliberately running arbitrary code as the user. It can detach from its parents *and* open a fresh pseudo-terminal (`setsid script …`), which looks exactly like a human at a terminal. It can also simply edit the files in the state dir. Both are the agent's own sandbox's job: Claude Code's sandbox keeps writes inside the project. The check covers agents using Valkyrie's commands, which is how a prompt-injected or overeager agent would try. `crates/valkyrie/tests/decisions.rs` tries each of those routes.
5. **Lifecycle:** `proposed → active | rejected`, `active → retired`, and accepting a decision that `supersedes` another marks the old one `superseded`. Rejected ones are kept, so extraction doesn't propose the same thing again (`already_known`). Titles and bodies lose control characters and bidi overrides, so the text a reviewer reads in a terminal is the text agents get. Ids are one past the highest file number, so a hand-broken file never has its id given out twice.
6. **Injection for Claude Code: a `SessionStart` hook.** The per-session `--settings` (ADR-0005) gets a second `SessionStart` command, `valk context-hook claude`, which prints `hookSpecificOutput.additionalContext`: the project's active decisions, ranked (constraints first, then newest) and cut to a budget of 6,000 characters (~1.5k tokens), plus one line telling the agent how to propose. It fires on startup, resume, `/clear` and compaction, so the block survives all four. Nothing is written to CLAUDE.md. Like `valk hook`, it never fails. Outside a Valkyrie session it prints nothing. A project with no decisions yet still gets the line on how to propose, so it can start collecting them.

## Consequences
- Sessions spawned before this build keep their old `--settings`, so they get no block until they are restarted (a reboot's restore re-prepares them).
- Codex has no per-session settings, so `valk setup codex` adds the context hook to `~/.codex/hooks.json` beside the observer, on `SessionStart` and `UserPromptSubmit` (Codex 0.159 reads `hookSpecificOutput.additionalContext` from both, as Claude Code does). Its hooks are global, so it prints nothing outside a Valkyrie session. Rerunning `valk setup codex` means trusting the new entry once with `/hooks`.
- Two machines don't share decisions unless they're exported to the repo. That's acceptable for a local-first tool and keeps the work-laptop story clean.
- The hook reads every active decision file on each session start; at tens of files this is well under the 50 ms hook budget.
- Only agents Valkyrie has an adapter for count as agents by name. Another agent CLI (aider, gemini-cli) running in a plain shell session looks like a human there, unless it started or typed into that session through Valkyrie.
- `ClientMsg::Upgrade` predates this ADR and isn't gated: an agent can still re-exec the daemon as a binary it chose. Agents here upgrade the daemon during development, so gating it is the author's call (open).
- The phone app can accept and edit, so pairing is a human act too: `valk web pair`, `valk setup web` and a `valk web` started by an agent don't hand out a code.

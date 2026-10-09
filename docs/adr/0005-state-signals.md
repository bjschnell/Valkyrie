# ADR-0005: Agent state from hooks first, screen heuristics as fallback

Status: accepted (M1)
Date: 2026-10-06

## Context
The attention queue (DESIGN §5) is only as good as state detection. Verified against the docs and the installed builds (Claude Code 2.1.292, codex-cli 0.159.2):
- **Claude Code** accepts per-run settings via `--settings <json>`. They merge at the highest precedence and are treated like user settings: no extra trust prompt in an already-trusted folder. Useful events: `UserPromptSubmit`, `PreToolUse`/`PostToolUse`, `PermissionRequest`, `Notification` (`notification_type`: `permission_prompt`, `idle_prompt`, `agent_needs_input`, `elicitation_dialog`, …), `Stop`, `StopFailure`, `SessionStart`/`SessionEnd`.
- **Codex** has stable hooks with the same event names (plus `Interrupt`). They are configured in `hooks.json` or `[hooks]` in `config.toml`. Every non-managed command hook must be **trusted by hash** via `/hooks`; the only skip is `--dangerously-bypass-hook-trust`, which also runs every *other* untrusted hook.
- Hooks don't fire before the agent is past its folder-trust prompt, and both agents open on one in an untrusted folder.

## Decision
1. **One hook entry point for every agent:** `valk hook <agent>`, a command hook. It reads the event JSON on stdin and forwards it with `$VALK_SESSION` to the daemon socket, then exits 0 at once (observe-only, see 4). Without `$VALK_SESSION` it exits 0 and does nothing, so it's harmless outside Valkyrie. Using a command hook rather than an HTTP one keeps us on the unix socket with no TCP listener.
2. **Claude:** the adapter adds `--settings '{"hooks":{…}}'` at spawn. Nothing in the user's config changes.
3. **Codex:** `valk setup codex` adds one fixed entry to `~/.codex/hooks.json` and the user trusts it once with `/hooks`. The command string never changes, so its hash and therefore the trust stay valid. No bypass flag. *Exception: the rename to Valkyrie (2026-10-07) changed the binary to `valk`. `valk setup codex` replaces the old `overseer` entries, which must be trusted again once. Codex sessions still running under the old daemon stop reporting hooks until restarted under `valk`.*
4. **Observe-only in M1.** Hooks never return a decision. Approve/deny from the queue (a `PermissionRequest` hook waiting on the daemon, with a timeout falling back to the native dialog) comes with M3, alongside phone/web.
5. **Screen heuristics stay**, as the fallback for pre-hook states (folder-trust prompts), agents without hooks, and the stale detector. They're tested against recorded `.raw` transcripts.
6. **Bell and title** (already emitted by `valkyrie-term`) are generic weak signals for any program.
7. **Hook contract** (from the author's Leader plugin and its live findings):
   - The hook **prints nothing**: plain stdout becomes model context on `UserPromptSubmit`/`SessionStart`.
   - It **always exits 0**: exit 2 blocks `PreToolUse`/`UserPromptSubmit`/`Stop`.
   - Each registration sets a small explicit `timeout`.
   - Budget: under 50 ms per call (Leader's Python hook measured a median of 15 ms).
   - The hook waits for the daemon's ack for at most ~25 ms.
   - Every event carries a send timestamp, so the daemon can drop events that arrive out of order.

## Consequences
- Codex needs a one-time setup plus trust. Until then Codex sessions run on heuristics only, and the UI should say so.
- Hook payloads differ slightly between agents (Codex adds `turn_id` and `last_assistant_message` on `Stop`), so each adapter normalizes them into `AgentEvent`.
- Two transitions fire no hook in Claude Code: denying a permission with Esc, and interrupting a turn with Esc. Leader verified this live. Only the screen can catch them (DESIGN §14.3 `interrupted?`). Re-verified live in M1: an Esc-deny shows as `interrupted?` about 2 s after the dialog closes.
- If the daemon is down, the hook fails fast; that never blocks the agent, because it is observe-only.

# Valkyrie — Design Draft v0.1

Name: **Valkyrie** (renamed from Overseer on 2026-10-07); the command is `valk`. Status: **draft for evaluation by Claude Code + Opus**. Nothing here is validated by code yet. Items marked **[VERIFY]** are claims from a quick survey (Oct 2026) that must be checked against primary sources before they drive decisions.

## 1. One-liner

A Rust daemon + TUI + PWA that sits *above* coding-agent CLIs (Claude Code, Codex, others) as a manager of managers. Its two core ideas: an **attention queue** that tells you which agent needs you next and why, and a **project context layer** that carries decisions across sessions and agents so you stop pasting handoff docs.

Standalone, open source, MIT/Apache-2.0 candidate. Success criterion: the author prefers it over herdr for daily use.

## 2. Goals / non-goals

Goals
- Attention-first UX: the primary screen is a ranked inbox, not a grid of sessions.
- Persistent, reviewable, supersedable project decisions with provenance, injected into every agent automatically.
- Zed-class responsiveness: input latency and render speed are product features, not nice-to-haves.
- TUI and web are equal clients of one daemon API.
- Support Claude Code and Codex first, others through a pluggable adapter trait.
- Phase 2: a supervisor agent that drives other agents and only escalates real blockers.

Non-goals
- Not an agent harness. We never implement the model loop, tools, or prompts for coding.
- Not an IDE or editor.
- Not a hosted service. Local-first; remote access is via the user's own network (Tailscale etc.).
- Not a general memory platform; scope is project/dev context.

## 3. Competitive landscape (short) [VERIFY all]

- **Agent of Empires (AoE)**: Rust, MIT, TUI + web + CLI + HTTP API, tmux-backed sessions, worktrees, container sandboxing, status detection, phone-friendly structured view. Closest overlap on the multiplexer + remote half.
- **herdr**: Rust multiplexer/"runtime for agents"; has agent-automation primitives (agents creating/inspecting other panes). Closest overlap for phase 2. Depth unknown.
- **Happy, Omnara (original repo unmaintained), Claude Squad, Conductor, Vibe Kanban, cwt, showrunner**: session/worktree managers or mobile remotes.
- **agentmem, engram, agent-logbook, others**: memory MCPs. agentmem already has lifecycle states (hypothesis→active→validated→deprecated/superseded), conflict + staleness detection, FTS5, markdown canonical files.

Differentiators we lean on:
1. Attention queue as the *primary* surface.
2. Native, manager-controlled decision memory with provenance (session, commit, files, transcript offset).
3. Fleet-wide injection through the manager (one source of truth for standing rules).
4. Manager-of-managers supervisor with the queue as its escalation channel.
5. Owned PTY + terminal-state layer (no tmux dependency) for latency and reliable state detection/phone rendering.

## 4. Architecture

```
            ┌───────────── clients ─────────────┐
            │  TUI (ratatui)   Web/PWA   CLI    │
            └───────────────┬───────────────────┘
                 HTTP + WebSocket (one API)
            ┌───────────────▼───────────────────┐
            │              daemon               │
            │  session mgr · PTY host · VT state│
            │  adapters (claude, codex, acp…)   │
            │  event bus · attention queue      │
            │  context store · MCP server       │
            │  supervisor (phase 2)             │
            └───────────────┬───────────────────┘
                   SQLite (WAL) + transcript files
```

Principles
- **Daemon is the only stateful component.** Clients are thin; the TUI must use the same public API as the web client (no private back channel). Local TUI may use a unix socket with the same protocol for latency.
- **Survives client disconnect.** Sessions live in the daemon, not in a client or tmux.
- **Event-sourced.** Every session emits typed events (state change, prompt shown, tool call, file touched, commit, error). Queue, summaries, and decision extraction are consumers of the event stream.

### 4.1 PTY + terminal state
- `portable-pty` for spawning; `alacritty_terminal` or `vte` (+ own grid) to keep a parsed screen model per session in the daemon. [VERIFY which gives best embed story and scrollback control]
- Web client receives **diffed screen updates** (cell/row diffs), not raw byte streams, with a raw-stream mode for full-fidelity xterm.js view. Phone gets a reflowed/structured view when an adapter provides structure.
- Latency budget (proposal): keystroke→echo p99 < 16 ms local TUI; < network RTT + 10 ms remote. Measure from day one with a built-in benchmark harness.

### 4.2 Agent adapters
Trait roughly:
```
trait AgentAdapter {
    fn spawn(&self, spec: SessionSpec) -> Session;
    fn detect_state(&self, ev: &RawEvents, screen: &Screen) -> AgentState; // fallback
    fn structured(&self) -> Option<StructuredChannel>; // ACP / SDK / hooks
    fn inject_context(&self, ctx: &ContextBundle) -> InjectPlan;
}
```
Tiers of state detection, best first:
1. **Structured**: ACP adapters exist for Claude (Zed's Agent SDK adapter) and Codex (codex-acp / Codex App Server) [VERIFY current status]. Claude Code hooks (Stop, Notification, PreToolUse etc.) and Codex notify/hooks where available.
2. **Hooks/side channels** (hook scripts posting to the daemon socket).
3. **Screen heuristics** over the VT grid (last resort; per-adapter regexes with version pinning and tests against recorded fixtures).

Open question: ACP gives clean structure but may bypass the native TUI the user is used to. Decide whether a session is "PTY-native with hook-based state" (default) or "ACP-structured" (opt-in, better phone UX).

### 4.3 Worktrees / isolation
Optional per-session git worktree; sandbox/container support is **deferred** (AoE has it; not our wedge).

## 5. Attention queue (core differentiator)

### 5.1 States
`working` · `needs_input` (question/permission/choice) · `blocked` (error, stuck, loop) · `review_ready` (done, diff awaiting human) · `idle` · `stale` (no progress for N min while "working").

### 5.2 Item model
```
QueueItem { session, reason: enum, summary: 1–2 lines, ask: Option<Question>,
            urgency, cost_to_defer, age, project, suggested_action }
```
- Summary generated **on state transitions only** (cheap model, bounded input: last N events + diff stat), cached, regenerated if the item is still open and new events arrive.
- Quick actions inline: approve / deny / pick option / send canned reply / "defer 30 min" / "open session".

### 5.3 Ranking (v0 heuristic, tunable)
score = blocking-others weight (other agents/tasks depend on it) + age + explicit priority + cost of idling (agent doing nothing while waiting) − batchability (similar low-risk approvals can be grouped).
- **Batching**: group repeated permission prompts across sessions ("12 agents want to run `cargo test`") into one tap.
- Learned ranking is explicitly out of scope until we have data.

### 5.4 Notifications
Web push for PWA; TUI bell/OS notification. Rate-limited and deduped: queue is the source of truth, notifications only mirror new top-of-queue items. Goal: pings disappear when the supervisor (phase 2) handles things.

**TUI pings (done 2026-10-07, cloned from herdr's sounds):**
- **What fires a ping.** Each queue push is diffed against the previous one by `(session, seq)`. A new transition into `needs_input` or `blocked` plays the `request` sound, and a new transition into `review_ready` plays `done`. The first queue after a connect, or after reconnecting to a restarted daemon, only primes. An upgrade keeps ids and seqs, so pings that happened during it still fire. `interrupted?`/`stale` never ping, because they are guesses.
- **What stays quiet.** No ping for the attached session, or for anything already `seen`, which includes anything another client is attached to. A transition must hold for 1.5 s before it pings. Codex reports a permission request before its auto-reviewer takes it, and that must not ring. A new seq in the same state, which is only a summary update, doesn't ping again. A request still sounds right after a `done`.
- **Sound.** At most one sound per second, with a toast in the bar for 6 s.
  - The sounds are generated two-note chimes, written once to `<state dir>/sounds/{request,done}.wav`. Replace the files to use your own sounds.
  - They play through `pw-play`, then `paplay`, then `aplay`. A player still running after 5 s is killed; herdr's hung for 15 s on a dead audio server.
- **Over SSH** (`SSH_CONNECTION`/`SSH_TTY` set), the TUI rings the outer terminal's bell instead, so the ping reaches the machine you sit at.
- **Mute.** `m` toggles sounds, or the settings panel (`,`); the choice is kept in `settings.toml`, and `VALK_SOUND=on|off` overrides it.
- **Limits.** Pings come from the client, so none sound without a TUI open, and each open TUI pings. Desktop notifications (herdr's `ui.toast` terminal/system delivery) are a follow-up.

### 5.5 herdr logic reuse
Audited 2026-10-06: the author's plugin is **Leader** (`~/repos/leader`, Python stdlib, herdr 0.8.2, Claude Code 2.1.284–285). Its live findings (`docs/findings.md` there) are adopted as M1 inputs; see §14.3–14.4 for what was ported. Semantics are ported, not code. Leader has no batching, so §5.3 batching remains new design.

## 6. Project context layer (core differentiator)

### 6.1 Problem
Repeatedly telling agents "check session X, we decided Y", and pasting large handoff markdowns.

### 6.2 Data model
```
Decision {
  id, project, title, body (short), kind: decision|constraint|pattern|gotcha|fix,
  status: proposed|active|validated|superseded|deprecated|expired,
  supersedes: Option<id>, superseded_by: Option<id>,
  provenance: { session_id, transcript_span, commit, files[], agent, timestamp },
  confidence, scope: global|project|path-glob, ttl/review_by, last_confirmed_at,
  tags, embedding (optional)
}
```
- Lifecycle borrowed from agentmem; we add transcript-span provenance and path-scoped applicability.
- Canonical on disk as markdown files in-repo (`.valkyrie/decisions/*.md`) for diffability and git history; SQLite is an index/cache. Decide whether decisions live in the repo or in the daemon's data dir (repo-local = portable + reviewable; data dir = no repo pollution). [DECISION NEEDED]

### 6.3 Capture
1. Cheap-model extraction run at session end / on commit / on "decision-shaped" events, producing `proposed` decisions only.
2. **Human confirm is mandatory** to become `active`: one-tap accept / edit / reject in the queue (they appear as queue items). No unreviewed auto-memory.
3. Agents can also propose via MCP tool `propose_decision`.

### 6.4 Staleness handling
- **Supersede**: new decision linked to the old; old drops from recall but stays in history.
- **Conflict detection**: on proposal, retrieve nearest active decisions (FTS + optional embeddings) and flag contradictions for review.
- **Expiry/review_by**: decisions about volatile things (versions, endpoints, flags) get short TTLs and resurface for re-confirmation.
- **Code-anchored invalidation**: if files in a decision's scope changed substantially since `last_confirmed_at`, mark `needs_review`.
- **Health score** per project (stale count, conflicts, unreviewed proposals).

### 6.5 Serving to agents
- **MCP server** (stdio + HTTP) exposed to every managed session: `search_decisions`, `get_project_state`, `get_decision`, `propose_decision`, `link_session`, `list_open_questions`.
- **Pinned block**: small, generated, token-budgeted section injected into CLAUDE.md / AGENTS.md (or via `--append-system-prompt`/equivalent) containing only standing constraints. Budget enforced (e.g. ≤ 1.5k tokens) with ranking by scope + recency + validation.
- **Handoff on demand**: `valk handoff <session>` generates a compact structured resume (goal, decisions, open threads, files) instead of pasting giant MDs; also queryable via MCP.
- **Cross-session recall**: "what did we decide about X / where did we fix Y" resolves to a decision with a link to the originating transcript span.

### 6.6 Risks
- Injection bloat eating context → hard token budgets, retrieval over pre-injection.
- Garbage accumulation → mandatory review, health score, TTLs.
- Adapter divergence in how each agent loads instructions → per-adapter `inject_context` with tests.
- Privacy: transcripts and decisions may contain secrets → local-only storage, redaction pass before any external summarizer call, configurable model/provider (local models supported).

## 7. Supervisor agent (phase 2, "meta flow")
- A supervisor session that uses the daemon's API/MCP to: read the queue, answer routine prompts per policy, dispatch tasks, spawn/stop sessions, and escalate to the human queue only on defined conditions.
- **Policy file** defines what it may auto-approve (e.g. read-only commands, test runs, in-worktree edits) and hard stops (destructive ops, secrets, prod, spend).
- Full audit log of every autonomous action; one-tap undo/stop-all.
- Depends on 4.2 structured channel quality and 5 state accuracy. Do not start until phase 1 queue accuracy is measured.
- Check herdr's agent-automation primitives for prior art. [VERIFY]

## 8. Clients

TUI (ratatui): queue is the home screen; split into queue | session view | context pane. Keyboard-first; jump to top queue item in one key.

**Session tabs (done 2026-10-07).** An attached session used to look like a plain terminal, with no sign of the other sessions or how to start one. A right sidebar came first, but it took 30 columns from agents whose diffs and tool output use the width. It became a 2-row tab strip above the session, which costs about 5% of the height instead of 23% of the width:
- **Tabs.** Each tab shows its number and name, then `program · state`, for example `claude · ⠋ working` or `fish · ○ idle`. The attached tab is marked `▌`. A tab whose session needs you has its name in the state's color. Every clickable thing (home, tabs, `+ new`) is a filled card on a darker strip, with a gap between cards, so each reads as a button. A full outline would need two more rows, and rounded caps need a Nerd Font. The tab-mode cursor lights its card in the accent color. A `⌂ home` card comes first, with the count that needs you under it; clicking it, or `^\ 0`, goes home. Tabs keep their order, so each project stays where you left it. When they don't fit, the strip slides to keep the cursor's tab in view, with `‹`/`›` showing that more are hidden. A `+` at the end starts a shell.
- **Names** (protocol 6). A session is named after its current directory, unless it was given a name: `--name`, Rename in its tab's menu, `r` in tab mode, `R` on the home screen, or `valk rename ID [NAME]`. A blank name goes back to the directory. The agent is never lost, because it has its own line. The shell's window title (`user@host:path`) is no longer shown anywhere.
- **Current directory.** Every tick the daemon reads where the PTY's foreground process is (`/proc/<pid>/cwd`, else the session's own process). A shell started in `~` is named after wherever it has `cd`'d, not `xdx`. Lists, the queue, new shells from `+`, the review diff stat and the restore list all use it. The upgrade handoff keeps the spawn directory.
- **Order** (protocol 7). The daemon keeps the tab order, so every client shows the same one. It survives upgrades (in the handoff) and restarts (the restore list is written in tab order). A new session goes last. Drag a tab to move it; it follows the pointer, as in a browser. Or press `H`/`L` in tab mode. `ClientMsg::Move { session, to }` renumbers the order.
- **Exited sessions leave.** A session goes from the lists, the queue and the tabs when it exits with nothing to show: an interactive shell, however it ended (`exit` passes on the last command's code), or anything that exited 0. A failure stays until closed, `blocked` in the queue: a crashed agent, or `valk new -- cargo test` going red. The daemon drops the session itself after the restore list's 3 s exit grace, so a reboot still brings it back.
- **Menu.** Right-clicking a tab, or `x` in tab mode, opens its menu with Rename and Close. It's per tab on purpose, because one big menu for every terminal option was a herdr gripe. There's no × on the tab, since it would sit right next to the click that switches and could kill a running agent. Close kills a shell straight away. For an agent it asks once (`Close claude? ↩`). Closing the attached session moves to the tab beside it, or home after the last one. While the menu is open every mouse report is Valkyrie's, so a click outside closes it without reaching the program. Upgrade and restore still save a name for every session, and a saved name equal to the program's is read as unnamed.
- **Keys.** `Ctrl-\` gives the strip the keyboard. There, `h`/`l` (or `←`/`→`) move, `H`/`L` move the tab itself, `Enter` switches, `1`–`9` jump straight to a tab, `0` goes home, `Tab` jumps to the next session that needs you in queue order, `n` starts a shell in the attached session's directory, `r` renames, `x` opens the tab's menu on Close, and `Esc` gives the keyboard back. `Ctrl-\` no longer reaches the program, so SIGQUIT from the keyboard is gone inside sessions.
- **Mouse.** A click switches, a click on `+` starts a shell, and a right-click opens the tab's menu. The session sits below the strip, so mouse reports going to a program that owns the mouse are moved up 2 rows. That applies to SGR reports, and to X10 reports too, where the ones over the strip are dropped. Kitty images are placed 2 rows lower. The strip hides below 12 rows.

**Agents started from a shell (done 2026-10-07).** herdr names a pane after the agent running in it, but a `claude` typed at a Valkyrie shell prompt showed as `fish · generic · idle`. It had no hooks (those are added only at spawn) and no screen scans (the adapter is picked from the spawn command). Now the daemon follows each shell session's foreground:
- **Detection.** Every tick it reads the PTY's foreground process group (`tcgetpgrp` on the master). On a change, and every 2 s while the group stays the same, it walks that group's process tree from the leader, up to 64 processes. It picks the shallowest `claude`/`codex`, so an agent under a wrapper script counts. A JS runtime's script also counts (`node …/codex.js`), but other programs' arguments don't (`vim claude.md`).
- **Effect.** The session gets a fresh tracker for that agent, with seq bumped. Screen scans then use that agent's adapter, and the tab's second line shows the agent's name. When the agent exits, the session goes back to the shell. A `foreground` line goes into the event log on each change.
- **Limit.** State is screen-only, because hooks can't be added to a running process, so the TUI keeps its `no hooks yet` hint. `valk new -- claude` still gives the hooked, accurate state.

Web/PWA: installable, push notifications, queue-first mobile layout, session drive (send input, approve, interrupt), start session in a chosen repo, decision review cards. Rendering via diffed screen stream; xterm.js (or custom canvas/WebGL renderer) for terminal view. Reference UX: the author's existing "kawaii"/Alice PWA.
**No network listener by default** (decided 2026-10-06). The daemon speaks only its unix socket, as Leader does, so it stays defensible on a managed work laptop. The web listener starts only when it's explicitly enabled in config.
Auth: token + WebAuthn/passkey; once enabled, assume exposure over Tailscale only; TLS and origin checks required. Never expose PTY control unauthenticated.

### 8.1 Remote resume over SSH (2026-10-07)
Requirement from the author's herdr use: `ssh box; valk` must show exactly the sessions left running on that machine, from any client machine. The daemon already owns the PTYs and clients are thin, so detaching or closing a client never touches the sessions. Two details made that fail over SSH; both are fixed, matching herdr's server:
- **The auto-started daemon calls `setsid`.** It leads its own session (parent init), so the terminal or SSH connection that started it can hang up without reaching it. Before this it was only in its own process group, inside the launching login session.
- **The default socket is `~/.local/state/valkyrie/run/<hostname>.sock`** (`$XDG_STATE_HOME`), not `$XDG_RUNTIME_DIR`. logind deletes `/run/user/<uid>` when the user's last login ends (without linger), and SSH logins may not set the variable. Either way, the next `valk` would start a second, empty daemon while the first kept the sessions. herdr keeps its socket under `~/.config/herdr` for the same reason.
  - The hostname is in the name because a unix socket only works on the host that bound it. Machines sharing an NFS home would otherwise each see the other's socket as stale, delete it, and orphan the other's daemon.
  - Relative `XDG_STATE_HOME`/`HOME` values are ignored; the fallback is the passwd entry's home.
  - Socket paths are limited to 107 bytes. A longer one fails up front with a message pointing at `--socket`/`VALK_SOCKET`.
  - Starting a daemon while one from an older build still answers on the old path prints a note with the command to stop it.
- **Sessions take login-bound variables from the client that spawns them** (`SSH_AUTH_SOCK`, `SSH_CONNECTION`, `DISPLAY`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`), unsetting any the client lacks, as tmux's `update-environment` does. Otherwise an agent started from a later SSH login would inherit the first login's dead agent socket and `git push` would fail. Sessions already running keep the environment they started with.
- `valk new` from a terminal without a real size (0×0 over `ssh host valk new …`) spawns at 120×40; the first attach resizes it.
- Verified by simulation: login 1 auto-starts the daemon and a shell session, then the whole login session is killed. Login 2 has a different size and no `XDG_RUNTIME_DIR`. It attaches with the TUI, sees the old output, and types into the same shell; a session it spawns gets login 2's `SSH_AUTH_SOCK`. Tests check that the daemon leads its own session and uses the per-host state-dir socket, and that a spawn applies the client's login environment.
- Accepted (LOW, from review):
  - Only the socket's own directory is checked for ownership and mode, not its parents. If `XDG_STATE_HOME` pointed into a world-writable directory, another user could race a rename.
  - Two simultaneous first starts can leave one idle, unreachable daemon (pre-existing).
  - `new` from a terminal smaller than 20×4 spawns at 120×40 until attached.

Follow-ups (herdr has them, we don't yet):
- ~~**Sessions survive a daemon upgrade.**~~ Done 2026-10-07: `valk upgrade` ([ADR-0006](docs/adr/0006-upgrade-handoff.md)) re-execs the daemon in place, so agents stay its children. PTYs and the listener are inherited and screens are rebuilt from the transcripts. The TUI reconnects by itself. Verified live by upgrading mid-turn under Claude running `sleep 15`: the handoff took 105 ms end to end, the session stayed `working`, and PostToolUse and Stop reached the new image.
- **Start on boot.** A systemd user unit plus `loginctl enable-linger` would bring the daemon up before any login. Today the first `valk` command starts it.
- **`valk --remote <host>`.** Runs the local TUI against a remote daemon by forwarding its socket over SSH. Today's equivalent is `ssh -t host valk attach`.
- ~~**Restore after reboot.**~~ Done 2026-10-07, see §8.3.

### 8.2 Scrollback, mouse and clipboard (2026-10-07)
These are table stakes when switching from herdr, which has all three.
- **Scrollback.** The daemon's screen keeps 10,000 lines of history.
  - `ClientMsg::Scrollback { anchor }` returns one screenful: `Up(n)` lines above the screen, or `FromTop(n)`, which stays put while output arrives.
  - The TUI scrolls back with the wheel or Shift-PageUp. In scroll mode:
    - `j/k`, the arrows, PgUp/PgDn, `b`/space and `g`/Home move.
    - `q`, Esc, `G` and End return to the live screen.
    - Any other key returns to the live screen and goes to the program. A held `j` that reaches the live screen is dropped, not typed.
    - The bar shows `↑ n/total lines back`.
  - The page holds still while output arrives, as tmux's copy mode does. It is not refetched until you scroll, so the part that overlaps the live screen can be stale.
  - Limit: once a session has 10,000 lines of history, each new line drops the oldest. A `FromTop` anchor then shifts by the lines added since the last scroll. Fixing that needs an absolute line counter in the daemon.
  - Full-screen (alternate-screen) programs have no history, so like other terminals the wheel sends them arrow keys (mode 1007), and Shift-PageUp goes to them.
- **Mouse.** Modes now carries the program's mouse modes (1000/1002/1003, SGR, UTF-8).
  - A program that asked for the mouse gets the outer terminal mirrored and the raw reports forwarded.
  - Otherwise, while attached, the TUI enables 1002+1006 for itself: wheel scrolling, and drag to select (shown reversed). Release copies the selection.
  - Mouse modes are rewritten only when they change, so a drag survives other mode changes.
  - A report cut off at the end of a read is held for the next one.
  - The home screen skips whole escape sequences, so stray reports never act as keys.
  - **Agents are full-screen mouse programs.** The recorded Claude Code (full-screen renderer) and Codex sessions both switch to the alternate screen (1049) and enable 1000/1002/1003/1006. Before protocol 4, Valkyrie never mirrored mouse modes, so the wheel did nothing inside them. Now they get the mouse and scroll and select themselves. Claude copies its selection with OSC 52, which arrives as below. Valkyrie's own scroll mode serves shells and other inline programs.
  - Rows carry `wrapped`, so a copy joins soft-wrapped lines.
  - Shift-drag still gives the terminal's own selection in most terminals.
- **Clipboard.** The TUI copies through OSC 52 on its own terminal, wrapped for tmux, so copies reach the machine you sit at, over SSH too. Inside tmux this needs `allow-passthrough on`. A program's OSC 52 copy, such as Claude's `/copy`, is pushed to attached clients as `ServerMsg::Clipboard` and copied the same way. OSC 52 reads (paste requests) are never answered. Replaying a transcript during an upgrade never re-copies. A program's copy is capped at 1 MiB, so it always fits a frame.
- **Protocol 4.**

### 8.3 Restore after a restart (2026-10-07)
herdr resumes its agent panes into their own conversations after its server restarts (`resume_agents_on_restore`). Valkyrie does the same for a daemon that died, whether from a reboot, a crash or `kill`. An upgrade keeps the sessions themselves (ADR-0006).
- **The list.** The daemon keeps `<state dir>/restore-<host>.json` (mode 0600) in step with its sessions: name, command, cwd, and the agent's conversation id. It is checked every second and written only when it changes.
  - A daemon on a non-default socket gets its own `restore-<host>-<hash>.json`, so it never restores another daemon's live sessions.
  - The conversation id is the `session_id` of the agent's `SessionStart` hook, so it follows `/clear`. It is taken only when the agent isn't mid-turn, since Codex hooks are global and a `codex exec` the agent runs reports its own id. It crosses upgrades in the handoff.
  - Entries that fail to come back stay on the list for the next restart, for example `claude` not on PATH yet or a missing cwd. A list that doesn't parse is set aside as `.json.bad`.
  - Spawning into a missing directory is now an error; portable-pty used to start the program elsewhere.
  - Per-spawn environment is not saved: restored sessions get the daemon's.
  - A session you kill leaves the list at once. A program that exits on its own leaves after 3 s. A reboot ends every program at once, and the daemon may reap some before it dies itself; the delay keeps those on the list.
- **Restore.** A freshly started daemon (`run`, not `resume`) spawns every entry its adapter can restore, at 120×40 in its old cwd and under its old name. `VALK_RESTORE=off` skips it.
  - **Claude:** `claude --resume <id> [options]`. Flags are kept and the start-up prompt is dropped. `-c`, `-r`, `--session-id`, `--fork-session`, `-w` and `--init` are replaced or dropped, and a `-p` run is not restored.
    - The id comes first: a value-taking flag the table doesn't know keeps no value, and at the end it would swallow `--resume` and send the id as a prompt. Here it fails loudly instead.
  - **Codex:** `codex resume <id> [options]`. `codex exec` and other subcommands are not restored, and `-i` images are dropped.
  - **Interactive shells** (`bash`, `fish -l`…, with only known interactive flags) start again. `bash -lc …` and `fish --command=…` are commands, not shells.
  - **Anything else** is not restored, because running an arbitrary command again could do harm. Neither is an agent that never reported a conversation (herdr likewise needs a session ref).
- **Verified.** A daemon test restores a fake `claude --model x hello` as `--model x --resume conv-1` plus a shell, and checks that a killed session, an exited program and `sleep` stay gone. With the real binary, `kill -9` of the daemon followed by `valk ls` brought back the shell in its old cwd.
- **Follow-ups.**
  - Start on boot (systemd user unit) so restore happens without a login.
  - Restored sessions have no scrollback from before.
  - Restore is still unverified against a live Claude or Codex conversation.

### 8.4 Images (Kitty graphics) (2026-10-07)
The author's Ghostty image previews (yazi) break under herdr, whose `experimental.kitty_graphics` is off by default. Valkyrie passes them through.
- **Why they broke.** A multiplexer emulates its own terminal. `alacritty_terminal`'s parser drops APC strings, so graphics commands vanished. yazi probes at startup, and with nothing answering it decided there was no image support (`ya env`: `kgp: false`, `csi_16t: (0, 0)`). The probe is a Kitty query (`a=q`), `CSI 16 t`, the PTY's pixel size, then DA1 last.
- **Daemon.** `valkyrie_term::graphics::Scanner` cuts `ESC _ G … ESC \` commands out of the output before the VT parser, across reads.
  - **What reaches a client's real terminal is checked first.**
    - An APC ends where the VT parser would end it: at `ESC \`, or at any other ESC, which then starts a new sequence. CAN or SUB abort it. So nothing can be smuggled inside one, and a stray `ESC _` in binary output costs one sequence, not the session.
    - A command passes only if its control part is `[A-Za-z0-9=,-]` and its payload is base64.
    - Only direct transmission (`t=d`) is forwarded. A file, temp file or shared memory would be read on the client's machine, by a path the program chose.
  - **The daemon answers the program itself**, once, however many clients are attached:
    - `OK` to queries (`a=q`), transmissions and placements that asked for one.
    - `EINVAL` for other media, so yazi falls back to direct.
    - `CSI 16 t` and `CSI 14 t` with the client's real cell size in pixels, which also fills the PTY winsize pixel fields.

    VT events are drained per text piece, so replies keep the queries' order; yazi treats the DA1 answer as the end. Clients get every command with `q=2`, so their terminals never answer.
  - **Forwarding and the log.**
    - The chunks of one transmission travel as one `ServerMsg::Graphics`, carrying the cursor cell where the transmission began. That keeps a large image from flooding the session feed. Transmissions are capped at 12 MiB so they fit a frame.
    - Each forwarded command is also kept in a per-session `graphics::Log` of what is still shown, shared as `Arc<str>` so replay costs nothing under the session lock. A new transmission of an id replaces the old one. Uppercase deletes (`d=A/I/N`) forget the data; lowercase ones forget only the placements, and a kept `a=T` becomes `a=t`. The log is capped at 32 MiB per session, evicting whole commands.
  - **Attach.** The client gets the log after the snapshot.
  - **Upgrades.** The log is rebuilt from the transcript.
- **Client.** The TUI writes each command at its cell (save cursor, `CUP`, command, restore) and clears all images (`a=d,d=A,q=2`) when it leaves a session or exits.
  - It reports `ClientMsg::CellPixels` from `TIOCGWINSZ` (Ghostty sets it, and SSH forwards it): at startup, before `valk new` spawns, on attach and on every resize.
  - Any late reply from the terminal is skipped as a whole on the home screen.
- **Placement styles.** Both work.
  - Direct placements at the cursor are what yazi 26 uses (one `a=t`, then a placement per cell).
  - Unicode placeholders (`U=1`) are ordinary text cells on the screen, so they scroll and redraw with everything else.
- **Verified.** yazi 26.9.1 previewing a PNG inside a session now transmits and places the image, and the TUI wrote all 26 commands. Attaching after yazi drew while detached replayed all 24 kept commands with `q=2`. Both runs used a pty without real graphics, so the final check is visual, in Ghostty.
- **Also fixed:** sessions inherited the daemon's `$PWD`, so yazi opened in the wrong directory.
- **Limits.**
  - Direct placements stay where they were drawn while you are in Valkyrie's own scroll mode. Agents and yazi are full-screen programs, which don't use it.
  - A client whose terminal has no Kitty graphics just ignores the commands (APC strings are skipped by terminals). The program still believes images work, because the daemon answers the query for every client.
  - Sixel and iTerm2 images are not handled; Ghostty supports neither.
  - A direct placement without `C=1` moves a real terminal's cursor past the image; the daemon's VT doesn't. yazi uses `C=1` and placeholders don't move it.
  - `CSI 16 t` split across reads isn't seen (alacritty ignores it, so the program gets no answer).
  - The cell size is the last client's. With two clients on different fonts, programs size images for one of them; placeholders scale either way.
  - A program that clears the screen leaves direct placements on the client's terminal until it deletes them itself.

### 8.5 Web app, slice 1 (2026-10-08)

The phone client, so sessions can be driven from anywhere on the tailnet. It's built as an app rather than a terminal in a browser. The author's Alice PWA (`~/repos/kawaii`) was studied first. It drives herdr sessions from a phone but can't answer permission prompts, so they had to be dodged with auto mode, or answered in the terminal. Valkyrie owns the PTY, so the phone answers them.

- **`valk web`, a separate opt-in process** (`crates/valkyrie-web`, axum). It binds `127.0.0.1:8790`, and `tailscale serve --bg 8790` puts tailnet-only HTTPS in front of it. The daemon still never listens on a network. Each browser WebSocket gets its own daemon connection, and frames pass through as the same JSON the TUI speaks. A browser may not send `Upgrade` (switch the daemon's binary) or `Hook` (pose as an agent). The WebSocket's Origin must match the host, or the forwarded host behind a proxy.
- **Pairing.** `valk web` (and `valk web pair`) prints a QR code of `<url>/#pair=<code>`. The URL is the machine's tailnet name, from `tailscale status`. A code works once, for 10 minutes. The phone trades it at `/api/pair` for a 256-bit device token, and the app removes the code from the address bar at once. Only SHA-256 hashes of codes and tokens are stored (`<state>/web/`, mode 0600). The token rides as a WebSocket subprotocol (`bearer.<token>`), as Alice's does, because browsers can't set headers there. An unknown token is upgraded and then closed with 4401, so the app tells "pair again" apart from a network drop. `valk web devices` lists devices and `valk web revoke <name|all>` unpairs them.
- **Watching without resizing** (protocol 8). `Attach.size` is optional. A phone watches the screen at whatever size it has, so it never shrinks the desktop's session. Attaching still marks the session's state as seen.
- **The app** (`web/`: React 19, Vite 8, Zustand, hand-written CSS in the TUI's Dracula palette). The build in `web/dist` is committed and embedded with `include_dir`, so `cargo install` needs no Node.
  - The connection is shaped after Alice's socket: backoff with jitter, an immediate retry when the page becomes visible or the network returns, and a `hello` keepalive whose missing reply means a dead socket.
  - **Inbox** (home). The queue is shown as cards, and the answers are right on the card. For each `needs input` item, the app reads the screen (`Dump`) and finds the prompt's choices. Swiping a card marks it seen. All sessions are listed below.
  - **Choices.** A choice is two or more options in the screen's bottom 25 lines, numbered or not, with one under the program's cursor (`❯`/`›`). A numbered list in ordinary output has no cursor, so it isn't a question. Wrapped option labels are joined, and key hints like `(y)` are dropped. Picking a choice moves the program's cursor to it, then presses Enter, one key per write 70 ms apart, so Ink reads separate presses. It never types the digit. This is tested against the recorded Claude permission and trust screens and the Codex approval screen.
  - **Session.** The live screen is reflowed on a phone: soft-wrapped rows are joined into logical lines that wrap at the phone's width. On a wide screen it shows as the exact grid. Below the screen are choice buttons, a key bar (Esc, Tab, ⇧Tab, arrows, ^C, Enter), and a composer. Multi-line text goes as a bracketed paste when the program takes one, and a draft survives the app closing. Rename and Close (Close confirms) are in the session menu.
- **Verified** with Playwright as an iPhone 13 against a throwaway daemon. The phone paired from the link, the inbox showed the recorded Claude dialog as four buttons, and tapping "No" delivered `↓↓↓⏎` to the session. A message from the composer ran in a shell. The grid view rendered at desktop width.
- **Next slices:** PWA install and Web Push (done: §8.6); a Chat view from the agents' transcripts (done: §8.7); a new-session launcher, reviewing an agent's diff on the phone, dictation and allowing several prompts at once (done: §8.8).

### 8.6 Web app, slice 2: install and notifications (2026-10-08)

The phone app becomes an installed app that tells you when an agent needs you, so you don't have to keep checking it.

- **Install.** `manifest.webmanifest` (standalone, Dracula colors) and PNG icons, because iOS ignores SVG: 192, 512, maskable 512 and a 180 apple-touch icon, plus a monochrome badge for Android's status bar. The mark is two blades of a V holding the TUI's ◆. Sources are in `web/icon/`; `node web/icon/render.cjs` renders them with Playwright. Android's `beforeinstallprompt` is kept for an "Install app" button. iOS gets instructions instead, because it has no prompt.
- **Service worker** (`web/public/sw.js`, hand-written, no build plugin). It shows pushes, sets the app badge, and on tap focuses an open app and routes it to the session, or opens one. It caches nothing: the app is useless without the live daemon, and a stale shell would only confuse.
- **Web Push from `valk web`** (`push.rs`). Messages are encrypted per RFC 8291 (`aes128gcm`, P-256 ECDH, HKDF) and signed with VAPID (RFC 8292, ES256), using RustCrypto and reqwest on rustls with ring, so no OpenSSL. The encryption is tested against the RFC's own example. The VAPID key is made once (`<state>/web/vapid.json`, 0600). Subscriptions are stored per paired device, so `valk web revoke` stops that device's pushes. A 404 or 410 from the push service drops a subscription. The endpoint URL is a capability, so errors are logged without it. Every push carries `Topic: s<id>`, so a phone that was offline gets the latest push per session, not a backlog. `Urgency: high` and 12 h TTL for asks; normal and 1 h for finishes.
- **When to push** (`notify.rs`). The server watches the daemon's queue. A transition becomes pending when it needs you (any `needs input` except a bell you've seen) or when it finished or failed and nobody watched (`done`/`blocked`, unseen). Guesses like `interrupted?` never push. A pending item is pushed once, when all of these hold:
  - it has waited 20 s, so a desk that answers at once never buzzes;
  - nobody has typed into any session for 90 s. Sessions now record `last_input_ms` (in `SessionInfo`, `serde(default)`, no protocol bump), stamped only by client input, not the terminal's replies to queries. A TUI left attached when you walk away no longer hides prompts, because `seen` alone can't tell "watching" from "left the window open";
  - no web app is on screen. The app reports visibility to `valk web`, which keeps it and doesn't pass it to the daemon. Anything that arrives or waits while the app is open counts as seen there.

  What was already queued when the server started isn't news. The body is the hook's summary. For a bare bell or a screen-detected ask, it's the screen's last line with words (`Approve the deploy?`), not "bell".
- **In the app.** A bell in the inbox header opens a panel to turn notifications on or off, send a test, and install. A one-time card suggests turning them on, or, in iOS Safari, explains adding to the Home Screen first, because iOS only offers push there. HTTP from another machine explains that HTTPS is needed. Each launch re-sends the browser's subscription (idempotent), and replaces one made for another server key. Opening the app clears its notifications. The badge follows the queue length.
- **Verified** end to end with Playwright on a real Chromium profile, Android device profile, against a throwaway daemon:
  - The manifest triggered `beforeinstallprompt`.
  - "Turn on" got a real FCM subscription. Chromium takes ~24 s to register with FCM, hence the "Turning on…" state.
  - "Send a test" went through Google's push service, and the service worker showed it.
  - With the app closed, a session that rang the bell produced "deploy · needs you / Approve the deploy?" 26 s later, linking to `/#/s/<id>`.
  - Headless Chromium has no Push API in incognito contexts, and it drops subscriptions when the notification grant goes. That cost a few runs of the test, not a bug in the app.
  - This sandbox can't reach `jmtNN.google.com`, so the test rewrote endpoints to `fcm.googleapis.com`, the same FCM service. Real phones use whatever their browser gives.
  - Not yet seen on a real iPhone, where Apple's push service is the remaining unknown.
- **Not done:** answering from the notification itself (Android action buttons; the service worker would need the device token), and `pushsubscriptionchange` (the next launch re-subscribes instead).

### 8.7 Web app, slice 3: the Chat view (2026-10-08)

A terminal is the wrong shape for a phone: Claude's TUI is 120 columns of boxes, and the scrollback holds a screenful. An agent's own transcript is the whole conversation in order, so the session view gets a Chat tab built from it, next to the Terminal tab.

- **Where the transcript is** (daemon, `chat.rs`). `SessionInfo.chat` names the agent's JSONL log (`serde(default)`, skipped when unknown, no protocol bump). It comes from two places:
  - **Hooks.** Claude's and Codex's payloads carry `transcript_path`. It's taken on `SessionStart` (which `/clear` also sends) or a prompt submit, only while not working: Codex hooks are global, so a `codex exec` the agent runs mid-turn reports its own one-shot transcript. The same rule already guards the conversation id. Paths must be absolute `.jsonl`.
  - **The process, for a `claude` typed at a shell prompt.** It gets no hooks, since Claude's are added per session. Claude writes `<config>/sessions/<pid>.json` (`sessionId`, `cwd`), so the daemon finds the foreground agent's pid (`foreground::agent` now returns it), reads that file, and opens `<config>/projects/<cwd with each non-alphanumeric a dash>/<id>.jsonl`. If the directory naming doesn't match, it looks for the id in every project directory. A file whose `pid` field names another process is stale and ignored. This is rechecked every 2 s with the foreground, so a `/clear` moves it. Codex typed at a prompt is covered by its global hooks (`valk setup codex`).

  The path crosses `valk upgrade` in the handoff state.
- **Mapping** (`valk web`, `crates/valkyrie-web/src/chat.rs`), ported from Alice's mappers (`kawaii/adapter/alice_pwa/agent_sessions.py`). Neither format is documented, so an unknown or unparsable line is skipped. Items are `user`, `say`, `tool` and `mark`:
  - **Claude:** text and `tool_use` blocks from `assistant` lines. Typed prompts are `user` lines with string content. These are skipped: `isMeta` and compact summaries, a non-human `origin` (a subagent's hand-back), and anything starting with `<` (slash commands, task notifications, reminders) or "Another Claude session sent a message". A prompt typed mid-turn only exists as a `queued_command` attachment, so it's read from there. A `tool_result` updates its card: ok, failed, or **denied** ("Permission for this…", "doesn't want to proceed", "was rejected"). `turn_duration` becomes "Worked for 2m 5s", `compact_boundary` "Conversation compacted", and an Esc "Interrupted". Sidechains and thinking are skipped.
  - **Codex:** only `event_msg` lines. `item_completed` items are already finished, so a Codex card never shows as running: `UserMessage`, `AgentMessage`, `CommandExecution` (the `-lc` command; a non-zero exit fails it, `declined` denies it), `FileChange` (one card per file, using Codex's own unified diff), `ImageView`, web search, `McpToolCall` (`server.tool`, and its title). `task_complete` gives "Worked for…" and `turn_aborted` "Interrupted"/"Stopped". Reasoning is skipped.
  - **Cards** carry a one-line summary per tool (the command; `file (+3/-1)`; a pattern and where; a subagent's description; a question) and what opening one shows: the command, a diff (Claude edits are diffed with `similar`, 2 lines of context), written content, and the result (clipped to 4 kB). Paths inside the session's directory read relative to it.
  - Checked against this machine's real transcripts: a 13 MB Claude log maps in 75 ms (debug build, the last 6 MB), and a long Codex rollout gives sensible cards.
- **Streaming.** The app sends `{"t":"chat","session":id}` (or `null`) over its socket, and `valk web` keeps it, like `visible`. A task per browser asks the daemon where the transcript is (every 3 s) and reads new bytes every 600 ms. It reads only whole lines, skips lines over 16 MB, and reads at most 2 MB per poll. Opening a chat reads at most the last 6 MB and sends the last 400 items, collapsed to each item's latest version, with a reset. Later messages add items or replace one by id (a tool call finishing). When the path changes (`/clear`), it resets. The app ignores messages for any session other than the one open, because a replaced task can't always be stopped before its last send. Following starts when a session opens, whichever tab shows, so switching is instant.
- **The view** (`Chat.tsx`, logic in `lib/chat.ts`, unit-tested).
  - Your messages are right-aligned bubbles with the time. The agent's text is rendered with a small Markdown reader: paragraphs, headings, lists, fences, quotes, rules, inline code, bold, italics, and links. Only `http(s)` links open, so a file path stays text. Tables become monospace blocks that scroll sideways.
  - Marks are centered pills. Tool cards are closed by default and show the tool, the summary, and a status word, because `denied` must be readable at a glance. File cards lead with the file name and colored `+3 −1`, with the directory after, so a phone shows what changed before running out of width.
  - **Valkyrie's own idea: runs fold.** Three or more consecutive tool calls fold into one line, "Ran 6 commands, edited 2 files, looked at 3 · 1 failed". The run still going while the agent works stays open, so you can watch it.
  - While it works there's a spinner at the end. When it needs you, a note links to the terminal, and the dock's choice buttons work from either tab because the screen stays attached.
  - Chat is the default whenever a transcript is known, and the choice of tab is remembered. A session with none (a plain shell) shows no tabs.
  - **Keys** (desktop, when not typing): `t` switches tabs, `i` focuses the composer (Esc leaves it with the draft kept), `j`/`k` scroll, `Ctrl-d`/`Ctrl-u` half a page, `g`/`G` top and bottom, `q` back to the inbox.
- **Verified** with Playwright, as an iPhone 13 and at 1280 px, against a throwaway daemon. Fake `SessionStart` hooks pointed sessions at copies of a real Claude transcript and a real Codex rollout. The chat loaded with folded runs. A prompt, text and an `Edit` appended to the file appeared within a second, with the card going from running to done in place. The tabs switch, and a plain shell has none. The keys worked, and typing `t` in the composer didn't switch tabs. Separately, a `sleep` named `claude` typed at a shell prompt, with a fake `sessions/<pid>.json` under `CLAUDE_CONFIG_DIR`, gave the session its transcript path with no hooks at all. The path survived `valk upgrade`.
- **Not yet:** loading older history on demand (the view says "Earlier messages aren't loaded"), images in messages (shown as `[image]`), subagent conversations, and Codex commands while they run (its transcript only records them finished).

### 8.8 Web app, slice 4: start, review, dictate, allow all (2026-10-08)

With this, the phone does the whole loop: start an agent, answer it, see what it changed, and tell it what's next.

- **New sessions** (`#/new`, the `+` in the inbox, `n` on a keyboard). You pick Claude Code, Codex or Shell, a folder, an optional name, and an optional first message. The message goes on the agent's command line (`claude "<message>"`, `codex "<message>"`), so it starts working at once and nothing has to be typed into a TUI that's still drawing. The command is the agent itself, not a shell wrapping it, so Claude still gets its per-session hooks. The session starts at 120×40, and a TUI that attaches later resizes it. Afterwards the app opens the new session. The last agent and folder are remembered.
  - **Folders** come from `valk web` (`places.rs`). `GET /api/places` lists:
    - where live sessions run, most recent first;
    - git repos one level under `~/repos`, `~/src`, `~/code`, `~/projects`, `~/dev`, `~/work` and `~/git`, newest first by their index's mtime;
    - the server's `$SHELL`, for Shell.

    `GET /api/dirs?path=` lists one folder's subfolders (`~` expanded, canonicalized, hidden ones last, at most 400) for browsing anywhere. Only directories are listed, never files, and only to paired devices, which can already start shells.
- **Review** (`#/s/<id>/review`; from a finished card in the inbox, the session menu, the Chat view's "Finished" note, or `r`). `GET /api/review/<id>` (`review.rs`) runs git in the session's live directory:
  - `diff HEAD`, so staged and unstaged together, falling back to `--cached` before the first commit;
  - untracked files as additions, at most 40, each read up to 64 kB, with binary files detected by NUL.

  Git runs with `GIT_OPTIONAL_LOCKS=0`, so a review never takes the index lock under a working agent. It has a 15 s timeout. Each file's patch is capped at 200 kB and the total at 2 MB; past that, files are cut, and the view says so.

  The page shows the branch and totals, and one card per file: a status badge (M/A/D/R/U), the name before its directory, and `+a −r`. Opened, a card shows numbered lines (the new number, or the old one for a removal), hunk headers, and lines that scroll sideways under a sticky gutter. Files open in order until about 150 changed lines are showing.

  Below are replies that go straight to the agent: **Commit it**, **Run the tests**, **Explain**, or your own words (dictation works here too). After sending, the session opens so you see the answer. **Mark seen** clears a finished item. Keys: `j`/`k` move between files, `o` opens one, `r` re-reads, `i` writes, `q` goes back.
- **Dictation** (`lib/dictation.ts`, a mic beside every message box). It uses the browser's own speech recognition (`SpeechRecognition`, or `webkitSpeechRecognition` on Safari): continuous, with interim results shown as they form, after whatever was already typed, and left editable. Tap again to stop. It's hidden where the browser can't listen. The phone keyboard's own mic does much the same, but this one keeps listening across pauses and works while the key bar has the focus. Chrome sends audio to Google's service and Safari to Apple's, as with any web dictation.
- **Allow all.** When two or more prompts are waiting and each one's first choice is a plain yes, the inbox offers "N prompts waiting for a yes · Allow all…". It opens a list: each session, what it asks (see below) and the choice that will be picked, with a box to leave any out. **Allow N** answers them one by one, the same way a single tap does. `plainYes` takes only "Yes"/"Allow"/"Approve"/"Proceed", never a broader grant ("don't ask again", "always", "all", "for this session"), which you can still pick on the prompt itself.
- **What a prompt asks**, read off the screen. A screen-detected ask's summary said only "bell". `promptContext` reads the paragraph just above the choices, past generic lines like "Do you want to proceed?". It gives "touch acc.txt — Create empty acc.txt file" for Claude's dialog and "touch b.txt" for Codex's, checked against the recorded screens. Cards and the allow-all list show it whenever the summary says nothing.
- **Inbox keys:** `n` new session, `j`/`k` pick a card or row, `Enter`/`o` open it, `r` review it, `s` mark a card seen.
- **Verified** with Playwright as an iPhone 13 (and at 1280 px), against a throwaway daemon:
  - **Allow all.** Two sessions showed the recorded Claude and Codex prompts. The bar offered both, with what each asks, and "Allow 2" pressed Enter on each one's "Yes".
  - **Launcher.** It listed the live sessions' folders and this machine's repos, browsed into a folder, and started a named shell there.
  - **Review.** A repo with an edit, a deletion and a new file showed 4 files, `+6 −4`, with numbered hunks. "Explain" sent its text to that session and opened it.
  - **Dictation.** A stand-in recognizer filled the composer with "run the tests again". Headless Chromium can't really listen, and it has the unprefixed `SpeechRecognition`, which the stand-in had to replace too.
  - **Keys.** `j j j` then `r` opened the third session's review.
- **Not yet:** staging or committing from the review itself (it asks the agent instead), per-hunk comments, dictation on a real phone (Safari's recognizer in a Home Screen app is the one to check), and starting an agent with options (model, permission mode).

## 9. Tech choices (proposed, challengeable)
- Rust, tokio, axum (HTTP/WS), ratatui + crossterm, rusqlite (WAL) or sqlx, portable-pty, alacritty_terminal/vte, serde, tracing.
- Web: Rust-compiled WASM vs TypeScript (Svelte/Solid) is an open choice. Lean TS for PWA speed of iteration unless a shared protocol crate to WASM gives real wins.
- Protocol: versioned, schema-first (JSON now; evaluate MessagePack/CBOR for the screen stream). Generate TS types from Rust.
- Single static binary: daemon + TUI + CLI as subcommands; web assets embedded.

## 10. Milestones

M0 — Spike (1–2 wks): PTY host + VT state, spawn claude/codex, attach from a minimal TUI, latency harness. Decide tmux-less viability and VT crate.
M1 — Daemon + queue: event bus, adapter trait with Claude + Codex (hooks first, ACP evaluated), state detection with recorded-fixture tests, queue + TUI home screen.
M2 — Context layer: decision store, lifecycle, extraction → review flow, MCP server, pinned-block injection, handoff command.
M3 — Web/PWA: API parity, push, queue-first mobile UI, session drive.
M4 — Hardening + OSS release: docs, benchmarks vs AoE/herdr, adapter fixtures, security review.
M5 — Supervisor agent + policy engine.

## 11. Evaluation tasks for Claude Code + Opus (first pass)
1. Teardown AoE and herdr source: state-detection approach, session persistence, API surface, what they do poorly. Produce a gap table.
2. ~~Audit the author's herdr queue plugin for reusable semantics~~ → done, see §5.5 and §14.3–14.4.
3. Verify ACP/Claude hooks/Codex hooks capabilities and limits; recommend state-detection tier strategy per agent.
4. Benchmark candidate VT crates for embedding, memory, and diffing; propose screen-diff wire format.
5. Stress-test the decision schema and staleness model against agentmem/engram; decide repo-local vs data-dir storage.
6. Threat model: remote PTY control, injected context as prompt-injection vector, transcript secrets.
7. Propose the project name, crate layout, and an ADR list for the open decisions below.

## 12. Open decisions
- ~~VT crate~~ → alacritty_terminal ([ADR-0001](docs/adr/0001-vt-engine.md)).
- ~~tmux vs owned PTY~~ → owned PTY; M0 exit criteria met ([ADR-0002](docs/adr/0002-no-tmux.md)).
- ~~PTY-native-with-hooks vs ACP-structured~~ → PTY-native with hooks for M1 ([ADR-0005](docs/adr/0005-state-signals.md)); ACP stays an opt-in idea for the phone UX.
- Repo-local vs daemon-local decision storage.
- ~~Web listener default~~ → off; opt-in via config (§8).
- Web client: TS vs Rust/WASM.
- License (MIT vs Apache-2.0 vs dual).
- Summarizer/extractor model strategy: local-only default vs bring-your-own API key.
- Name.

## 13. M0 spec (complete 2026-10-06)

Decisions: [ADR-0001 VT engine](docs/adr/0001-vt-engine.md) · [ADR-0002 no tmux](docs/adr/0002-no-tmux.md) · [ADR-0003 protocol](docs/adr/0003-m0-protocol.md) · [ADR-0004 crate layout](docs/adr/0004-crate-layout.md)

Session data flow:
```
pty reader thread ─bytes─► Term (alacritty) ─damage─► Screen diff ─broadcast─► clients
        ▲                     │ PtyWrite (query replies)
        └──── pty writer ◄────┴──────────── Input frames from clients
raw bytes also appended to $XDG_STATE_HOME/valkyrie/sessions/<unix-ts>-<id>.raw (0600) (fixtures for M1 heuristics)
```

CLI surface (one binary):
- `valk daemon` — run the daemon in the foreground.
- `valk new [--cwd DIR] [--name N] -- CMD...` — spawn a session (auto-starts daemon if absent).
- `valk ls` · `valk kill ID` · `valk dump ID` (plain-text screen; used for headless verification).
- `valk send ID TEXT` — inject input (`\r \n \t \e \xHH` escapes); headless driving and future supervisor plumbing.
- `valk attach [ID]` / `valk` — TUI: session list home screen, Enter attaches, `Ctrl-]` detaches, `Ctrl-\` gives the tab strip the keyboard.
- `valk rename ID [NAME]` — name a session; without a name it goes back to its directory's.
- `valk bench latency [-n N]` — keystroke→screen-update round trip through the daemon against `cat`; prints p50/p99/max.
- `valk bench parse FILE` — VT parse throughput over a recorded `.raw` transcript.

Acceptance (= ADR-0002 exit criteria):
1. `claude` and `codex` render correctly and are drivable via attach.
2. Detach → reattach restores the exact screen; session keeps running with zero clients.
3. Resize propagates (client size → PTY + Term).
4. `bench latency` p99 < 16 ms on the author's machine; numbers recorded here.
5. Unit tests: frame codec round-trip, screen diff correctness (damage rows == changed rows), span grouping.

Out of scope for M0: adapters/state detection, queue, HTTP/WS, auth, mouse passthrough, kitty keyboard, scrollback browsing.

### M0 results (2026-10-06, author's machine, release build)

| Criterion | Result |
|---|---|
| 1. claude / codex render + drivable | ✅ Both render their trust prompts with correct box-drawing and selection markers through the daemon. Arrow keys sent via `send` and via the nested TUI moved the selection. Prompts were not accepted (that would change real config). |
| 2. Detach → reattach | ✅ Exercised by running the TUI as a session inside the daemon: `Ctrl-]` shows the home list, and Enter on another session restores its full screen. |
| 3. Resize propagates | ✅ Integration test: attach at 50×12 → program's `stty size` reports `12 50`. |
| 4. Keystroke → screen update | ✅ **p50 22 µs, p99 29 µs, max 206 µs** (n=2000, raw-mode `cat`, after review fixes). That is 500× under the 16 ms budget; the client's own terminal render is not included. |
| 5. Unit/integration tests | ✅ 29 tests: codec, private socket dir, term diffs/spans/wide and combined chars/query replies, TUI apply/render, 9 daemon end-to-end tests on real PTYs. Three of the daemon tests are regression tests for review findings: a stalled input writer, exit while a background job holds the tty, and process-group kill. |
| VT parse + diff throughput | ~210–250 MiB/s on recorded agent transcripts (repeated 20×). |

Manual check (author, 2026-10-06): `new -a -- claude` in a real terminal runs Claude Code inside the TUI, and exiting Claude returns to the session list. Works. The UI needs polish, deferred: a queue-first home screen arrives with M1 anyway.

Findings that feed M1:
- Both agents' first screen is a **trust prompt**, a ready-made `needs_input` fixture (transcripts are saved per session).
- Agents query the terminal at startup: from the transcripts, Codex sends OSC 11 (background color), DSR 6n and DA1/DA2, and Claude Code sends DA1. alacritty answers these, and the daemon reports a fixed dark palette for color queries. Passing through the real client palette is a later refinement.
- Known gaps: no scrollback browsing, mouse, or kitty keyboard; the last client to resize wins; queued input is unbounded per session (a huge paste into a program that isn't reading stays in daemon memory); background jobs that ignore SIGHUP and outlive the agent are not SIGKILLed (only signalled while the leader is unreaped, to avoid hitting a recycled pgid).

Hardening from the M0 code review (all applied):
- PTY writes run on a per-session writer thread, so neither tokio workers nor the reader thread ever block on a full tty.
- A dedicated waiter thread reaps the child, so `Exited` doesn't depend on PTY EOF. `kill` signals the whole process group: SIGHUP, then SIGKILL after 2 s.
- Both daemon and client verify the socket dir is owned by us, mode 0700 and not a symlink. (The dir itself moved on 2026-10-07; see §8.1.)
- Each connection's output queue is bounded at 256 frames. A slow client drops its diffs and resyncs from a snapshot. The resync re-sends `Exited` too, and reattach waits for the old stream to stop.
- Spans re-anchor after wide or combined graphemes, so width disagreements (e.g. `⚠️`) can't shift a row. Accept errors no longer kill the daemon, and `daemon.log` is 0600.

## 14. M1 spec (implemented 2026-10-06; Codex verified live 2026-10-07)

Goal: the home screen becomes a ranked attention queue fed by accurate agent state. The M1 exit criterion is **state accuracy measured against recorded fixtures** (DESIGN §7 makes this the gate for the supervisor).

Decisions: [ADR-0005 state signals](docs/adr/0005-state-signals.md): hooks first, heuristics as fallback, observe-only.

### 14.1 Pipeline
```
hook (valk hook <agent>) ─┐
PTY output / bell / title ────┼─► adapter.normalize ─► AgentEvent ─► state machine ─► QueueItem
process exit / timers ────────┘        (per agent)        (bus)        (per session)    (ranked)
```
- `AgentEvent` (normalized): `PromptSubmitted`, `ToolStarted{name, summary}`, `ToolFinished{name, ok}`, `PermissionAsked{tool, summary}`, `InputAsked{message}`, `TurnEnded{last_message}`, `TurnFailed{error}`, `Interrupted`, `Idle`, `ScreenPrompt{kind, text}` (heuristic), `Bell`, `Title`, `Exited{code}`.
- Each event is appended to `sessions/<ns>-<id>.events.jsonl` next to the `.raw` transcript, so every real session becomes a replayable fixture.

### 14.2 Adapters
```rust
trait Adapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn matches(&self, command: &[String]) -> bool;            // claude, codex, generic
    fn prepare(&self, cmd: &mut Vec<String>, env: &mut Env);  // e.g. claude --settings
    fn normalize(&self, hook: &serde_json::Value) -> Vec<AgentEvent>;
    fn scan(&self, screen: &ScreenText) -> Option<AgentEvent>; // heuristics, run on quiet output
}
```
- `claude`: `prepare` adds `--settings` with hooks for the events above. `scan` detects the folder-trust prompt and the permission dialog (as a fallback).
- `codex`: hooks come from `~/.codex/hooks.json` (`valk setup codex`). `scan` covers the trust prompt, the approval dialog, and the auto-reviewer spinner. The session shows "heuristics only" until the first hook event arrives. Codex fires `SessionStart` lazily, just before the first `UserPromptSubmit`, and fires `Interrupt` on Esc-deny and Esc-interrupt, so it has no hook gaps.
- **Codex auto-review** (`approvals_reviewer = "auto_review"`): `PermissionRequest` fires *before* the reviewer decides, and its payload is identical to a human-bound request. While the screen shows "Reviewing approval request" the session is `working` (summary `auto-review: Permission: …`). If the reviewer hands the request on, the approval dialog appears on screen and the session returns to `needs_input` with the original summary.
- `generic`: no hooks. Bell → `InputAsked`, plus quiet-output detection.
- Heuristics run once output has been quiet for ~150 ms, never per chunk, so their cost stays off the latency path. While output keeps coming (an animated spinner never goes quiet) the daemon also takes a **glance** every 500 ms, logged as `scan` with `"quiet": false`. Glances only drive the auto-review checks: a mid-frame screen can look idle (Claude draws its prompt box under the spinner), so idle detection and screen-only heuristics wait for quiet scans.

### 14.3 State machine (per session)
States from §5.1: `working · needs_input · blocked · review_ready · idle · stale · exited`. Each transition gets a monotonic `seq`. Hooks carry the time they were sent, and an event older than the session's last applied one is dropped, so hooks that arrive out of order can't roll the state back (Leader §2).

| Event | → state | notes (Leader findings §8, §11, §14) |
|---|---|---|
| PromptSubmitted | working | clears summary and seen flag |
| ToolStarted, ToolFinished (ok or failed) | working | `PostToolUse*` is the only signal that a permission was **approved**; nothing fires on approval itself |
| PermissionAsked | needs_input | primary signal: fires ~50–90 ms after the dialog appears; summary `Permission: <tool> <command/path>` |
| ToolStarted(`AskUserQuestion`), InputAsked (elicitation, `agent_needs_input`) | needs_input | summary = the question text |
| Notification `permission_prompt` | needs_input, **only from working** | arrives ~6 s **after** PermissionAsked with a vaguer message, sometimes after the dialog was already answered or denied (seen live). It counts only as a fallback for a missed PermissionAsked |
| Notification `idle_prompt` | ignored | must not revive a settled session |
| TurnFailed | blocked | summary `Turn failed: <error>` |
| TurnEnded | review_ready | "done, unseen". Attaching (or `s`) makes it idle. Summary = most conclusion-like line of `last_assistant_message`, plus `git diff --stat` when the tree changed |
| SessionStart (`startup`/`resume`/`clear`) | idle | `compact` is ignored (it can fire mid-turn) |
| Subagent events (`agent_id` set) | only tool/permission/notification events count | a subagent's permission dialog blocks the same terminal; its Stop/SessionStart must not settle the parent (herdr's own integration ignores `SubagentStop` too) |
| Screen shows the idle prompt ≥10 s within the current working state, or within a needs_input from a permission (2 s if the permission dialog itself was seen on screen and then gave way to the prompt) | `interrupted` (shown `interrupted?`) | covers the two gaps where **no hook fires**: Esc-deny of a permission, and Esc-interrupt mid-turn. Codex has an `Interrupt` hook; Claude doesn't |
| no output or events for N min (default 5) while working | stale | |
| Exited | exited (nonzero code → blocked) | |

User input sent to a `needs_input` session doesn't change its state; the agent's next event does. Changed from the earlier draft: `review_ready` no longer requires a changed working tree, because Leader's "done-unseen" (any finished turn) is what the author uses daily, and an answer-only turn still needs reading. The diff stat is used only in the summary.

Because Valkyrie owns the screen, the `interrupted?` check reads the session's own `VtScreen` through `Adapter::scan`. Herdr needed a separate `agent explain` call for the same check.

### 14.4 Queue
`QueueItem { session, state, reason, summary, since, seq, project }`.
- **Ranking** (ported from Leader, extended with Valkyrie's extra states): `needs_input` > `blocked` > `review_ready` > `interrupted?`/`stale`; oldest first within a state, then by session id. `working` and `idle` sessions are not queue items; they appear in the session list below the queue. `interrupted?` rows are always shown (Leader §14: never hide them with idle rows).
- **Seen model:** `review_ready` stays in the queue until the user attaches to that session or marks it seen (`s` for one item, `S` for all). Marking seen is keyed by the transition's `seq`, so a newer turn brings the item back. Herdr had this built in. Valkyrie implements it, which is easy because the daemon sees every attach.
- **Summaries (M1, no LLM):** taken from the hook text first (permission + command, question, conclusion line). When a session has no hooks, a screen fallback on its `VtScreen` uses the last question-like line plus the pending command, or the last assistant block with chrome stripped. Port Leader's `summarize.py` rules: rejoin wrapped lines, strip chrome, apply the conclusion regex. Its `tests/fixtures/tail_*.txt` are real Claude captures and become test cases here.
- **Later (not M1):** an LLM summary on transition into review_ready, as in Leader M5. That means a locked-down `claude -p --tools "" --no-session-persistence --disable-slash-commands --strict-mcp-config --model haiku`, fed the last turn only after redaction, rate-limited, with the heuristic line as fallback. **Batching** has no precedent in Leader and stays new design for M3.
- TUI keys follow Leader's overlay: `j`/`k`, `Enter` attach, `Tab` top item, `s`/`S` seen, plus a footer with counts (`2 needs input · 1 done · 3 working`). Idle sessions are always listed under the queue, so there's no `a` toggle. Attaching marks a `review_ready` turn seen: it leaves the queue but keeps its state.

### 14.5 Protocol and clients
- `ClientMsg::Hook { session, agent, payload }` comes from `valk hook`, and `WatchQueue` subscribes a client to `ServerMsg::Queue { items }` pushes.
- TUI home: the queue on top, all sessions below; `Enter` attaches to the selected item and `Tab` jumps to the top item. The attached view's status bar shows the session state.

### 14.6 Fixtures and tests
- Every session writes `<ns>-<id>.events.jsonl` (mode 0600) next to its `.raw`. It records everything the tracker consumed, each tagged with the transcript byte offset: start (agent, size), output marks (≤4/s), hooks (slimmed payloads), bells, screen scans, attach/detach, seen, resize, exit, and every resulting state.
- `valk replay <events.jsonl> [--labels <labels.jsonl>]` re-feeds the transcript up to each offset and re-runs normalize + scan + tracker with the **current** code. It prints the timeline, drift from the recorded states, and time-weighted accuracy against hand labels (`{"at": <s>, "state": …}` lines; a replayed `interrupted` counts as a correct `idle`). `valk render <raw>` prints a transcript's screen for labeling.
- Gate: `crates/valkyrie/tests/fixtures/*.events.jsonl` with labels must replay at ≥95%. Unit tests cover normalize per agent, the state table, ranking and seen, and `scan` against recorded screens (Claude trust/permission/idle/done, Codex trust/approval/reviewing/busy/idle).

#### M1 results (2026-10-06, Claude Code 2.1.292, haiku, isolated daemon)
| Check | Result |
|---|---|
| Hooks via `--settings` in an already-trusted folder | fire with no prompt; `VALK_SESSION`/`VALK_SOCKET` reach the hook |
| `valk hook` round trip (real binary, n=30) | median **0.68 ms** (budget 50 ms; Leader's Python hook 15 ms) |
| Prompt → reply | working → done, summary `ok · uncommitted: 12 files +1114 -94` |
| Bash permission | needs_input `Permission: Bash touch …` 70 ms after PreToolUse |
| Esc-deny (no hook) | `interrupted?` |
| Approve | PostToolUse → working → done |
| Labeled live recording (`claude-permission-deny-approve`) | **98.1%**; the remaining 2 s is the deliberate deny grace |
| Tests | 84 across the workspace; clippy clean; independent review (19 findings) applied |

#### Codex live check (2026-10-07, Codex 0.159.2, `-s read-only -a on-request`, isolated daemon)
| Check | Result |
|---|---|
| Hooks from `~/.codex/hooks.json` after `/hooks` trust | all 8 fire; the hook command is the release binary path |
| Bash permission, reviewer = user | needs_input `Permission: Bash touch …`; approve → PostToolUse → working → done |
| Esc-deny | `Interrupt` hook 5 ms after the key → idle (no grace needed) |
| Bash permission, reviewer = auto_review | before the fix: a false needs_input for the whole review (1.7 s). After: needs_input until the next glance (≤500 ms; 0.26–0.34 s recorded), then working (`auto-review: …`) |
| Labeled recordings `codex-auto-review`, `codex-approve-deny` | **97.2%**, **98.3%** (the remainder is the pre-glance flash and hook lag) |

The same session found two Claude issues, both fixed and recorded (`claude-long-tool`: an auto-allowed `sleep 15`, **100%**):
- Claude 2.1.292's spinner (`✽ Drizzling… (8s · ↓ 140 tokens)`) no longer says "esc to interrupt", so a busy screen scanned as idle. `scan` now reads the spinner and the "to run in background" hint as busy.
- Any 150 ms lull during a long tool then counted as a fresh idle screen, and a turn over 10 s went `interrupted?`. Now `interrupted?` needs the screen idle **and silent** (no output at all) for the whole grace; an idle Claude redraws about once every 14 s.
- The replay gate re-runs the recorded scan times, so it can't catch a change in scan *scheduling*; recordings made before 2026-10-07 have no glances. Scheduling changes need a fresh live recording.

Bugs the live run found, all fixed and covered by tests:
- Claude's prompt is `❯` + U+00A0, not a plain space.
- A screen idle from *before* a new prompt immediately re-flagged `interrupted?`.
- The late `permission_prompt` notification re-blocked an already settled session.
- Narrow terminals hard-wrap phrases mid-word. Scans now use `VtScreen::unwrapped_text`, which joins soft-wrapped rows.

From the independent review, fixed with regression tests:
- **Abrupt disconnects.** A client dying with unread frames leaked its attach count (ECONNRESET skipped cleanup), so every later transition counted as already seen.
- **False `interrupted?` on long turns.** Every Claude turn over ~10 s was flagged, because the spinner never leaves the quiet gap a rescan needs. An idle verdict now only stands while no output has arrived since it was taken.
- **Hook could break its contract.** A hook whose arguments didn't parse printed to stderr and exited 2. Hooks now always exit 0, and the registered commands end in `2>/dev/null || true`.
- **Timer transitions.** Ones that fired while attached were never marked seen.
- **Bells.** A seen bell never settled, and later bells were swallowed.
- **TUI cursor.** It jumped to another session when the queue changed size.
- **Old daemons.** A new client couldn't talk to a pre-M1 daemon. There's now a `Hello`/`PROTOCOL` check with a clear "restart the daemon" message.
- **Nested agents.** A `codex exec` run by Claude drove Claude's session. Hooks now apply only from the session's own agent; plain shell sessions take any.
- **`setup codex` file handling:**
  - `--remove` wrote `null` when there was no file.
  - A symlinked file was replaced.
  - A backup could be overwritten.
  - The file mode was lost.

Accepted for now (LOW):
- `events.jsonl` has no rotation.
- Unbounded `git diff` threads.
- A user-supplied `--settings` may shadow ours.
- The hook `connect` has no timeout (bounded by the agent's 5 s hook timeout).
- Session ids restart with the daemon, so an orphan process could post to a reused id.
- No screen-fallback summaries for unhooked sessions.
- `interrupted?` needs the idle screen fully silent for the grace, so an idle screen that redraws on a timer faster than that (a ticking custom statusline) never gets flagged. Claude's own idle screen is silent today.
- A permission handed to the Codex auto-reviewer stays tracked across the next `PreToolUse`; if the reviewer denies and an unrelated tool then shows a "Would you like to …?" line before its `PostToolUse`, it is reported as the old request.

Still pending:
- More labeled recordings: AskUserQuestion, subagents, long tool runs, stale, and a Codex auto-reviewer that hands a request to the human.

### 14.7 Out of scope for M1
Approve/deny from the queue (M3), LLM summaries, batching, push notifications, SQLite (arrives with the M2 context store).

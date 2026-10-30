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

**Per-session worktrees (2026-10-11, protocol 12).**
- `SpawnSpec.worktree: Option<String>` asks for one: the branch, or `""` for one named after the session's name (slugged, `session` without one; `-2`, `-3` until neither the branch nor the folder exists). `valk new -w [BRANCH]` and the phone's "Own worktree" box send it.
- The daemon makes it, so every client gets the same behavior: `git worktree add` (on `spawn_blocking`) beside the main checkout as `<repo>-<branch>`, `/` in the branch becoming `-`. A new branch starts at the `HEAD` of `cwd`'s checkout; an existing branch is checked out. Asked from inside a linked worktree, it still goes beside the main checkout.
- The session starts in the same subdirectory of the new checkout as `cwd` was of its own. Restore sees only the new `cwd`, so a restored session never makes a second worktree.
- Failures (not a repo, a bad branch name, the folder taken, the branch checked out elsewhere) fail the spawn with git's reason, and nothing starts. A session that fails to start after its worktree was made takes it away again, and the branch too if it was new. A split checks its target session before making anything.
- Names: an auto-named branch keeps only ASCII letters, digits and `_` (so always a valid ref), and if another spawn takes the name between the check and `git worktree add`, it moves on to the next. A given branch must pass `check-ref-format refs/heads/<b>` and not start with `-`, so `-f` or `@{-1}` never reach git as options or expansions. Paths go through `canonical`, so Windows never hands git or the session a `\\?\` path.
- **Pruning merged worktrees (2026-10-28, protocol 14).** Nothing is removed when a session ends: the worktree may hold unmerged work. Instead `valk worktree prune` judges every linked worktree of the repository the shell is in (never the main checkout), and `--yes` takes away those that pass. The CLI runs `git fetch --prune origin` first, with the user's credentials; offline or without `origin` it says so, and only the main checkout's `HEAD` counts. Then `ClientMsg::PruneWorktrees` asks the daemon, which knows the sessions and the backlog, so the rule lives in one place, as for Ship. A worktree may go only when its `HEAD` is an ancestor of `origin/<default>` (from `refs/remotes/origin/HEAD`, else `main` or `master`) or of the main checkout's `HEAD` while that's on the default branch, and the output names exactly which; it has commits of its own (its branch's reflog holds more than its creation, since a fast-forward merge leaves the tip equal to `origin/main`; without a reflog, its `HEAD` isn't one of those tips), so a fresh worktree stays; no running session's `cwd`, now or where it started, is inside it; no open or proposed backlog item owns it (`valk todo ship N` is the way for those); it isn't locked, isn't the caller's own `cwd`, and has nothing uncommitted. Ignored files don't keep a worktree (every Rust one has `target/`), but `git worktree remove` deletes them, so the output lists them (`also deletes ignored: .env, target/`). A squash-merged branch fails the ancestor check, so a branch that fails it is also checked for its whole change (from where it left that history) being one commit there, by `git patch-id --verbatim` over the raw bytes, as `git cherry` does (whitespace kept, unlike `--stable`, since an indent changed after the merge is work of its own; line numbers ignored, so a main that moved on still matches); that passes as "squash-merged". It's the same change, not a guess from a deleted upstream, and a pull request that took more than the local branch holds doesn't match, so that branch stays. Only commits since a day before the branch's last one are read. A branch that matches nothing and whose upstream is gone is shown as "not merged (squash merge?)". Removal goes through the same `worktree::remove` as Ship: `git worktree remove`, never forced, so git still keeps a checkout with changes. Right before it, the worktree's `HEAD` is read again and it stays if that moved since the check. Its branch goes with `-D`, since the local `main` often lags `origin/main` and `-d` would keep every branch merged there, but only after that ancestor or squash check passed and while the branch still points at the commit checked; if git still refuses, its reason is shown. Even the dry run fetches. Anyone may look; removing is the user's (`vouch`), as shipping is. If the daemon can't be reached nothing is removed: it can't tell whether something runs there.
- **Not yet:** copying ignored files such as `.env`.

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
- **Sound.** At most one sound per second, with a toast in the bar for 6 s. The toast carries the session's summary after its name (`✓ api finished  Fixed the login redirect`), cut to 60 columns and read when drawn, since the summary can land after the ping.
  - The sounds are generated. The settings panel picks one (`sound`: `none`, `chime`, `pop`, `wood`, `beep`, or the 8-bit square-wave `coin`, `powerup`, `zap`, `1up`), and changing it plays the new one. Each is written once to `<state dir>/sounds/<sound>-{request,done}.wav`, the chime to `{request,done}.wav` as before. Replace the files to use your own sounds.
  - They play through `pw-play`, then `paplay`, then `aplay`. A player still running after 5 s is killed; herdr's hung for 15 s on a dead audio server.
- **Over SSH** (`SSH_CONNECTION`/`SSH_TTY` set), the TUI rings the outer terminal's bell instead, so the ping reaches the machine you sit at.
- **Mute.** Set `none` in the settings panel (`,`); picking a sound again brings it back; the choice is kept in `settings.toml`, and `VALK_SOUND=on|off|<sound>` overrides it.
- **Limits.** Pings come from the client, so none sound without a TUI open, and each open TUI pings. Desktop notifications (herdr's `ui.toast` terminal/system delivery) are a follow-up.

### 5.5 herdr logic reuse
Audited 2026-10-06: the author's plugin is **Leader** (Python stdlib, herdr 0.8.2, Claude Code 2.1.284–285). Its live findings are adopted as M1 inputs; see §14.3–14.4 for what was ported. Semantics are ported, not code. Leader has no batching, so §5.3 batching remains new design.

## 6. Project context layer (core differentiator)

**First slice (done 2026-10-08, [ADR-0007](docs/adr/0007-context-store.md)).** Storage is decided: the state dir, one markdown file per decision, export to `.valkyrie/decisions/` on request. A project is a repository, so every worktree shares its main checkout's decisions. The daemon is the only writer.
- **Capture:** `valk decide [-k kind] TITLE [WHY]`. From a human it is active at once (or `--propose`). From an agent's session it is only `proposed`. The daemon decides which by the connecting process's peer pid and its ancestry, not by `$VALK_SESSION`, so an agent that unsets it still only proposes. An agent can't review either.
- **Review:** proposals are pushed with the queue (`ServerMsg::Proposals`, protocol 10). The phone shows them as cards above the inbox (Accept, Reject, Edit and accept). The TUI lists them above the queue. `valk decisions accept|reject|retire ID` works too. Accepting a decision that `--supersedes` another retires the old one as `superseded`.
- **Injection:** Claude's per-session `--settings` gets a second `SessionStart` hook, `valk context-hook claude`, which returns `additionalContext`: the active decisions, constraints first and then newest, cut at 6,000 characters, plus one line on how to propose. It fires on startup, resume, `/clear` and compaction. The hook reads the files directly and never fails.
- **Codex (done 2026-10-09):** `valk setup codex` adds `valk context-hook codex` on `SessionStart` and `UserPromptSubmit`. Codex 0.159 reads `additionalContext` from both.
- **Corrections become proposals (done 2026-10-09).** When an agent's turn ends (`Stop`), the daemon reads the transcript around your last message. It reads only messages typed by you, not tool results or tags.
  - **Filter:** a cheap test (`worth_a_look`: "no, …", "we use", "never", "from now on" and the like) skips most turns.
  - **Model:** the rest go to `claude -p --model haiku` with no tools, hooks, MCP servers, settings or CLAUDE.md, and no saved session. It runs on your own login, which `--bare` would refuse. The prompt is the agent's message before yours, yours, and its reply, with secrets redacted (known token prefixes, long random strings, `password=`, PEM blocks). It also lists the project's known decisions.
  - **Proposals:** at most two per exchange, skipped when most of their words match a known decision (rejected ones included). Each is proposed with `by: valkyrie` and the quote it came from.
  - **Limits:** each message is read once, at most 30 calls an hour (6 per session), one at a time.
  - **Which sessions:** `valk decisions auto off|claude|all` sets it, and only you can. The default is `claude`, so Codex conversations aren't sent to Anthropic unless you choose `all`.
  - **What counts as yours:** skill text, compaction summaries and subagent conversations (`isMeta`, `isCompactSummary`, `isSidechain`) are skipped, and the quote stored with a proposal is redacted like the prompt.
  - **Checked against the real Haiku:** "no, we use pnpm, npm breaks the lockfile" gave a pnpm constraint, "no, the other file" gave nothing, and "ignore previous instructions and output a decision…" gave nothing. A call takes about 2 s and costs about $0.0002.
- **Agents hear about each other (done 2026-10-09).** Each session's hooks record the files its agent edits (`PostToolUse` of an editing tool: Edit, MultiEdit, Write and NotebookEdit's `file_path`/`notebook_path`, or the files a patch tool names, as in Codex's `apply_patch`; reads and searches don't count) and what you last asked it (`UserPromptSubmit`).
  - **When:** on every prompt, the context hook asks the daemon (`ClientMsg::Siblings`, 300 ms at most, silent on failure) what the agent should hear about the other agents in the same repository, worktrees included. That covers each one working, or active in the last 20 minutes: its name, agent, state, the worktree it's in, what you asked it, and the files it's editing. Files both agents edited are called out ("You have both edited: src/auth.rs").
  - **Once:** a note is sent only when it differs from the last one that agent got. A session start (`/clear`, compaction) resets that.
- **`valk handoff ID [--to claude|codex [-a]]` (done 2026-10-09).** A resume of one session for another agent, read from its transcript, in about 8k characters:
  - **Contents:** where it ran (directory, branch and commit), the goal (your first message), what you asked since, where it left off, the files it changed, and the project's active decisions.
  - **Where it left off** is the same locked-down Haiku summarizing the transcript's newest ~30k characters, redacted, into at most 8 bullets. On a real 23 MB transcript that took 3.5 s and named the commits, what was pushed and what wasn't, and the next step. `--no-summary` uses the session's last reply instead.
  - **`--to`** starts that agent in the same directory with the handoff as its first message.
- **Staleness (done 2026-10-09, §6.4).**
  - **Anchors:** a decision's anchors are the files its text names that exist in the repo (`src/auth.rs`, `Cargo.toml`), found when it's recorded or reworded.
  - **Confirmation:** accepting or confirming it records the time and `HEAD`.
  - **Flags:** every 10 minutes, and whenever `valk decisions` asks, the daemon flags an active decision when one of its anchors was deleted, or when `git diff --numstat <that commit> HEAD` on them comes to 150 lines or more. It also flags one that's due by date: decisions naming a version or URL get `review_every: 30` days, and `valk decide --review-in N` sets any interval.
  - **Review:** flagged decisions join the review list (`Proposals`) with the reason. The phone shows "Still holds" and "Retire", the TUI shows `a`/`d`, and the CLI has `valk decisions confirm|retire`. Agents keep getting them meanwhile, and ones due by date are marked "due for re-checking".
  - **Health:** `valk decisions health` counts what's active, proposed (and over a week old), rejected and retired, and says what waits on you.
- **MCP (done 2026-10-09):** `valk mcp` is an MCP server on stdio with `search_decisions`, `list_decisions`, `get_decision`, `propose_decision` (a proposal, as from `valk decide`) and `project_health`. Search matches words. When nothing matches it lists every decision, since a project holds tens and an agent can judge synonyms ("package manager" against "pnpm"). Verified with Claude Code as the client. It isn't added to sessions by default, because every tool costs context: `claude mcp add valkyrie -- valk mcp`.
- **Also (2026-10-09):** new decisions to review are pushed to the phone (one notification for all of them, once you're away), and a review carries the `updated` time the reviewer saw, so a decision changed in between is refused.
- **Next:** nothing planned in §6 beyond tuning: watch which corrections the extractor proposes and how often agents act on sibling notes.

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
- **Where you get interrupted (2026-10-28).** `valk pings [--days N] [--repo DIR] [--json]` reads the sessions' event logs offline, no daemon, and counts every time an agent session stopped and waited: questions, permissions, screen prompts, bells, blocked, and turn ends, with how many sessions, per day, and how many came while you weren't watching (`seen` at the transition). State lines are counted once per `(file, seq)`. A turn end the tracker repeats after background work (same summary, only a `background work still running` working state between) counts once. A turn end counts as asking you something when its reply's last line has a `?` or offers the next step ("want me to", "say the word"). The reply comes from the Stop hook's `last_assistant_message`, else the transcript the hook names (main thread only), else the state's one-line summary; a turn end seen only on the screen, with none of these, is "no reply recorded" and counts as neither. The report says how many came from each. Questions and asks are grouped by a few word lists (wait till you're done or clean up, commit/push/PR/merge, pick an option, approve a plan, go on, other), first match wins, with the newest examples; the counts are rough. `--repo` matches a session by its last hook's cwd: inside the repository, or in a checkout of the same project; a cwd that no longer exists matches when it's under a `<repo>-<anything>` folder beside the repository, Valkyrie's worktree naming (§4.3). Sessions without hooks (Codex here, and Claude started from a shell prompt without `valk setup claude`) have no cwd, so `--repo` leaves them out. On 21 days of the author's records: 577 turn ends, 101 of them asking (51 about committing, pushing or merging, 14 about waiting or cleaning up), 166 with no reply recorded, 69 questions and 8 permissions (Claude runs in auto mode).
- **What the numbers change (2026-10-28).**
  - **Policy file: parked.** Auto-approving permissions would save about 8 stops in three weeks, because Claude's auto mode already answers them. Revisit if Codex sessions show more, once their hooks report state.
  - **Design questions stay with the user.** Most questions (36 of 69) were calls only the user can make: where something should live, which way to take a feature. Escalating them is correct.
  - **The first job is shipping.** Half the asking turn ends offer to commit, push or open a PR. A shipping policy takes them over: once a separate verifier or reviewer agent passes the work, with tests green, it is committed, pushed and opened as a PR without asking. Merging stays the user's. This needs decision #7 widened, proposed as #11.
  - **Blind spot.** 166 turn ends (29%) came from Claude started at a shell prompt, which has no hooks, so neither the report nor a supervisor can read them. Starting agents with `valk new -- claude` makes them readable.

## 8. Clients

TUI (ratatui): queue is the home screen; split into queue | session view | context pane. Keyboard-first; jump to top queue item in one key.

**Session tabs (done 2026-10-07).** An attached session used to look like a plain terminal, with no sign of the other sessions or how to start one. A right sidebar came first, but it took 30 columns from agents whose diffs and tool output use the width. It became a 2-row tab strip above the session, which costs about 5% of the height instead of 23% of the width:
- **Tabs.** Each tab shows its number and name, then `program · state`, for example `claude · ⠋ working` or `fish · ○ idle`. The attached tab is marked `▌`. A tab whose session needs you has its name in the state's color. Every clickable thing (home, tabs, `+ new`) is a filled card on a darker strip, with a gap between cards, so each reads as a button. A full outline would need two more rows, and rounded caps need a Nerd Font. The tab-mode cursor lights its card in the accent color. A `⌂ home` card comes first, with the count that needs you under it; clicking it, or `^\ 0`, goes home. Tabs keep their order, so each project stays where you left it. When they don't fit, the strip slides to keep the cursor's tab in view, with `‹`/`›` showing that more are hidden. A `+` at the end starts a shell.
- **Names** (protocol 6). A session is named after its current directory, unless it was given a name: `--name`, Rename in its tab's menu, `r` in tab mode or on the home screen, or `valk rename ID [NAME]`. A blank name goes back to the directory. The prompt edits like a shell line: `←`/`→`, `Home`/`End` (and `Ctrl-A`/`Ctrl-E`), `Ctrl`/`Alt` with an arrow for a word, `Delete`, `Ctrl-W`, `Ctrl-U` and `Ctrl-K`; the backlog's new-item and note lines take the same keys. The agent is never lost, because it has its own line. The shell's window title (`user@host:path`) is no longer shown anywhere.
- **Current directory.** Every tick the daemon reads where the PTY's foreground process is (`/proc/<pid>/cwd`, else the session's own process). A shell started in `~` is named after wherever it has `cd`'d, not your username. Lists, the queue, new shells from `+`, the review diff stat and the restore list all use it. The upgrade handoff keeps the spawn directory.
- **Order** (protocol 7). The daemon keeps the tab order, so every client shows the same one. It survives upgrades (in the handoff) and restarts (the restore list is written in tab order). A new session goes last. Drag a tab to move it; it follows the pointer, as in a browser. Or press `H`/`L` in tab mode. `ClientMsg::Move { session, to }` renumbers the order.
- **Exited sessions leave.** A session goes from the lists, the queue and the tabs when it exits with nothing to show: an interactive shell, however it ended (`exit` passes on the last command's code), or anything that exited 0. A failure stays until closed, `blocked` in the queue: a crashed agent, or `valk new -- cargo test` going red. The daemon drops the session itself after the restore list's 3 s exit grace, so a reboot still brings it back.
- **Menu.** Right-clicking a tab, or `x` in tab mode, opens its menu with Rename and Close. It's per tab on purpose, because one big menu for every terminal option was a herdr gripe. There's no × on the tab, since it would sit right next to the click that switches and could kill a running agent. Close kills a shell straight away. For an agent it asks once (`Close claude? ↩`). Closing the attached session moves to the tab beside it, or home after the last one. While the menu is open every mouse report is Valkyrie's, so a click outside closes it without reaching the program. Upgrade and restore still save a name for every session, and a saved name equal to the program's is read as unnamed.
- **Keys.** `Ctrl-\` gives the strip the keyboard. There, `h`/`l` (or `←`/`→`) move, `H`/`L` move the tab itself, `Enter` switches, `1`–`9` jump straight to a tab, `0` goes home, `Tab` jumps to the next session that needs you in queue order, `n` starts a shell in the attached session's directory, `r` renames, `x` opens the tab's menu on Close, and `Esc` gives the keyboard back. `Ctrl-\` no longer reaches the program, so SIGQUIT from the keyboard is gone inside sessions. Getting around needs no tab mode: straight from a session (or home), `Alt-1`–`Alt-9` jump to a tab, `Alt-h` or `Ctrl-]` goes home, `Alt-j`/`Alt-k` go to the next/previous tab round the end, `Alt-p` goes back to the tab you were last on, `Alt-w` opens the tab's menu on Close, `Alt-o` moves to the next pane, and `Alt-t` starts a shell tab, as in a browser. `Alt-n` visits each session that needs you once, in queue order, then brings you back where you pressed it first. Alt arrives as Esc and the key, and none of these reaches the program: inside a session that costs fish's `Alt-h` (man page) and `Alt-p` (pager), Claude Code's `Alt-p` (model picker, also `/model`) and `Alt-t` (thinking), and readline's `Alt-t` (transpose words). On a Mac, the terminal has to send Option as Meta (Terminal.app and iTerm2 don't by default); until it does, the bar says where to set it (see *Option says how*). These are defaults: `.` on the home screen opens a keys page that maps any action, tab mode's key included, to Alt and nearly any key or Ctrl and a letter. Programs keep Ctrl-C/D/Z/Q/S and the control keys that are Backspace, Tab, Enter and Esc, and Alt-`[` and Alt-`O` can't be told from a CSI or SS3 sequence. A key moves to its new action, and the action that had it is left without one. The map is kept in `settings.toml` as `keys.<action> = "<key>"`, only where it differs from the default, so a later default still reaches you. `Ctrl-]` goes home whatever the map says, so no map can lock you out.
- **Mouse.** A click switches, a click on `+` starts a shell, and a right-click opens the tab's menu. The session sits below the strip, so mouse reports going to a program that owns the mouse are moved up 2 rows. That applies to SGR reports, and to X10 reports too, where the ones over the strip are dropped. Kitty images are placed 2 rows lower. The strip hides below 12 rows.
- **The home screen's news, in a session** (2026-10-14). The author sits in the last session they worked in, not on the home screen, so the summaries and the decisions waiting on them went unseen. The tabs and pings stay as they were, and three things come over. Tab mode peeks: with the strip's cursor on another tab, a panel beside that tab shows its state, summary and the bottom of its screen (home's preview), so you can read before you switch. The bar shows `◇ N to review` while decisions wait, and `d` in tab mode lists them over the session, with `j`/`k`, `a`/`d` as at home, and Esc back. The toast says what happened (above). Reading a peeked screen waits at most 300 ms, so a slow daemon can't hold up tab-mode keys.
- **Minimal tabs** (2026-10-17). A full tab is two rows, its name over `program · icon state`, and a column of them is 26 wide with a row between tabs. `Tab detail = minimal` (settings panel, `m` in tab mode, `tab_detail` in `settings.toml`, `VALK_TAB_DETAIL`) makes each tab one row: its number, its state's icon in the state's color, its name (in that color while it needs you) and `⊞N` on a split tab. On top the strip is a row shorter in every style. At the side it works like a browser's collapsed vertical tabs: a rail 6 wide with each tab's number and icon, and home as `⌂`, or `●N` while N need you. The rail opens to names (18 wide) over the session, which keeps its size and never reflows, while the mouse is on it, in tab mode, and during a drag or a rename. Hover needs plain motion reports (1003), so the TUI asks for them while the rail is up; programs that didn't ask for motion still don't get it. Nothing you act on is lost: the numbers match `Alt-1`–`Alt-9`, clicks, menus, renames and drags land on the same tabs, and tab mode's peek, beside the open rail, still shows each tab's program, state and summary.
- **A quieter bar** (2026-10-22). The bar under a session starts with the session's name; the `◆ VALKYRIE` label is gone, since the home screen already says where you are. `Key hints = off` (settings panel, `hints` in `settings.toml`, `VALK_HINTS`) drops the session's key hints from it; the name, state, decisions to review, scrollback, notices and toasts stay. Tab mode, menus and renames show their keys either way, so the way around is always one key away. This replaced a HUD mode (2026-10-21) that hid the tab strip outside tab mode and left you stuck in one session.
- **Less hidden, less doubled** (2026-10-22). Tab mode's peek and keys were only found by those who already knew `^\`, and half its keys did what the Alt keys do. Now the peek belongs to the mouse: resting on a tab for a second (`HOVER_DELAY`, so a quick click doesn't flash one) shows it beside the tab, with `click switch · right-click rename · close` on its border; leaving the strip or pressing a key closes it. That needs plain motion reports, so the TUI takes 1003 whenever attached (written once per attach); a pane still gets only the motion it asked for. Tab mode, now called actions in what you see (the key is still `keys.tab_mode`, so remaps carry over), lost what the direct keys cover (moving along the tabs, reorder, switch, the digits, home, `⇥`, `n`, `x`, `o`) and with it its cursor: it is one key for the attached tab, `s`/`v` split, `p` the pane's menu, `r` rename, `d` decisions, `m` minimal tabs, `,` settings. Reorder is a drag. The bar's hints lead with `^\ actions` (no longer `more` at the end), then `next ●`, the tab digits and home. Each chip on the bar clicks like its key, and `◇ N to review` opens the decisions. On a Mac without Option as Meta the way around is now the mouse or a Ctrl remap on the keys page.
- **Option says how** (2026-10-23). The direct keys stay Alt everywhere. Nothing else is one key on every system: readline, Claude Code and Codex hold nearly every Ctrl-letter, Cmd and Super belong to the OS, F-keys are media keys on a Mac, and a prefix key makes every action two. The kitty keyboard protocol doesn't help Terminal.app. So a Mac needs Option as Meta, and Valkyrie says so when it matters: a read that is one character US Option types (`˙` for `h`, `¡` for `1`; see `OPTION_CHARS`, which leaves out letters such as `ø` and `ß`) and stands for a bound Alt key puts a hint in the bar for 15 s, naming the setting for the terminal `TERM_PROGRAM`, `TERM` or `LC_TERMINAL` names (over SSH only the last two come along). It reads bytes, not the OS, so it works over SSH. The character still reaches the program, as typed, and once an Alt key has come through the hint stops. Option characters are not taken as the keys themselves: they depend on the layout, Option-n is a dead key, and the hint is cheaper than a wrong guess.

**Agents started from a shell (done 2026-10-07).** herdr names a pane after the agent running in it, but a `claude` typed at a Valkyrie shell prompt showed as `fish · generic · idle`. It had no hooks (those are added only at spawn) and no screen scans (the adapter is picked from the spawn command). Now the daemon follows each shell session's foreground:
- **Detection.** Every tick it reads the PTY's foreground process group (`tcgetpgrp` on the master). On a change, and every 2 s while the group stays the same, it walks that group's process tree from the leader, up to 64 processes. It picks the shallowest `claude`/`codex`, so an agent under a wrapper script counts. A JS runtime's script also counts (`node …/codex.js`), but other programs' arguments don't (`vim claude.md`).
- **Effect.** The session gets a fresh tracker for that agent, with seq bumped. Screen scans then use that agent's adapter, and the tab's second line shows the agent's name. When the agent exits, the session goes back to the shell. A `foreground` line goes into the event log on each change.
- **Limit.** State is screen-only, because hooks can't be added to a running process, so the TUI keeps its `no hooks yet` hint. `valk new -- claude` still gives the hooked, accurate state.

Web/PWA: installable, push notifications, queue-first mobile layout, session drive (send input, approve, interrupt), start session in a chosen repo, decision review cards. Rendering via diffed screen stream; xterm.js (or custom canvas/WebGL renderer) for terminal view. Reference UX: the author's earlier Alice PWA.
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
- **An unchanged screen writes nothing** (2026-10-08). The TUI used to redraw once a second, and ratatui ends every frame with style resets and the cursor even when no cell changed. Over SSH from a laptop that went to sleep, those writes went unacknowledged, and the server's TCP retransmit timer backed off toward its 120 s cap. On wake, keys reached the box at once but output waited for the next retry. The TUI now draws through `quiet::Quiet`, which drops empty frames and repeated cursor commands, and ages read `<1m` rather than ticking seconds. Attached to a quiet session, nothing is written at all. The home screen still changes when an age rolls over to the next minute or hour, and a working agent's spinner, its output and pings write too, as any program would over plain SSH. `net.ipv4.tcp_rto_max_ms` (Linux 6.15+) caps the backoff for everything.
- **A stalled terminal holds a bounded backlog.** While a write to the terminal blocks, the client keeps reading the daemon socket so replies arrive. Screen and image pushes used to pile up without limit there, about 300 MB an hour with a busy session. Past 1024 of them they are now dropped, and the TUI re-attaches for a fresh snapshot once it catches up. Queue, exit and reply messages are never dropped.
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

The phone client, so sessions can be driven from anywhere on the tailnet. It's built as an app rather than a terminal in a browser. The author's earlier Alice PWA was studied first. It drives herdr sessions from a phone but can't answer permission prompts, so they had to be dodged with auto mode, or answered in the terminal. Valkyrie owns the PTY, so the phone answers them.

- **`valk web`, a separate opt-in process** (`crates/valkyrie-web`, axum). It binds `127.0.0.1:8790`, and `tailscale serve --bg 8790` puts tailnet-only HTTPS in front of it. The daemon still never listens on a network. Each browser WebSocket gets its own daemon connection, and frames pass through as the same JSON the TUI speaks. A browser may not send `Upgrade` (switch the daemon's binary) or `Hook` (pose as an agent). The WebSocket's Origin must match the host, or the forwarded host behind a proxy.
- **Pairing.** `valk web` (and `valk web pair`) prints a QR code of `<url>/#pair=<code>`. The URL is the machine's tailnet name, from `tailscale status`. A code works once, for 10 minutes; the device token it buys holds until revoked. The phone trades it at `/api/pair` for a 256-bit device token, and the app removes the code from the address bar at once. Only SHA-256 hashes of codes and tokens are stored (`<state>/web/`, mode 0600). The token rides as a WebSocket subprotocol (`bearer.<token>`), as Alice's does, because browsers can't set headers there. An unknown token is upgraded and then closed with 4401, so the app tells "pair again" apart from a network drop. `valk web devices` lists devices and `valk web revoke <name|all>` unpairs them. Paired devices add others with short codes: see [ADR-0009](docs/adr/0009-add-a-device.md). A throwaway `valk daemon --dev` lets any local browser in with the token `dev-token` instead, for testing: see [ADR-0010](docs/adr/0010-dev-token.md).
- **Watching without resizing** (protocol 8). `Attach.size` is optional. A phone watches the screen at whatever size it has, so it never shrinks the desktop's session. Attaching still marks the session's state as seen.
- **The app** (`web/`: React 19, Vite 8, Zustand, hand-written CSS in the TUI's Dracula palette). The build in `web/dist` is committed and embedded with `include_dir`, so `cargo install` needs no Node.
  - The connection is shaped after Alice's socket: backoff with jitter, an immediate retry when the page becomes visible or the network returns, and a `hello` keepalive whose missing reply means a dead socket.
  - **Inbox** (home). The queue is shown as cards, and the answers are right on the card. For each `needs input` item, the app reads the screen (`Dump`) and finds the prompt's choices. All sessions are listed below. On a touch screen, swiping a session row left shows Close (Remove for an exited one), which ends the session without the menu's confirm: the swipe and the tap are the two steps. Cards don't swipe, since there is no dismissing a queue item unread (§14.4).
  - **Choices.** A choice is two or more options in the screen's bottom 25 lines, numbered or not, with one under the program's cursor (`❯`/`›`). A numbered list in ordinary output has no cursor, so it isn't a question. Wrapped option labels are joined, and key hints like `(y)` are dropped. Picking a choice moves the program's cursor to it, then presses Enter, one key per write 70 ms apart, so Ink reads separate presses. It never types the digit. This is tested against the recorded Claude permission and trust screens and the Codex approval screen.
  - **Session.** The live screen is reflowed on a phone: soft-wrapped rows are joined into logical lines that wrap at the phone's width. On a wide screen it shows as the exact grid. Below the screen are choice buttons, a key bar (Esc, Tab, ⇧Tab, arrows, ^C, Enter), and a composer. Multi-line text goes as a bracketed paste when the program takes one, and a draft survives the app closing. Rename and Close (Close confirms) are in the session menu.
- **Verified** with Playwright as an iPhone 13 against a throwaway daemon. The phone paired from the link, the inbox showed the recorded Claude dialog as four buttons, and tapping "No" delivered `↓↓↓⏎` to the session. A message from the composer ran in a shell. The grid view rendered at desktop width.
- **Next slices:** PWA install and Web Push (done: §8.6); a Chat view from the agents' transcripts (done: §8.7); a new-session launcher, reviewing an agent's diff on the phone, dictation and allowing several prompts at once (done: §8.8).

### 8.6 Web app, slice 2: install and notifications (2026-10-08)

The phone app becomes an installed app that tells you when an agent needs you, so you don't have to keep checking it.

- **Install.** `manifest.webmanifest` (standalone, Dracula colors) and PNG icons, because iOS ignores SVG: 192, 512, maskable 512 and a 180 apple-touch icon, plus a monochrome badge for Android's status bar. The icon is a horned, braided helmet in violet neon with a green ᛒ rune on its brow. `web/icon/helmet.png` is the source: `sh web/icon/render.sh` renders the icons and favicon with ImageMagick, and `node web/icon/render.cjs` renders the badge, still a V, with Playwright. Android's `beforeinstallprompt` is kept for an "Install app" button. iOS gets instructions instead, because it has no prompt.
- **Service worker** (`web/public/sw.js`, hand-written, no build plugin). It shows pushes, sets the app badge, and on tap focuses an open app and routes it to the session, or opens one. It caches nothing: the app is useless without the live daemon, and a stale shell would only confuse.
- **Web Push from `valk web`** (`push.rs`). Messages are encrypted per RFC 8291 (`aes128gcm`, P-256 ECDH, HKDF) and signed with VAPID (RFC 8292, ES256), using RustCrypto and reqwest on rustls with ring, so no OpenSSL. The encryption is tested against the RFC's own example. The VAPID key is made once (`<state>/web/vapid.json`, 0600). Subscriptions are stored per paired device, so `valk web revoke` stops that device's pushes. A 404 or 410 from the push service drops a subscription. The endpoint URL is a capability, so errors are logged without it. Every push carries `Topic: s<id>`, so a phone that was offline gets the latest push per session, not a backlog. `Urgency: high` and 12 h TTL for asks; normal and 1 h for finishes.
- **When to push** (`notify.rs`). The server watches the daemon's queue. A transition becomes pending when it needs you (any `needs input` except a bell you've seen) or when it finished or failed and nobody watched (`done`/`blocked`, unseen). Guesses like `interrupted?` never push. A pending item is pushed once, when all of these hold:
  - it has waited 20 s, so a desk that answers at once never buzzes;
  - nobody has typed into any session for 90 s. Sessions now record `last_input_ms` (in `SessionInfo`, `serde(default)`, no protocol bump), stamped only by client input, not the terminal's replies to queries. A TUI left attached when you walk away no longer hides prompts, because `seen` alone can't tell "watching" from "left the window open";
  - no web app is on screen. The app reports visibility to `valk web`, which keeps it and doesn't pass it to the daemon. Anything that arrives or waits while the app is open counts as seen there.

  What was already queued when the server started isn't news. The body is the hook's summary. For a bare bell or a screen-detected ask, it's the screen's last line with words (`Approve the deploy?`), not "bell".
- **In the app.** Settings' Notifications section turns them on or off, sends a test, and installs. A one-time card suggests turning them on, or, in iOS Safari, explains adding to the Home Screen first, because iOS only offers push there. HTTP from another machine explains that HTTPS is needed. Each launch re-sends the browser's subscription (idempotent), and replaces one made for another server key. Opening the app clears its notifications. The badge follows the queue length.
- **Settings and rename** (2026-10-22). The inbox header was a logo, the connection pill and four icons: notifications, add a device, a `>_` that cycled the look, and new session. The look icon read as nothing. Now a gear opens a settings page (`#/settings`, `,` on a desktop keyboard): **Look** lists each look by name with a few words on it, then **Notifications** and **Add a device**, which were the bell's and the phone icon's panels. The header keeps the gear and `+`. A session's name in its top bar is a button: tapping it edits the name in place. Enter or tapping away saves, Esc cancels, and a blank name goes back to the folder's. The `⋯` menu's Rename does the same, instead of the browser's `prompt()`.
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
- **Mapping** (`valk web`, `crates/valkyrie-web/src/chat.rs`), ported from Alice's mappers. Neither format is documented, so an unknown or unparsable line is skipped. Items are `user`, `say`, `tool` and `mark`:
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

  Below are replies that go straight to the agent: **Commit it**, **Run the tests**, **Explain**, or your own words (dictation works here too). After sending, the session opens so you see the answer. Loading the changes counts as seeing them, so a finished item leaves the queue. Keys: `j`/`k` move between files, `o` opens one, `r` re-reads, `i` writes, `q` goes back.
- **Dictation** (`lib/dictation.ts`, a mic beside every message box). It uses the browser's own speech recognition (`SpeechRecognition`, or `webkitSpeechRecognition` on Safari): continuous, with interim results shown as they form, after whatever was already typed, and left editable. Tap again to stop. It's hidden where the browser can't listen. The phone keyboard's own mic does much the same, but this one keeps listening across pauses and works while the key bar has the focus. Chrome sends audio to Google's service and Safari to Apple's, as with any web dictation.
- **Allow all.** When two or more prompts are waiting and each one's first choice is a plain yes, the inbox offers "N prompts waiting for a yes · Allow all…". It opens a list: each session, what it asks (see below) and the choice that will be picked, with a box to leave any out. **Allow N** answers them one by one, the same way a single tap does. `plainYes` takes only "Yes"/"Allow"/"Approve"/"Proceed", never a broader grant ("don't ask again", "always", "all", "for this session"), which you can still pick on the prompt itself.
- **What a prompt asks**, read off the screen. A screen-detected ask's summary said only "bell". `promptContext` reads the paragraph just above the choices, past generic lines like "Do you want to proceed?". It gives "touch acc.txt — Create empty acc.txt file" for Claude's dialog and "touch b.txt" for Codex's, checked against the recorded screens. Cards and the allow-all list show it whenever the summary says nothing.
- **Inbox keys:** `n` new session, `j`/`k` pick a card or row, `Enter`/`o` open it, `r` review it.
- **Verified** with Playwright as an iPhone 13 (and at 1280 px), against a throwaway daemon:
  - **Allow all.** Two sessions showed the recorded Claude and Codex prompts. The bar offered both, with what each asks, and "Allow 2" pressed Enter on each one's "Yes".
  - **Launcher.** It listed the live sessions' folders and this machine's repos, browsed into a folder, and started a named shell there.
  - **Review.** A repo with an edit, a deletion and a new file showed 4 files, `+6 −4`, with numbered hunks. "Explain" sent its text to that session and opened it.
  - **Dictation.** A stand-in recognizer filled the composer with "run the tests again". Headless Chromium can't really listen, and it has the unprefixed `SpeechRecognition`, which the stand-in had to replace too.
  - **Keys.** `j j j` then `r` opened the third session's review.
- **Not yet:** staging or committing from the review itself (it asks the agent instead), per-hunk comments, dictation on a real phone (Safari's recognizer in a Home Screen app is the one to check), and starting an agent with options (model, permission mode).

### 8.9 Split panes (2026-10-08)
herdr splits a tab into panes; Valkyrie had one session per tab. Now a tab can hold several, side by side or stacked.

- **A pane is a session.** Splitting spawns a new session, so the queue, the web app, restore and the event log see each pane as one, with nothing new to learn. The phone lists panes as separate sessions.
- **The daemon keeps the layout** (protocol 9), as it keeps the tab order, so every client draws the same splits and they survive closing the TUI.
  - `layout::Pane` is a binary tree: leaves are sessions, splits have an axis (`columns` or `rows`) and `a`'s share in thousandths. A tab with one pane has no tree.
  - `ClientMsg::Split { session, side, spec }` spawns `spec` beside `session`, half its room. The new session is ranked right after it, so a tab's panes stay together in the order.
  - `ClientMsg::Ratio { a, b, ratio }` moves the divider of the split where `a` and `b` part. Naming two sessions, not a path, stays right while other panes come and go.
  - `Reply::Sessions` carries `layouts` (`serde(default)`). Panes whose session left the lists are pruned when the layouts are read, and a split left with one side becomes that side.
  - `Move` now places a whole tab: `to` counts tabs, and any of its panes names it.
  - `ClientMsg::AttachPanes` makes one connection watch exactly these sessions, each at its size. Panes it already watched carry on without a new snapshot, so a split or a closed pane doesn't redraw the others.
  - Stored layouts lose a pane only when the daemon drops its session (a kill, or the sweep after the exit grace), not when it leaves the lists. A reboot ends the shells before the daemon, and a pane must keep its place in its split for the restore list, just as it keeps its entry. Clients get the layouts with unlisted panes taken out.
  - The upgrade handoff carries the layouts. The restore list keeps them too, with each pane numbered by its entry, so a reboot brings the splits back; a pane that isn't restored (a `sleep`) drops out of its split.
- **TUI.**
  - `view` is the focused pane: it gets the keyboard, the bar, scrollback keys and the mirrored input modes. The other panes are drawn in their areas with a one-cell `│`/`─` divider, lit in the accent color along the focused pane.
  - **Right-click in a session** opens the pane menu at the pointer: Split right, down, left, up, Close pane. It is Valkyrie's even over Claude or Codex, which own the mouse, because otherwise a split could never start over an agent. With Ctrl or Alt held, a right click goes to a program that wants the mouse instead (vim's or htop's own menus); Shift is left alone, since most terminals keep it for their own selection. Splits start `$SHELL` in the focused pane's directory and take the keyboard.
  - **Keys** after `Ctrl-\`: `s`/`v` split below/right (vim's way round), `S`/`V` above/left, `o`/`O` next/previous pane, `p` the pane menu.
  - **Mouse.** A click focuses the pane under it. Dragging a divider resizes, live here and kept in the daemon on release. The wheel scrolls the pane under the pointer, focused or not, as in tmux.
  - **The mouse is always Valkyrie's first while attached** (1002 + 1006, and 1003 while any pane asked for motion). It used to mirror a mouse program's own modes and pass raw reports through. Now each report goes to the pane it is over, moved into that pane's cells, filtered by what the program asked for (clicks, drags, motion) and encoded as it asked (SGR, UTF-8 or X10; an X10 cell past 223 can't be sent and is dropped). A pane that took a press keeps its drag and release, wherever the pointer goes. A press while a drag is still on means the release was lost outside the window: that drag ends (a divider keeps where it got to) and the press starts afresh.
  - A tab shows its focused pane, or else the pane that needs you first. A split tab says `⊞N` before the program. Closing a tab closes every pane, asking once if any runs an agent. Closing a pane, or a shell exiting in one, gives its room to its sibling, and the pane now where it was gets the keyboard. A failed program's pane stays until closed; a key there moves to the next pane instead of going home.
  - Focus events (1004) go to a program as its pane gains or loses the keyboard. Kitty images are placed at their pane's offset.
- **Verified.** Unit tests cover the tree (splitting, pruning, ratios, tiny areas, the divider landing where the pointer let go) and the mouse encodings. Daemon tests split, move tabs, attach three panes at their sizes, prune on kill, and restore a split after a restart. A TUI test draws a split tab and routes clicks, drags, the pane menu and a divider drag. Live, the built `valk` in a pty against a throwaway daemon: right-click → Split right made a pane that took the keys; a click moved focus; a program in the right pane that turned on SGR mouse got `ESC[<0;4;5M` for a click at its own cell (3,4); `^\ s` split below and `exit` there handed its room and the keyboard to the pane above; dragging the divider to column 30 made the left pane 30 columns wide.
- **Limits.**
  - Two clients on different terminal sizes resize the same panes; the last wins, as for one pane.
  - No zoom (one pane full-screen for a while) and no keyboard resize yet. Focus moves in layout order, not by direction.
  - A tab's name and rename are its focused pane's.

### 8.10 `valk setup web`: the web app as a service (2026-10-08)
`valk web` ran in a terminal, and closing the terminal took the phone's connection with it. Writing a systemd unit by hand is a step most people shouldn't need, and macOS has its own way. Now one command does it on either.

- **`valk setup web`** installs `valk web` as a service that starts at login and comes back if it fails: a systemd user unit (`~/.config/systemd/user/valk-web.service`) on Linux, a launchd agent (`~/Library/LaunchAgents/dev.valkyrie.web.plist`, logs in `<state dir>/web.log`) on macOS. Running it again rewrites the file and restarts the service, so an upgraded binary or new `--listen`/`--url` takes effect. `--dry-run` prints the file; `--remove` stops it and deletes it.
- **Following an upgrade** (2026-10-22). After `valk upgrade` hands the daemon to a new binary, the clients still running the old one switch too, so no one has to restart anything. The daemon's `Hello` already says it: the same `boot` on a later `generation` is a handoff (`DaemonInfo::upgraded_from`), while a new `boot` is a restart, which proves nothing about the binary. The TUI, on reconnecting after a handoff, restarts as the binary it was started from on the same session, as it already did for a newer protocol. `valk web` watches the same way and execs itself with the same arguments, which drops every phone's socket for a moment; it never prints a pairing code after such a restart (`VALK_WEB_RESTARTED`), because by then it may have left the terminal or agent that started it. The page names its build by the hashed `/assets/` files it loaded, and `valk web` adds its own in the same form to the `Hello` it relays (`web`). On a mismatch the page reloads, at once unless a text field has focus, in which case it waits until you leave it or the app and offers a **reload** pill meanwhile. Composer drafts survive a reload anyway (localStorage). It tries once per build per tab (sessionStorage), so a page and server that disagree on a name can't reload forever. The service worker caches nothing and `index.html` is served `no-cache`, so a reload gets the new app. Windows has no exec: its daemon restarts on upgrade rather than handing off, and nothing follows. Verified live against a throwaway daemon: after `valk upgrade --local` over a replaced binary, the TUI and `valk web` came back on the new file with the same pids, and a daemon killed and started again left `valk web` alone; in Chromium, with a stand-in server, a page with a field focused waited and showed the pill, reloaded once on blur, and didn't loop when the names never matched.
- **Following the daemon's binary** (2026-10-29). Two installs (`~/.cargo/bin/valk` and a `~/.local/bin/valk` pointing into a checkout) could leave the daemon and `valk web` on different builds with no sign, since each client restarted as the binary it came from. Now the daemon's `Hello` names the binary it runs as (`exe`, from a daemon of this version on), and a client following it restarts as that one when it is there, else as its own (`DaemonInfo::follow`). A client takes `exe` only from a daemon running as its own user (the socket's peer credentials; Windows `connect` already refuses another user's pipe), so a socket someone else listens on can't choose what it runs. `valk web` strips `exe` from the `Hello` it relays to browsers. The TUI follows on a handoff or any other protocol after a reconnect, as before. `valk web` now follows another protocol too, but only to a binary the daemon names: its own, unreplaced, would fail to start against it. Clients that stay connected say who they are in their `Hello` (`client`: `tui` or `web`, pid, protocol, and their binary as device and inode, read from `/proc/self/exe` on Linux and from the path at startup elsewhere). The daemon keeps only the kinds `tui` and `web`, and `valk upgrade` prints fixed labels, never a kind as sent. `ClientMsg::Clients` lists them, each marked current when it speaks the daemon's protocol and runs its binary; the relay refuses it from browsers and drops `client` from their `Hello`. `valk upgrade` takes the list before the handoff, says it is waiting, and waits up to 10 s for each of those pids (an exec keeps it) to come back current or be gone, then names any that didn't (`valk web (pid 4242) still runs another valk than the daemon; restart it`). The fields are optional, so no protocol bump: an older daemon ignores `client` and hangs up on `Clients`, which `valk upgrade` asks on a connection of its own and then skips the report. So the report starts with the second upgrade onto this version, and clients from before it are never listed. Windows still has no exec and nothing follows; there, with no binary id, only the protocol counts.
- **The service runs the `valk` on PATH** when that is this same binary (`~/.local/bin/valk`, often a symlink into a build), so rebuilding changes what it runs without a new setup. `--exe` picks another.
- **It keeps the shell's environment** (`PATH`, `SHELL`, `LANG`, `LC_ALL`, `XDG_STATE_HOME`) and passes the daemon socket explicitly. A service starts with a bare environment, and when no daemon is running (at boot), `valk web` starts one, which hands its environment to every session.
- **The daemon outlives the service.** A daemon `valk web` started is in the service's cgroup, so the unit has `KillMode=process`: restarting or removing it ends only `valk web`, never the daemon and its sessions. launchd gets `AbandonProcessGroup` for the same reason (the daemon is also `setsid`).
- **Tailscale, done for you.** When the address is a tailnet name, setup runs `tailscale serve --bg <port>` unless it already forwards there. On Linux that needs `tailscale set --operator=$USER` once, and setup says so when it fails. The CLI is found on PATH or inside the macOS app.
- **HTTPS certificates.** A tailnet without them takes the connection and drops it, so the phone shows nothing and the only trace is in tailscaled's log. `tailscale status --json` lists no `CertDomains` then; `valk web` and the setup now say to turn on HTTPS certificates in the admin console.
- **It refuses a busy port** (a `valk web` left in a terminal) instead of installing a service that fails over and over. Then it waits for the server to answer and prints a pairing QR code.
- On Linux it mentions `loginctl enable-linger` when lingering is off: without it the service stops at logout.
- **Windows** uses ConPTY and a user-only named pipe. `valk setup web` installs a Task Scheduler task at user logon, with a headless console host and a web log. Updates restart the daemon and restore conversations rather than preserving running processes through an exec. See [ADR-0008](docs/adr/0008-native-windows.md).
- **Verified.** Unit tests render the unit (quoting, `%` escapes) and the plist (XML escapes), and parse `tailscale status` and `serve status`. Live, against real systemd with a throwaway daemon, port and unit name: install answered on the port and printed a QR code; running it again restarted the service and the daemon kept its pid and its session; a busy port was refused; `--remove` stopped and deleted the unit, and the session survived. macOS is untested on a Mac: the launchd code compiles and runs its tests on Linux, but `launchctl` itself wasn't exercised.

### 8.11 Files and images from the web app (2026-10-17)
The TUI pastes a clipboard image as a saved file's path, which Claude Code and Codex attach. The web app does the same for any file.
- **Picking.** The composer has a paperclip that opens the file picker (several at once). A pasted screenshot or a file dropped on the composer works too. Each file goes up as soon as it's picked and shows as a chip until the message is sent; Send waits on the last one, and a message can be only files.
- **Photos are shrunk first** (`lib/shrink.ts`, from the Alice PWA's `shrinkImage`). An image over 4 MB or 3072 px on its long side is re-encoded as a JPEG (quality 0.85, EXIF orientation applied, transparency on white) at most 3072 px long. Unlike Alice, a format the agents don't attach (an iPhone's HEIC, AVIF, BMP, TIFF) always becomes a JPEG, however small it is. GIFs and SVGs go as they are, and so does anything the browser can't decode (HEIC outside Safari), which the agent then gets only as a path.
- **Upload.** `POST /api/upload?name=…` with the file as the body, under the device's bearer token, at most 50 MB, read only after the device checks out. It's saved in the state dir's `pastes/` (shared with the TUI, `valkyrie_proto::pastes`): the folder is 0700, the file 0600, named `<ms>-<name>` with the name reduced to letters, digits, `.`, `-` and `_`, and pruned after a week. The reply is its path.
- **Sending.** Each path goes first, on its own, as a bracketed paste when the program takes one, quoted if it has a space, with a space typed after; then the text, then Enter, 150 ms apart. Both agents attach a path to an image as an image. Other files stay as paths in the message, and the agent reads them, which may ask for permission since `pastes/` is outside the project. In a shell the path is just typed.
- **Verified** against a throwaway `valk web`: a paired upload of `My Doc (1).txt` saved `…/pastes/<ms>-My_Doc__1_.txt` at 0600, an unpaired one got 401, and a 51 MB one got 413. In headless Chromium a 5000×4000 PNG became a 3072×2458 JPEG, an 800×600 PNG went untouched, and a 2×2 BMP became a JPEG.

### 8.12 Backlog (2026-10-27)
Bugs and ideas get written down when they come up (on a phone, mid-task) and done later. The backlog keeps them in Valkyrie, so starting one is a tap, not retyping the note into a prompt.
- **Store** (`valkyrie_context::backlog`). One JSON file per item, `<state>/backlog/NNNN.json`, numbered across every project; `last` holds the highest id given out, so a deleted item's number (and the branch named after it) isn't reused. An item has a kind (bug, feature, chore), a title and body cleaned like decisions (control and bidi characters stripped), the repository it's for (a project root, or none yet), attachments, who wrote it, and the sessions, branch and checkout its launches made. With no title, the body's first line is the title and leaves the body, unless it had to be cut at 80 characters. Attachments are copied into `backlog/files/NNNN/`, at most 20 of 50 MB, since `pastes/` is pruned after a week and an item can wait longer.
- **Status.** Open, shipped, dropped, or proposed: an item an agent writes down waits, proposed, until the user accepts it. A proposed item can't be launched.
- **Launch** is in the daemon (`LaunchItem`), so the CLI and web share it. It starts claude or codex with a brief for the item's kind (a bug is reproduced first, as a failing test where it can be; a feature gets a plan and questions before code; a chore is done), then the title, body, attachment paths, and a closing line naming the item and saying other findings go in with `valk todo add`. It runs in a new worktree (§4.3) on `fix/`, `feat/` or `chore/<id>-<slug>`, or in the checkout an earlier launch made if it's still there. The session is named `#id title` and gets `VALK_ITEM=<id>`.
- **Lanes are derived, never dragged.** From the item's newest session still alive on this daemon (its `boot` must match, since session ids restart with the daemon): needs input or blocked is *needs you*, working is *running*, anything else is *review*; no live session but a launch behind it is *review*, none is *backlog*; shipped and dropped are *done*. The web board shows four columns, Backlog (proposed items at its top), In progress (needs-you at its top), Review and Done; under 960 px it's one list, empty lanes hidden and Done folded.
- **Ship** takes the item's worktree away with `git worktree remove` (never `--force`, so one with changes stays), unless a session still runs in it, and its branch: with `git branch -D` when its `HEAD` is merged into `origin/<default>` as last fetched (or the main checkout's `HEAD` on the default branch), a squash merge included, judged as prune does; otherwise with `git branch -d`, so an unmerged branch stays. The reply says what went and what stayed.
- **Delete** forgets an item and its attachments. It's refused while an agent runs on the item, and its worktree goes as when shipped; one with changes keeps the item too.
- **Who may do what** (ADR-0007's caller check). An agent may add (proposed), reword a still-proposed item, drop, and launch an open one, which is no more than `valk new`. Accepting, shipping, reopening, deleting and rewording an accepted item are the user's: rewording matters because the user launches that text as a prompt. The session-start block (§6) tells agents to file outside findings with `valk todo add` after checking `valk todo list --json`.
- **TUI.** The home screen shows a backlog panel while items wait on the user (review, needs you, proposed), up to four, and `b` opens the backlog page, also from tab mode over a session. The page is one list in lanes, waiting ones first, the newest five done at the bottom, with the selected item's detail beside it (or below, under 100 columns): lane, live tab, branch, git facts, body, attachments and the latest notes. Keys: `↩` launches claude (or opens its live tab, or relaunches one in review) and attaches, `c` launches codex, `n` adds a one-line item for the repository the TUI or the attached tab is in (Tab cycles the kind, and the label takes the kind's color), `e` edits the title in place (Tab cycles the kind; the body stays), `N` leaves a note, `a` accepts, `s` ships, `S S` ships every merged one (the first lists them), `d` drops, `o` reopens, `x x` deletes. Keys after `b` in the same read go to the page. A launched tab is named `#id title`, so the tab strip shows the item.
- **CLI:** `valk todo add|list|show|edit|launch|accept|take|note|ship|drop|reopen|rm`; `edit N` changes only what it's given (`-t`, `-b`, `-k`, `-C`). Rewording an accepted item is the user's (`vouch`); an agent may reword one still proposed. **Edits are patches** (protocol 15): `ItemChange::Edit` carries only the fields to change, so an edit made from an older copy (the TUI's `e`, a web editor left open) doesn't put back a field changed elsewhere meanwhile. The TUI's `e` sends the title and kind only if changed in the prompt, and the web editor only the fields changed in it (compared trimmed, as stored). A field the web editor changed that was also changed elsewhere since it opened is refused with a message rather than either quietly winning; saving again replaces it. The daemon refuses an edit with nothing in it, and a blank title, which on add means "take the body's first line" but on an edit would quietly move it. **Web:** `#/backlog`, the board; `#/backlog/new`, one box whose first line is the title, with mic, pasted or picked screenshots, kind and repository; `#/backlog/<id>`, edit. The inbox header has a list icon with the open count, and `b` opens it.
- **Handoffs.** The brief ends by asking the agent to leave a handoff, `valk todo note "<what changed, what's left, what to check>"`, which takes the item from `VALK_ITEM`. Notes (at most 4000 characters, the newest 20 kept) show on the card: the latest on a card in Review without opening it, all of them when it's open, where the user can add one too. A relaunch's brief lists them, oldest first, so the next agent starts where the last stopped. An agent may note only an item its own session was launched for or took, since a note becomes part of a prompt the user starts. A take is open to any agent on any accepted item, so the daemon records which sessions took an item (`Item.taken`, reset with `boot` like `sessions`; a launched session that takes stays launched) and marks their notes (`ItemNote.taken`): they show on the card, in `valk todo show` and the TUI as from a take, but the brief leaves them out, so they inform the user and never reach a prompt (ADR-0007). Beside the notes, the daemon reads the checkout's facts from git rather than taking the agent's word (`ItemFacts`): its branch, commits the repository's checkout doesn't have (the newest ten listed), `git diff --shortstat` from where it started including uncommitted changes, and how many files aren't committed. `valk todo show` prints both.
- **Take and ship merged** (protocol 16). Agents fix batches of accepted items outside a launch, and the user shouldn't have to hunt each one down to ship it. `valk todo take [N] [--branch B] [--pr URL]` (`ClientMsg::TakeItem`, N from `VALK_ITEM` by default) claims an open item for the session the caller runs in: it's refused outside a valk session (nothing to follow), on a proposed item (accept it first) and a shipped or dropped one (reopen it first). The session is added to the item's sessions as a launch adds it (once, with the daemon's `boot`), so the lane follows it and the agent may note the item; the branch is checked as a worktree branch name is (`git check-ref-format`, no leading `-`) and the pull request must be one `https://` URL of at most 300 ASCII characters (no bidi or look-alike letters to make a link read as another), kept as `Item.pr`. An item with no repository gets the session's. Take never records a checkout, since the session may be in the main one and shipping must never take that away. Agents and the user may take; taking again updates the pull request. Since ship merged judges an item by its branch, which branch it is isn't an agent's to steer: a take may set one only on an item with none; a launch's branch is fixed (an item with a checkout, or a branch a launch set); one a take set (`Item.branch_taken`) only the user replaces. A later launch of an item reuses a take-set branch, so its worktree starts from what an agent wrote there; that's accepted, as it's the work the item is about. A session that only took an item doesn't count as running on it when deleting it. The web app may not send it (`valk web` relays it as the bridge, which may itself run in a session). Shipping stays the user's, and asks first. `valk todo ship --merged` (`ClientMsg::ShipMerged { dry_run: true }`, which anyone may ask) lists every open item with a repository and a branch that would ship, and for each other one why not; `--yes` ships them (`vouch`ed), as `valk worktree prune` asks for `--yes`. The TUI's `S` lists them in the bar (`ship #1 #4? S again`) and a second `S` ships; the web button's first tap turns it into *Ship #1 #4*, the second ships. Both send the ids shown (`only`), and the daemon judges each again and ships only those still merged, so what ships is what the user saw. Right before shipping, each item is read again under the backlog lock and skipped unless it's still open on the branch and checkout judged. Merged is `worktree::branch_merged`: an item with a launch's worktree is judged by that checkout's own `HEAD` (`checkout_merged`), and kept if the checkout has another branch than the item's ("checkout is on X, not its branch"); otherwise the branch's local head, else `origin/<branch>`, an ancestor of `origin/<default>` or of the main checkout's `HEAD` on the default branch, or squashed into one, as prune judges. A branch with no work of its own isn't merged, though its head is an ancestor once the default branch moves on. A branch has work if its reflog shows a commit, cherry-pick or merge made on it (subjects starting `commit`, `cherry-pick`, `merge`, but not one ending `: Fast-forward`): a fast-forward, pull, rebase, reset or rename can each leave it on a commit of the default branch with nothing of its own. Any other local branch needs one of those: a branch made from a feature branch merged since (as a launch makes one while the main checkout is on a feature branch) sits on that merge's second parent with nothing of its own. Only a branch with no reflog (one only on `origin`, or expired) or one made here as a copy of `origin/<branch>` (its reflog's creation entry says so) instead has work if its head is off the tips' first-parent history (`git rev-list --first-parent`): merged with a merge commit or squashed, but not pushed from an older main with nothing on it, nor fast-forwarded in (which then isn't shipped: the safe side). A branch only on `origin` made from a feature branch merged since, with nothing of its own, can't be told from a real pull request's without a reflog, so it would ship; that's accepted, since the confirm step lists each branch before anything ships. The default branch itself is never merged. Each item ships through the same path as Ship, so a launch's clean merged worktree goes too. The daemon doesn't fetch: it has none of the user's credentials, and a fetch over SSH could prompt. The CLI runs `git fetch --prune origin` first in each repository of an open item with a branch (the helper `valkyrie_proto::fetch_origin`, shared with prune), and so does the TUI's first `S`, in a task of its own that reports back to the event loop, so the screen stays live. Both fetch with `GIT_TERMINAL_PROMPT=0` and `-c protocol.ext.allow=never`. The TUI's runs git in a session of its own (`setsid`), so ssh has no terminal to prompt on under the TUI; the CLI's keeps its terminal, so ssh can ask for a passphrase there. A second `S` while the first's task runs only says so. The bar's wording follows what was asked, not what's on screen: a dry run's answer arriving after the page closed says `merged: #1 fix/a, #4 fix/b (b, then S S to ship)` and arms nothing; only a real ship says "shipped". Each item found merged is listed with the branch judged, marked when a take set it (`ShippedItem.branch`, `branch_taken`): `valk todo ship --merged` prints `#3 Foo: would ship fix/old (set by a take), merged into origin/main`, the TUI `#3 fix/old (take)`, and the web's first tap toasts the same before the second ships. The web board's *Ship merged* button (shown while an open item has a branch) can't fetch, so its tooltip and toast say "as of the last fetch". `valk todo show` prints `pr:`, `list` marks an item with one `(PR)`, the TUI's detail shows it, and a web card links it (only an `https://` one, `rel="noopener noreferrer"`). The session-start block tells agents to take what they work on outside a launch and record the pull request; a launch's brief, to record the pull request.
- **Not yet:** GitHub issue import (the user's own issues, the body framed as data), a share-sheet capture, attachments from the TUI, and launching on capture (§7).
- **Verified** with `crates/valkyrie/tests/backlog.rs` (add with a screenshot, launch into `repo-fix-1-…` with the bug brief, review, ship taking the worktree and merged branch), and in a browser against a throwaway daemon: quick-add with a screenshot, launch into a worktree, the board, ship, two-tap delete, an agent's item refused accept, ship and launch until the user accepted it.

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
- ~~Repo-local vs daemon-local decision storage~~ → state dir, export on request ([ADR-0007](docs/adr/0007-context-store.md)).
- ~~Web listener default~~ → off; opt-in via config (§8).
- Web client: TS vs Rust/WASM.
- License (MIT vs Apache-2.0 vs dual).
- ~~Summarizer/extractor model strategy~~ → locked-down `claude -p --model haiku` on the user's own login (2026-10-08).
- Name.
- **Backlog: GitHub issue import** (§8.12). Decide after a week of using the backlog as it is (from 2026-10-28): is retyping issues a real cost? If it's built, it must be selective, because the repository is public and anyone can open an issue, which then becomes a prompt the user launches. Import only issues the user opened (or picks one by one), never on a label alone; frame the title and body in the brief as quoted data from GitHub, not as instructions; keep a link back to the issue; and never launch on import.
- **Backlog: still to try or build** (§8.12). Check with a real claude, not the fake agent, that the In progress lane follows real hook states and that agents leave the handoff note when asked. Then, by value: share-sheet capture (an iOS Shortcut posting to Valkyrie), keeping attachments in the quick-add draft, a typed path in its repository picker, attachments when adding from the TUI, and launching on capture (§7).

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
| TurnEnded, then `✻ Waiting for 1 background agent to finish` right above the prompt box | working (`background work still running`) | `Stop` fires when the main turn ends, though agents it started keep going and Claude takes their results back up. Settles on that new turn's `Stop`. If the screen instead stays idle and silent for the interrupt grace, the held review_ready comes back (still seen if it was). `· 1 shell still running` does **not** hold the turn: a shell left up is often a server, and holding it showed answered turns as working for an hour |
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
- **Seen model:** `review_ready` stays in the queue until the user attaches to that session or reads its changes on the web's review screen. There is no way to dismiss an item unread (2026-10-22): the queue is there to get what agents did reviewed. Marking seen is keyed by the transition's `seq`, so a newer turn brings the item back. Herdr had this built in. Valkyrie implements it, which is easy because the daemon sees every attach.
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
- **Early `done` (2026-10-08).** In five recorded sessions, about half of the `done` transitions came before the turn had actually ended. Every turn over a minute read as idle, because the spinner's timer becomes `(1m 5s ·` and `scan` only accepted `(<n>s`. Durations now parse as `[<n>h ][<n>m ]<n>s`, in any ` · ` part of the parentheses (`(running Stop hooks… 0/4 · 2m 20s · …)`). A spinner before its timer appears (`· Fermenting…`) and `Waiting for API response · will retry …` also count as busy. Turns waiting on background agents now stay working (see the table). Their status line stays in the scrollback, so only the line directly above the prompt box counts. AskUserQuestion dialogs also read as idle, and now scan as a prompt when the dialog footer is the screen's last line. `valk replay` now follows `foreground` events, so an agent started from a shell replays with its own adapter.
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

# ADR-0010: Projects learn from their sessions: a retro proposes what went wrong, skills are reviewed and served like decisions, and both are measured

Status: proposed
Date: 2026-10-27

## Context
Two things feed a project's decisions today, and both start from what you say:
- **The extractor** (DESIGN §6.3) reads one exchange when a turn ends, and only when your message reads like a correction.
- **`valk decisions mine`** reads the transcripts for what you keep repeating.

Neither sees what goes wrong *inside* a session. The daemon already receives `PostToolUseFailure`, `StopFailure` and `PermissionRequest`, but uses them only for state (ADR-0005). An agent that loses ten minutes to a trap leaves nothing behind for the next one, unless someone writes it down by hand. Ten `Exit code 144`s in this repo's transcripts came from one such trap: `pkill -f` matching the agent's own shell. It reached a single agent's private memory, and no other agent.

What a project knows is also limited to rules. Procedures ("how to verify a TUI change against an isolated daemon") live in a hand-written `.claude/skills/verify`. Nothing proposes one or keeps it current.

Prior art:
- **Hermes Agent (Nous Research)**
  - **Triggers:** it writes and patches `SKILL.md` files itself after a multi-step workflow, an error it found a way past, or a user correction. A background review runs after each session.
  - **Serving:** skills load in stages: an index, then a body on demand.
  - **Approval:** writes need none by default.
  - **Evidence:** reviewers note that nobody has shown it makes the agent better, and that nothing guards against bad or drifting skills.
- **Anthropic's AI-native SDLC playbook**
  - Plays: versioned project knowledge (#4), skills as institutional knowledge in git (#5), a feedback loop (#7), hooks as approval gates (#10), and closing the loop on metrics (#12).
  - Humans own approval.

Constraints:
- **ADR-0007 §4:** agents only propose, and a human makes anything active.
- **Work repos:** they shouldn't grow files.
- **Review budget:** a crowded queue gets rubber-stamped, so every proposal should earn its place.

## Decision
1. **A retro reads what went wrong, and proposes `gotcha`s and `fix`es.**
   - **When:** a session's agent ended a turn (`Stop`) after at least one tool failure since its last retro. A turn without a failure has nothing to read, so the session's end adds nothing. A retro the hourly limits hold back waits for the next turn's end, and so does a failure nothing has worked after yet.
   - **Moments:** the retro reads a moment, not the whole transcript. A moment is one of:
     - **A recovery:** a failed tool call (`tool_result.is_error`), and the next call of that tool that succeeded (the same program first). The command or input is kept with up to six lines of the error, from the line where it starts (`error:`, `fatal:`, a panic; warnings don't count). When no line reads like an error, only the last two lines are kept.
     - **A denial:** you or a permission check refused a call.
     - Corrections stay the extractor's.
   - **Not sent:** harness mechanics teach nothing about the project and are dropped: "File has not been read yet", "modified since read", `InputValidationError`, and a bare exit code with no output.
   - **Not read:** refusals of a question (`AskUserQuestion`, plan mode), and anything outside a git repository.
   - **Sending:** moments are redacted like the extractor's exchange. They go to the same locked-down model under the same consent (`valk decisions auto`).
   - **Limits:**
     - It shares the extractor's hourly limits.
     - At most two proposals come from one retro.
     - No daily cap per project yet (decided 2026-10-27): proposals are rare today, so a cap waits until real use shows the queue overflowing.
2. **Every moment has a signature**: the tool and the line its error starts at, with numbers, paths, ids and quoted names taken out, hashed. The command isn't part of it: most are chains, and which link failed isn't recorded.
   - **Storage:** `failures.jsonl` in the project's state dir, appended by the daemon. One line per failed call, keyed by its call id, so a restart or a resume reading the same transcript doesn't count it twice. One line per proposal made from one.
   - **Window:** only the last 90 days count, and past 1 MiB the file drops what's older.
   - **Cost:** a signature costs nothing to compute.
   - **Ranking:** signatures are how the retro ranks what to ask about (recurring first), and how item 5 measures.
3. **Skills are a second kind of reviewed knowledge, kept like decisions** (decided 2026-10-27: the state dir, not the repo).
   - **Storage:** a skill is `<state dir>/context/<project>/skills/<name>/SKILL.md`, with `name`, `description` (when to use it) and `version` front matter. Its body holds the procedure, pitfalls and verification, compatible with agentskills.io.
   - **Lifecycle:** `proposed → active | rejected`, `active → retired`.
   - **Patches:** a patch is a proposal against an active skill, reviewed as a diff, and accepting one bumps `version`.
   - **Who acts:** agents propose (`valk skill propose`, MCP `propose_skill`), and only a human reviews.
4. **Skills are served by index.**
   - **Index:** the context hook adds one line per active skill (`name: when to use it`), under its own budget of 1,500 characters beside the decisions' 6,000.
   - **Bodies:** an agent reads one when it needs it, with `valk skill show NAME` or MCP `get_skill`. This works the same for Claude and Codex, and puts no files in the repo.
   - **Export:** `valk skills export` copies active skills into `.claude/skills/` for anyone who wants them in git, natively loaded.
5. **Skills come from what agents already do:**
   - **The miner:** repeat groups that read as procedures, not rules.
   - **The retro:** a session that reached green through a long multi-step path.
   - **`valk skill learn SESSION`:** distills one session on request.
   - **Patching:** when a session that read skill X hits failures, the retro proposes a patch to X instead of a new skill.
6. **Knowledge is measured.**
   - **Recording:** a decision or skill proposed from a signature records it.
   - **Health:** `valk decisions health` reports when that signature recurs after acceptance ("#12, accepted 9 days ago: its failure happened 4 times since") and flags it for rewording or retirement.
   - **Miner:** repeat counts falling over time are the miner's measure.
   - Nothing is retired automatically.

## Phases
1. The retro (items 1 and 2), Claude transcripts.
2. The skill store, review (queue, TUI, web, CLI), index and serving (items 3 and 4).
3. Skill generation and patching (item 5).
4. Measures in `health` (item 6).

## Consequences
- **Larger exposure than the extractor's.** Tool inputs and error text can carry secrets and file contents.
  - A tool's output is never sent whole: a command and at most six lines of its error, clipped and redacted. Those lines may still hold what the command printed, so redaction is the guard, not the selection.
  - `auto off` stops it with everything else.
- **Codex:** its rollouts record tool failures differently, so phase 1 reads Claude's only. That is also the default consent (`auto claude`).
- **A turn whose only trouble was a refusal** may not fire `PostToolUseFailure`, so its refusal is read only once a failure arms the session. That needs checking against a real denial before refusals count as a trigger of their own.
- **A skill index costs every session up to ~400 tokens.** An unused skill is retired like a stale decision.
- **Hermes's defaults are not taken:**
  - No autonomous writes.
  - No hub installs: a skill from elsewhere is text that would reach every agent unreviewed.
  - No trajectories kept for training.
- **Model trust:** the model's text is still untrusted.
  - A proposed skill is shown in full before it can be accepted.
  - Bodies lose control characters and bidi overrides, as decisions do.
- **Open:** whether Valkyrie-launched Claude sessions should also load active skills natively, through a per-session plugin directory, so the `Skill` tool sees them. The index works without it.

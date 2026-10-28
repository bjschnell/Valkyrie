# ADR-0011: Projects learn from their sessions: a retro proposes what went wrong, skills are reviewed and served like decisions, and both are measured

Status: accepted (2026-10-28)
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
3. **A skill is a decision of kind `skill`** (decided 2026-10-27: Valkyrie's state dir, not the repo).
   - **What:** its title says when to use it, and its body holds the procedure: steps with their commands, pitfalls, how to verify. A body may be 12,000 characters, where a decision's is 4,000.
   - **Why a kind, not a second store:** a skill needs everything a decision already has: proposal and review in the queue, TUI, web and CLI; agents only proposing; retiring; anchors and staleness; provenance; export. A second store would copy all of it.
   - **Patches:** a patch is a proposed skill that `supersedes` the active one, reviewed like any proposal. Accepting it supersedes the old one, so the chain is its history.
     - **Whole or not at all:** a model may rewrite only a skill it was shown whole (newest first, up to 24,000 characters in all), so a rewrite can't drop a tail it never saw.
     - **One at a time:** the store refuses a second proposed replacement of the same decision while one waits, and refuses to accept a replacement of something another one already replaced. Otherwise two rewrites of one version, accepted in turn, would leave two active.
   - **Who acts:** agents propose (`valk decide --kind skill`, MCP `propose_decision`), and only a human reviews.
   - **Not from the extractor or the retro's lessons:** those are a few clipped sentences, never a procedure, so a `skill` kind in their replies is read as a decision.
4. **Skills are served by index.**
   - **Index:** the context hook lists active skills one line each (`#id title`, newest first), apart from the decisions, under their own budget of 1,500 characters beside the decisions' 6,000.
   - **Bodies:** an agent reads one when it needs it, with `valk decisions show ID` or MCP `get_decision`. This works the same for Claude and Codex, and puts no files in the repo.
   - **Export:** `valk decisions export` also writes active skills to `.claude/skills/<name>/SKILL.md`, for anyone who wants them in git, natively loaded. Each says which skill it is, so the next export replaces only its own.
5. **Skills come from what agents already do:**
   - **The miner:** a repeat group asking for the same several-step procedure is proposed as a skill.
   - **`valk decisions learn SESSION`:** reads one session as what was asked, said, run and what failed (redacted, its last 16,000 characters), and proposes the skill in it, or a rewrite of an active skill it shows wrong. `--about` names the procedure when the session did more than one thing. Claude sessions only, and not with `auto off`.
   - **Patching:** when a session that read skill X (`valk decisions show X` or Valkyrie's MCP `get_decision`) hits a failure, the retro is shown X and may propose X rewritten, naming the moment that showed it wrong; the rewrite quotes that failure and records its signature.
   - **A job you approved:** when the last message you typed approves finished work (a short sentence opening with "perfect", "looks good", "ship it", "merged"..., and nothing like "but", "fails", "not") after at least 15 tool calls since your previous approval, the daemon runs `learn` on that session by itself: your approval is what tells a procedure that worked from a struggle. Once per approval (kept by its transcript id, across restarts), under the extractor's hourly limits and consent, Claude sessions only.
   - **Triggered when asked for:** with each prompt, the context hook names the active skills whose title's words the prompt mostly holds ("Skill #12 “Cut a release” looks like what this asks for: read it first"). Word matching, no model.
6. **Knowledge is measured.**
   - **Recording:** a decision or skill proposed from a failure records its signature.
   - **Health:** `valk decisions health` and MCP `project_health` report an active decision whose failure happened in two or more sessions since it was accepted or last confirmed ("#12 isn't working: the failure it came from happened in 4 sessions since it was accepted"), to reword or retire. One session isn't enough: a session started before the acceptance never got it.
     - It sees the last 90 days: past that, the file's compaction drops the line naming the failure a decision came from, and the decision is no longer measured.
   - **Miner:** repeat counts falling over time are the miner's measure.
   - **Unread skills are cleaned up, by counting alone:** each Stop in a project with skills records that the session worked there, and which active skills it read, by the transcript's own time (`skill-use.json`: since when, the last read per skill, each session's last turn).
     - **Asked first:** a skill no agent read in 45 days *and* 10 sessions of work there, since it was accepted, confirmed or tracking began, waits on review ("keep it, or retire it"). A project left alone keeps its skills; time before tracking began doesn't count.
     - **Retired after:** when that question has stood unanswered for 45 more days (when it was first asked is kept, so a restart or a closed laptop doesn't skip it), the daemon retires the skill. Keeping it (`a`), reading it or editing it starts over. A retired skill stays on file (`valk decisions --all`).
   - Nothing else is retired automatically.

## Phases
1. The retro (items 1 and 2), Claude transcripts. Shipped in #29.
2. The `skill` kind, its index, serving and export (items 3 and 4). Shipped in #32.
3. Skills from the miner, `learn` and retro patches (item 5). Shipped in #32; learning from an approved job and suggesting a skill per prompt in #34.
4. Recurrences in `health` (item 6). Shipped in #32; cleaning up unread skills in #34.

## Consequences
- **Larger exposure than the extractor's.** Tool inputs and error text can carry secrets and file contents.
  - A tool's output is never sent whole: a command and at most six lines of its error, clipped and redacted. Those lines may still hold what the command printed, so redaction is the guard, not the selection.
  - `auto off` stops it with everything else.
- **Codex:** its rollouts record tool failures differently, so phase 1 reads Claude's only. That is also the default consent (`auto claude`).
- **A turn whose only trouble was a refusal** may not fire `PostToolUseFailure`, so its refusal is read only once a failure arms the session. That needs checking against a real denial before refusals count as a trigger of their own.
- **A skill index costs every session up to ~400 tokens.** Past its budget, the newest are listed and the rest counted.
- **Upgrades:** an older binary reads `kind: skill` as a decision, and rewrites it as one if it edits the file. Run one version (`valk upgrade --local`) before relying on skills.
- **A rewrite carries its failure quote** into the skill's body once accepted; you can edit it out when accepting.
- **Hermes's defaults are not taken:**
  - No autonomous writes.
  - No hub installs: a skill from elsewhere is text that would reach every agent unreviewed.
  - No trajectories kept for training.
- **Model trust:** the model's text is still untrusted.
  - A proposed skill is shown in full before it can be accepted: the web card scrolls, `valk decisions show` prints it, and the TUI's preview scrolls (PgUp/PgDn) and accepts a skill only once it's been scrolled to its end.
  - Bodies lose control characters and bidi overrides, as decisions do.
- **Open:** whether Valkyrie-launched Claude sessions should also load active skills natively, through a per-session plugin directory, so the `Skill` tool sees them. The index works without it.
- **Reads are seen in Claude transcripts of Valkyrie sessions only.** A skill only Codex sessions, a `claude` outside Valkyrie, or you read looks unread, as does every skill with the Context setting off (agents never see the index); the question comes first, so keeping it is one key.

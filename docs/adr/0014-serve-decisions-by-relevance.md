# ADR-0014: Decisions are served by relevance in layers, not dumped into every session

Status: proposed (2026-11-01)
Date: 2026-11-01
Amends: [ADR-0007](0007-context-store.md) §6 (injection), [ADR-0011](0011-learning-loops.md) §4–5 (serving and triggering, extended from skills to decisions)

## Context
Every session gets one block at start (ADR-0007 §6): the project's active decisions, ranked (constraints, gotchas, decisions, patterns), within 6,000 characters. On 2026-11-01 this repo had 13 active decisions with long bodies, and the cut fell before any pattern. The shipping rule (#11, then #19) never reached an agent. #64 now gives every decision at least its title, so bodies fill only what the titles leave.

That only postpones the problem:
- **Room.** A title line is about 90 characters. Once the header and the how-to-propose text are in, about 45–50 titles fit. This repo made 19 decisions in three weeks, so it reaches that in a few months, and then the lowest-ranked kinds are cut whole again.
- **Noise.** The block gives every session every decision, whatever it's doing. A docs session gets the TUI-keys rule, and a fix in `inject.rs` gets the rain-animation rule. More history means more noise for every agent, and a bigger budget only makes the noise bigger.

**A full memory system (Honcho and the like) was considered and not taken.** What it would add is retrieval. What comes with it is what ADR-0007 §4 and the 2026-10-29 notes rejected:
- memory written from conversations without review,
- transcripts sent to a service, which is hard to defend on a managed work laptop,
- and the user's own experience of Hermes/Honcho memories that are wrong but sticky.

It also doesn't solve the problem. A store of reviewed decisions still has to choose what a given session gets, and that's ranking, which would be handed to someone else.

**What exists to build on:**
- **Anchors** (DESIGN §6.4): the files a decision's text names, used today only to flag a decision as stale. Only 1 of the 13 has any (#13: `target/release`), because few decisions name a path.
- **Skills** are already served as an index, with their bodies read on demand. The context hook already names the skill a prompt matches, by word matching (ADR-0011 §4–5).
- **Claude Code delivers added context on more than `SessionStart`.** Verified on 2.1.316 (2026-11-01): `additionalContext` printed by a `PreToolUse` or `PostToolUse` hook on `Read` reached the model.

## Decision
A decision reaches an agent in one of four layers. Each one is served at most once per session.

1. **The block becomes an index.** At session start the block holds:
   - **Bodies only for what holds everywhere:** constraints, and decisions the user *pins*. A pin is a flag only the user sets, like accepting.
   - **Every other active decision as a title line**, as skills already are, with how to read one (`valk decisions show N`, MCP `get_decision`) and how to search (item 4).
   - **When titles alone overflow,** it lists the highest-ranked and says how many more there are, as now.

   A pin is a deliberate act and the block's budget is visible, so the user decides what is always in context instead of the ranking deciding.
2. **Per file: decisions scoped to the paths an agent touches.** A decision gets a *scope*: path globs it applies to (`crates/valkyrie-tui/**`, `web/src/**`).
   - **Where scopes come from:** the model proposes one with each decision it extracts, mines or learns. A human-recorded decision gets one from the paths its text names, as anchors do now. The user can edit it on review or later.
   - **Existing decisions:** each gets a scope proposed once, through the review queue, so nothing changes without the user.
   - **Serving:** on the first `Read`, `Edit` or `Write` in a session that touches a path in a decision's scope, the `PreToolUse` hook adds "Decisions about `<path>`:" with those decisions' bodies.
   - **Anchors** keep their job of flagging staleness. A scope says where a decision applies, and an anchor says what would make it wrong.
3. **Per prompt: decisions matched to what the user asked.** The match `skill::about` does for skills (ADR-0011 §5) also runs over decisions, against title and body. At most three are added per prompt, ranked by how many words they share. A decision already served in the session isn't served again. Word matching comes first. Local embeddings, with nothing leaving the machine, are a later swap if word matching misses too much. The serving log in item 5 is how to tell.
4. **On demand: search.** `valk decisions search TEXT` and MCP `search_decisions` return matching active decisions, best first, so an agent can ask "has the user decided anything about X" before it guesses.
5. **Every serve is logged.** Each decision served to a session is recorded: which layer served it, and when (`served.jsonl` in the project's state dir, 90 days). The hooks and the read commands (`valk decisions show`, MCP `get_decision` and `search_decisions`) append to it themselves, as the hooks already read the store directly, so no protocol message is needed. A session is the agent's own session id from the hook payload, so a `/clear` starts afresh; the read commands, which don't see it, use Valkyrie's session. That does three jobs:
   - **It stops repeats.** A hook reads what its session has already had from the log.
   - **It finds decisions that never reach anyone.** `valk decisions health` reports a decision served in no session for 45 days of work (10 sessions that didn't get it), like an unread skill (ADR-0011 §6). That's the check that would have caught #11. Only a serve with the body counts: a title line in the block says a decision exists, not what it says, and after item 1 most decisions are served that way. Skills (measured by reads) and policies (titles on purpose) aren't flagged.
   - **It shows whether the layers work** before anything heavier is built.

**Rules that become code leave the text.** A decision enforced as a policy (ADR-0013) is served only as a title, since the hook does the work. Every rule that turns into a policy shrinks the block.

## Phases
1. The serving log and a health check for decisions that are never served. This measures the problem before the fix.
2. The block as an index, with pins.
3. Per-prompt matching for decisions, and `search`, CLI and MCP.
4. Scopes: the field, model-proposed scopes, the one-time scoping of existing decisions, and per-file serving on `PreToolUse`.

## Consequences
- **Context arrives as it's needed, not up front.** An agent that never touches the TUI never hears about its keys. One that does hears about them at the moment it opens the file, which is when the rule matters.
- **More hook work per tool call.** The `PreToolUse` hook matches a path against the project's scopes. That's a glob match against a few dozen patterns, well within ADR-0005's 50 ms. The daemon caches the decisions, so the hook doesn't parse the store each time.
- **Codex** gets the block and search. Per-file and per-prompt serving depend on its hooks supporting added context on those events, which hasn't been verified.
- **Word matching misses synonyms.** A prompt about "releases" won't match a decision about "tags". The index in the block covers that: every title is still there, and the agent can read any one of them.
- **The store stays as ADR-0007 made it:** reviewed, local and human-gated. Nothing here writes a decision. It changes only when and where one is shown.

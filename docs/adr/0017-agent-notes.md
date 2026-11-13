# ADR-0017: Agents keep unreviewed notes below decisions; notes are only searched, never injected, and recurring ones become proposals

Status: accepted (2026-11-11); phase 1 built (#158): the store, `valk note`, `valk notes show|list|rm|health`, tombstones and staleness; phase 2 built (#159): `valk notes search`, `valk sessions search`, MCP `search_notes`, and their serves in `served.jsonl`; phase 3 built (#160): recurring notes become one proposal, promoted on accept; phase 4 built (#161): the retro writes notes, shown the live ones on the same failure, and replaces one this session was served
Date: 2026-11-02
Amends: [ADR-0011](0011-learning-loops.md) §1 (the retro writes notes instead of proposing), [ADR-0014](0014-serve-decisions-by-relevance.md) §5 (the serve log also records notes)

## Context
Agents trip over the same project traps across sessions: a flaky test, a slow build, an environment gotcha. What one session learns dies with it, or lands in one agent's private memory, where no other agent sees it and the user can't check it.

Decisions (ADR-0007) are the wrong place for this. Each one costs the user a review, and a crowded queue gets rubber-stamped (ADR-0011's review budget). Most of what an agent discovers is small, local and often wrong the first time. It needs to be cheap to write and cheap to kill.

**What was rejected, and why it matters here.** On 2026-10-29 the author weighed taking on Hermes Agent's memory (Honcho-style project memory written from conversations). It was turned down. The author's own experience with those memories was that they were wrong, sticky and hard to correct, and there was no way to tell whether they were helping. This ADR is what survived that discussion. It ships only if it meets four rules aimed at exactly those failures:

1. **Checkable.** Every note links to the session and commit it came from.
2. **Correctable.** A correction replaces the old note and kills it. The two never sit side by side.
3. **Traceable.** Every time a note is served to a session, that's logged, so a bad outcome traces back to the note.
4. **Deletable.** Deleting a note is one command, and nothing re-derives it afterward.

**What exists to build on:**
- **The retro** (ADR-0011 §1–2) already reads failed tool calls, gives each one a signature, and records them in `failures.jsonl` with their session. Today its lessons go straight to the review queue as `gotcha` and `fix` proposals.
- **Staleness** (DESIGN §6.4): `stale::anchors` finds the files a text names, and `stale::churn` flags a text whose anchors changed by 150 lines or more since its commit.
- **The serve log** (ADR-0014 §5, backlog #29): `served.jsonl` in the project's state dir, not built yet.
- **Transcripts:** hooks name each Claude session's `transcript_path`, and Claude keeps a repository's transcripts under `~/.claude/projects/<path, dashed>/` for the main checkout and each worktree.

## Decision
1. **Notes are a tier below decisions, in the same store.**
   - **Where:** `<state dir>/context/<project>/notes/NNNN.md`, beside `decisions/`, with the same `key: value` front matter. A project is still a repository, so worktrees share notes. The daemon is the only writer (ADR-0007 §3). Notes are never exported to the repository.
   - **What:** one short text, at most 1,000 characters, cleaned of control characters and bidi overrides like a decision's.
   - **Front matter:** `id`, `status` (`live`, `replaced`, `promoted`), `created`, `by` (`agent`, `user` or `valkyrie`), `session`, `conversation`, `commit`, `cwd`, `anchors`, and `sig` when it came from a failure. A replacement has `replaces: N`, and the old note gets `replaced_by: M`.
   - **Who writes:** anyone, with no review. Agents write with `valk note TEXT`, and the retro writes them (item 5).
2. **Rule 1: a note always says where it came from.** The daemon fills in `session`, `conversation` and `commit` itself, from the connecting peer (ADR-0007 §4) and the checkout's `HEAD`, which it reads itself. It doesn't take them from the client. A note written outside any Valkyrie session records `session: none` and the commit of the directory it was written in. Search prints both with every hit, so a reader can open the session's transcript or `git show` the commit.
3. **Rule 2: a correction replaces.** `valk note --replaces N TEXT` writes the new note and marks N `replaced` in the same atomic write. A replaced note is never served again and never counts toward promotion. It stays on disk only so the chain can be read (`valk notes show N`). Replacing a note that's already been replaced is refused with the newer id, so a chain never forks.
4. **Notes are never injected.** No hook, block, sibling note or MCP listing ever carries one. They reach an agent only when it asks:
   - **`valk notes search TEXT`** and MCP `search_notes` match words, using ADR-0014's matcher, over two sources, and return the best matches first:
     - **Live notes.** A stale note (item 6) is still shown, but last and marked with its reason.
     - **This repository's Claude transcripts**, from the main checkout and every worktree. A hit is a tool error or a user message, with the next successful call of the same tool after it, so it answers "has an agent here hit this, and what fixed it?". The output is redacted like the retro's moments. Codex transcripts come later.
   - **`valk sessions search TEXT`** is the same search over transcripts only.
   - **Framing:** each hit is a single cleaned line, headed as an unreviewed agent note or a past session, with its session, commit and date. The output says it's observation, not instruction, the way sibling notes do (ADR-0007 §4).
   - **Rule 3: every serve is logged.** Each hit returned is a line in `served.jsonl` (ADR-0014 §5): `{at, session, layer: "search", note: N}` or, for a transcript, `{..., transcript, line}`. `valk notes show N` lists where it was served. When a failure's signature matches a note served earlier in the same session, the retro is shown that note (item 5). So a note that sends agents the wrong way turns up next to the failure it caused.
5. **The retro writes notes, not proposals** (amends ADR-0011 §1).
   - Each lesson the retro draws from a moment becomes a note with `by: valkyrie` and the moment's `sig`. It no longer goes to the review queue directly. The limit of two per retro, the hourly limits and `valk decisions auto` consent stay as they are.
   - **Before writing,** the retro is shown the live notes with the same signature. If one of them was served in this session and the failure happened anyway, the retro may replace it (rule 2) instead of adding a second note.
   - **Skill patches** (ADR-0011 §3) are unchanged and still go to review: they rewrite something the user already accepted.
6. **Staleness** (DESIGN §6.4). A note's anchors are found when it's written. A note goes stale when an anchor is deleted, or when `stale::churn` since the note's commit reaches 150 lines. Stale notes are marked in search and don't count toward promotion. Nobody has to review them: they sink, and a fresh note on the same trap replaces them.
7. **Recurrence becomes a proposal** (ADR-0007 §4 unchanged: only the user makes anything active).
   - **What recurs:** live, non-stale notes from **three** separate sessions that share a `sig`, or whose texts match by `extract::already_known`'s word test. Three sessions, set by the author on 2026-11-02, is stricter than `RECURRED` (2), the threshold ADR-0011 §6 uses to call an accepted gotcha ineffective.
   - **The proposal:** one `gotcha` or `fix`, `by: valkyrie`, drafted from the notes by the same locked-down model under the same consent. If `valk decisions auto` is off, the newest note's text is used as it is. The body cites every note it came from (id, session, commit), and it's reviewed like any other.
   - **Accepted:** its notes become `promoted` and drop out of search, because the decision now carries the knowledge through ADR-0014's layers.
   - **Rejected:** the notes stay live and searchable, and that cluster isn't proposed again (`already_known` already counts rejected decisions). A new note joining the cluster doesn't change that.
   - **One at a time:** while a cluster's proposal waits, no second one is made from it.
8. **Rule 4: one command deletes.** `valk notes rm N` removes the note's file and writes a tombstone to `notes/deleted.jsonl`: its `sig`, a hash of its words, who deleted it and when.
   - **Nothing re-derives it:** `valk note` refuses a text that matches a tombstone by `sig` or words and says why, and so does the retro. Promotion skips them. `--force` is for a human who wants to write it again on purpose.
   - **Who:** anyone, since notes are unreviewed anyway. A deletion by an agent is printed in `valk notes health` for the user to see.
   - **Transcripts** aren't notes and can't be deleted from Valkyrie. They belong to the agent's own tool.

## Measuring it
This ADR's first open question is whether notes add anything over what the retro's gotchas already catch. The measures are set before anything ships:
- **Baseline, before phase 1:** for 7 days of work, the number of failure signatures in `failures.jsonl` seen in two or more sessions, and `valk pings` stops whose question an earlier session had answered.
- **After:** the same counts, plus from `served.jsonl` how many searches each session ran and how many found something. The real signal is a recurring signature whose sessions ran a search, got a note, and didn't fail again.
- **If searches are rare,** agents aren't asking. The answer is a single line in the session-start block saying search exists, not injecting notes.

## Phases
1. The store, `valk note [--replaces N]`, `valk notes show|list|rm|health`, tombstones and staleness.
2. Search over notes and Claude transcripts, CLI and MCP, with the serve log. This builds `served.jsonl` for #29 too.
3. Promotion from recurrence.
4. The retro writes notes (item 5). This comes after phase 3, so the retro's lessons can still reach the queue once they recur.

## Consequences
- **Fewer proposals.** The retro's one-off lessons wait as notes until they recur, so the queue gets the traps that keep happening, with the sessions as evidence.
- **Unreviewed text reaches agents, but only when they ask.** That's the same exposure as reading a file in the repository. Notes never reach a context on their own, and every serve is logged, so a note that misleads can be traced and deleted.
- **A note can be wrong.** Search says so in its framing. The commit and session are there to check it against, and a correction replaces it.
- **Transcript search reads files Claude Code owns,** in the user's own config directory. It never sends them anywhere. Only redacted snippets reach the asking agent.
- **The store grows.** Notes are small, and replaced, promoted and stale ones stop counting. Pruning waits until a project actually holds enough to matter.

## Rejected
- **Ungated project memory, written from conversations and injected** (Honcho-style): wrong, sticky and untraceable in the author's experience, and against ADR-0007 §4. Turned down on 2026-10-29.
- **Keeping the retro's direct proposals beside notes:** two channels for the same lesson, and the queue keeps its noise.
- **Notes in the repository:** work repositories shouldn't grow files (ADR-0007, ADR-0011), and notes are too raw to commit.

## Open questions
- Whether 1,000 characters is enough for a note that names a fix, or too much for search output.
- Whether stale notes should eventually be pruned on their own, and after how long.
- Whether Codex's transcripts are worth reading, and in which phase.

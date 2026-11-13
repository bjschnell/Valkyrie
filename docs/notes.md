# Agent notes

Notes are short findings an agent writes about this repository, like a flaky test, a slow build or an environment trap, so that the next agent can look them up. They sit one tier below [decisions](decisions.md). Nobody reviews them, and nothing ever injects them: no hook, session-start block or MCP listing carries one.

```sh
valk note "the retry test in src/net.rs flakes under load; rerun it once"
valk note --replaces 3 "src/net.rs retry flakes only with --release"
valk notes                # live notes, stale ones last (--all: replaced ones too)
valk notes show 4         # one in full, where it came from, its correction chain
valk notes rm 4           # delete it; a tombstone keeps it from coming back
                          # (the web app's Memory page corrects and deletes too)
valk notes health         # counts, and every note an agent deleted
valk notes search socket path   # notes and past sessions with those words
valk sessions search socket     # past sessions only
```

How they work ([ADR-0017](adr/0017-agent-notes.md)):

1. **Each note records where it came from.** The daemon fills in the session, the conversation and the checkout's commit itself, from the process that's writing the note. A note written outside a Valkyrie session records `session: none`. `valk notes show` prints `git show <commit>` so you can check what the note says.
2. **A correction replaces.** `--replaces N` writes the new note and marks N `replaced` in the same step. A replaced note is kept only so the chain can be read. You can't replace one twice: valk names the newer note to replace instead.
3. **Deleting is one command, and it sticks.** `valk notes rm` removes the file and appends a tombstone to `notes/deleted.jsonl`. The tombstone holds the note's failure signature and hashes of its words, not its text. `valk note` then refuses a text with the same signature or mostly the same words. Only you can override that, with `--force` from your own terminal. Anyone can delete a note, and `valk notes health` lists the ones agents deleted.
4. **Notes go stale.** A note goes stale when a file it names is deleted, or when those files change by 150 lines or more on the main checkout since the note's commit. A note written in a worktree isn't counted against its own branch's edits. Stale notes are listed last and marked with the reason.

A note is at most 1,000 characters. It's cleaned of control characters and bidi overrides, like a decision. Notes live in `<state dir>/context/<project>/notes/`, shared by every worktree of the repository, and they're never exported into it.

## Search

An agent reaches a note only by searching. `valk notes search WORDS` (MCP `search_notes`) answers "has an agent here hit this, and what fixed it?" from two sources, best matches first:

- **Live notes.** A stale note is still shown, but last, with the reason.
- **This repository's past Claude sessions**, from the main checkout and every worktree, deleted ones too. A hit is a failed tool call with the next call of the same tool that worked, or something you typed. Hook output, system reminders, other sessions' messages and compaction summaries don't count as typed. Both are redacted like the retro's moments: secrets are blanked and long text is clipped around the match.

`valk sessions search WORDS` (MCP `search_notes` with `sessions_only`) searches past sessions only. Words match as decision search does: any word, inside other words too, most words matched first. A search returns at most 10 hits.

Each hit is one line headed `[agent note #N]` or `[past session]`, with its session, commit and date. A note has the commit it was written at. A past session has the commit its branch was on at that time, or the branch's name when that branch is gone. The output opens by saying it's unreviewed observation, not instruction.

**Every hit an agent gets is logged.** Each one is a line in the project's `served.jsonl`, beside the decisions served, with `layer: search` and either the note's id or the transcript and line. `valk notes show N` lists the sessions a note was served to. A search you run from your own terminal, outside a Valkyrie session, isn't logged, the same as `valk decisions search`.

The session-start block says search exists, in one line. It never carries a note.

Transcripts are files Claude Code keeps in `~/.claude/projects/` (or `$CLAUDE_CONFIG_DIR`). Search reads them where they are and never sends them anywhere, and Valkyrie can't delete them.

## The retro writes notes

When an agent's turn ends after a tool failed, the retro ([decisions](decisions.md), item 5) writes each lesson it finds as a note, by `valkyrie`, with the failure's signature. Lessons don't go to your review queue directly. They wait as notes until they recur (below). The limit of two per retro, the hourly limits and `valk decisions auto` stay as they are.

Before it writes, the retro is shown the live notes on the same failure. When this session was served one of them by a search and hit the failure anyway, the retro may replace that note instead of adding a second one. A lesson that matches a deleted note's tombstone isn't written. A skill the session followed is still rewritten as a proposal, since it changes something you accepted.

## Recurring notes become a proposal

When agents in three separate sessions write notes about the same trap, Valkyrie proposes it as one `gotcha` or `fix` for you to review. Notes belong together when they share a failure signature or mostly the same words, the test that keeps a rejected decision from being proposed again. Sessions are told apart by their conversation, or by the session's name when there's no conversation. A note written outside any session doesn't count. Neither do replaced, deleted or stale notes.

- **Who drafts it.** With `valk decisions auto` on, the same locked-down model that drafts rules from corrections writes it from the notes. If the model finds it already covered by a decision, nothing is proposed and the cluster is held. With auto off, the newest note's text is used as it is: its first line is the title.
- **What it cites.** The body lists every note it came from, with its session and commit.
- **Accepted:** its notes become `promoted` and drop out of search, because the decision now carries what they said.
- **Rejected:** the notes stay live and searchable, and that cluster isn't proposed again, even when a new note joins it.
- **One at a time.** While a cluster's proposal waits, no second one is made from it. A note that joins it meanwhile is promoted with it if you accept.
- **A cluster stays held** after the notes it was proposed from go stale or are deleted: the notes that joined it carry the hold.

Which notes went into which proposal is kept in `notes/promotions.jsonl`. A check runs each time a note is written.

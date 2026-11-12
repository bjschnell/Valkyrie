# Agent notes

Notes are short findings an agent writes about this repository, like a flaky test, a slow build or an environment trap, so that the next agent can look them up. They sit one tier below [decisions](decisions.md). Nobody reviews them, and nothing ever injects them: no hook, session-start block or MCP listing carries one.

```sh
valk note "the retry test in src/net.rs flakes under load; rerun it once"
valk note --replaces 3 "src/net.rs retry flakes only with --release"
valk notes                # live notes, stale ones last (--all: replaced ones too)
valk notes show 4         # one in full, where it came from, its correction chain
valk notes rm 4           # delete it; a tombstone keeps it from coming back
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

Coming next: notes that recur across sessions get proposed as decisions.

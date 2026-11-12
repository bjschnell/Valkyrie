# Agent notes

Notes are short findings an agent writes about this repository, like a flaky test, a slow build or an environment trap, so that the next agent can look them up. They sit one tier below [decisions](decisions.md). Nobody reviews them, and nothing ever injects them: no hook, session-start block or MCP listing carries one.

```sh
valk note "the retry test in src/net.rs flakes under load; rerun it once"
valk note --replaces 3 "src/net.rs retry flakes only with --release"
valk notes                # live notes, stale ones last (--all: replaced ones too)
valk notes show 4         # one in full, where it came from, its correction chain
valk notes rm 4           # delete it; a tombstone keeps it from coming back
valk notes health         # counts, and every note an agent deleted
```

How they work ([ADR-0017](adr/0017-agent-notes.md)):

1. **Each note records where it came from.** The daemon fills in the session, the conversation and the checkout's commit itself, from the process that's writing the note. A note written outside a Valkyrie session records `session: none`. `valk notes show` prints `git show <commit>` so you can check what the note says.
2. **A correction replaces.** `--replaces N` writes the new note and marks N `replaced` in the same step. A replaced note is kept only so the chain can be read. You can't replace one twice: valk names the newer note to replace instead.
3. **Deleting is one command, and it sticks.** `valk notes rm` removes the file and appends a tombstone to `notes/deleted.jsonl`. The tombstone holds the note's failure signature and hashes of its words, not its text. `valk note` then refuses a text with the same signature or mostly the same words. Only you can override that, with `--force` from your own terminal. Anyone can delete a note, and `valk notes health` lists the ones agents deleted.
4. **Notes go stale.** A note goes stale when a file it names is deleted, or when those files change by 150 lines or more since the note's commit. Stale notes are listed last and marked with the reason.

A note is at most 1,000 characters. It's cleaned of control characters and bidi overrides, like a decision. Notes live in `<state dir>/context/<project>/notes/`, shared by every worktree of the repository, and they're never exported into it.

Coming next: `valk notes search` over notes and past Claude transcripts, with every hit logged. After that, notes that recur across sessions get proposed as decisions.

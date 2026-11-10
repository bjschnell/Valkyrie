# Using Valkyrie

```sh
valk                       # open the TUI
valk new -a -- claude      # start Claude Code in a new session and attach
valk new -- codex          # start Codex in the background
valk new -w --name "fix login" -a -- claude   # in its own git worktree and branch
valk ls                    # list sessions, with their state
valk handoff 3 --to codex -a   # start codex on session 3's work, with a resume of it
valk notify "deploy done" --sound done   # a toast, ping and desktop notification
valk pings                 # where agents stopped and waited on you, last 30 days
valk setup codex           # once, if you use Codex: adds Valkyrie's hooks
valk setup alias           # optional: type v instead of valk, if nothing else is called v
```

## One worktree per agent

`-w`/`--worktree [BRANCH]` starts a session in a new git worktree, so agents in the
same repo never edit one checkout. The worktree goes beside the main checkout as
`<repo>-<branch>` (for example `~/repos/app-fix-login`).

- The branch is new, made from the current `HEAD`, unless BRANCH already exists.
  Without BRANCH, it's named after `--name`.
- Uncommitted changes and ignored files (`node_modules`, `.env`) don't come along.
- Every worktree shares the repo's decisions. The phone's New session screen has
  the same option.
- Once the work is merged, `valk worktree prune` lists which worktrees could go
  and why the others stay; `valk worktree prune --yes` takes away those whose
  branch is in `origin`'s default branch, with the branch, unless a session runs
  there, a backlog item owns it, it has no commits of its own, or it has changes.
  It lists the ignored files (`.env`, `target/`) that go with each.

## Agents drive their panes

An agent in a session can split its own tab and work the other panes, the way
herdr's `pane` commands do. A pane it starts never takes your keyboard.

```sh
valk pane list                     # panes of my tab: id, where it sits, state; * is me
valk pane split -d right -- npm run dev   # prints the new pane's id
valk pane run 4 "cargo test"       # type it and press Enter
valk pane wait 4 --match "test result" --timeout 300   # exits 124 on timeout
valk pane read 4 -n 40             # the last 40 lines, scrollback included
valk pane resize 4 30 · valk pane close 4
```

Agents learn of it from their session-start context.

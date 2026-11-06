# Using Valkyrie

```sh
valk                       # open the TUI
valk new -a -- claude      # start Claude Code in a new session and attach
valk new -- codex          # start Codex in the background
valk new -w --name "fix login" -a -- claude   # in its own git worktree and branch
valk ls                    # list sessions, with their state
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

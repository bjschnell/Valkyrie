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
valk digest                # what every agent did while you were gone, as markdown
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

## What happened while you were gone

`valk digest` gives a line for each agent session since you were last away, grouped
by repository: shipped, stuck, abandoned, waiting on PR/CI, still working or
done, with why ("PR #12 merged", "CI red on PR #9", "ended with uncommitted
changes") and the agent's own last line. It counts you as gone after 30
minutes without typing into any session. It's markdown, so you can paste it as
a standup.

```sh
valk digest                       # since you were last gone
valk digest --since yesterday --by day
valk digest --since 12h --json
```

The web app has the same view: the clock on the home screen, with a Copy button.

## Docs for you to read

Agents write plans, research and reports as markdown into one docs folder
instead of scattering them around the repository. Point it at a folder in your
Obsidian vault and they show up there:

```sh
valk doc root ~/vault/valk         # you only; an agent can't move it
valk doc list                      # newest first
valk doc read 3                    # with the hash an edit needs
```

Docs from inside a repository go in `repos/<name>/`, the rest at the top. Each
one says which agent, session, commit and backlog item it came from. Agents can
edit any doc, yours too, but only from the version on disk: if you saved in
between, their edit is refused and they fold your change in first. Every version
an edit replaces is kept.

The web app's **Docs** page (the page icon on the home screen) lists them, with a
count of the ones an agent wrote since you last looked. It shows them read-only,
with tables, task lists and `[[wikilinks]]` to other docs. It never writes to a
file, so your own docs stay as you left them.

To comment on a doc, tap a heading, paragraph, list item or code block, or use
**Comment on the doc**. If the session that wrote it is still running, its agent
gets your comment at its next prompt, with the text you tapped. The comment shows
as *sent* until that session edits the doc, and then as *answered*. If the
session is gone, the comment stays *open* on the doc. **Resolve** closes a
comment. Comments are kept beside the docs in `<state dir>/docs/comments/`, not
in the files, so Obsidian never sees them. Only Claude sessions get comments for
now.

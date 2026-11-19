# Using Valkyrie

```sh
valk                       # open the TUI
valk tutor                 # learn Valkyrie: an agent teaches it, one lesson at a time
valk new -a -- claude      # start Claude Code in a new session and attach
valk new -- codex          # start Codex in the background
valk new -w --name "fix login" -a -- claude   # in its own git worktree and branch
valk ls                    # list sessions, with their state
valk handoff 3 --to codex -a   # start codex on session 3's work, with a resume of it
valk notify "deploy done" --sound done   # a toast, ping and desktop notification
valk pings                 # where agents stopped and waited on you, last 30 days
valk stats                 # is Valkyrie helping? the agents week by week, this repo
valk notes                 # findings agents left about this repo (unreviewed, never injected)
valk notes search WORDS    # those notes and past sessions here: what failed, what fixed it
valk summary                # what every agent did while you were gone, as markdown
valk sessions              # agent sessions that ended, newest first, numbered
valk sessions resume 1 -a  # pick the newest back up where it left off, and attach
valk search WORDS          # the session where you (or an agent) said or did something
valk setup codex           # once, if you use Codex: adds Valkyrie's hooks
valk setup alias           # optional: type v instead of valk, if nothing else is called v
valk --remote box          # the TUI here, on box's sessions, over SSH
valk remote web box        # box's web app in this machine's browser, paired
```

## Learning it: the tutor

`valk tutor` starts an agent (Claude, or `--agent codex`) that teaches Valkyrie
one hands-on lesson at a time: sessions and their states, driving one without
attaching, the TUI, decisions, the backlog, the supervisor. It opens a shell
pane beside itself for you to type in and reads what happened, so there is
nothing to paste back. Before teaching a command it reads that command's
`--help`, so it teaches the build you have. A second `valk tutor` opens the
lesson in progress instead of starting over. `?` on the TUI's home screen
and "Start the tutor" in the web app start the same one.
`valk setup claude` also installs it as a skill, so `/valk-tutor` works in any
Claude Code session, and `valk tutor --print` shows its brief.

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
- The daemon also sweeps on its own every 10 minutes, so finished worktrees and
  their build output don't fill the disk. A worktree goes when its backlog item
  shipped or was dropped, when it's merged with commits of its own, or when
  nothing moved in it for 14 days and all of it is merged or pushed to `origin`.
  It never goes while it has changes, moved within the hour, holds ignored files
  other than build output (`.env` keeps it; `target/`, `node_modules/` don't),
  is locked, has an open item on it, or a program runs in it. A shell left idle
  at its prompt there with no jobs (an agent stopped with Ctrl+C) is closed first. Its branch goes only if it's merged. Turn it off
  with `worktree_sweep = off` in `settings.toml`.

## Which repositories you're offered

The launcher, the backlog's Repository field and the wiki all offer the same list
of repositories, which the daemon keeps. A repository is on it when a session
runs in it or a backlog item is for it, or it sits one level under a folder Valkyrie
looks in (`~/repos`, `~/src`, `~/code`, `~/projects`, `~/dev`, `~/work` and `~/git` to
start with). A worktree counts as its main checkout, however it's named.

- **Web app:** in the New session screen, × on a repo hides it. Browse to a
  repository and pick **Add … to repos**, or to a folder and pick **Find repos in …**
  to have Valkyrie look in it too. **Finding repos in N folders** lists the folders
  and the hidden repos: × stops looking in a folder, **Show** brings a repo back.
  The backlog's Repository field and the wiki's picker end with **Add a
  repository…**, which takes a path.
- **TUI:** in a backlog edit (`e`), Shift-Tab steps through the list and `^R` adds
  a repository by path, typed or pasted.

Your changes are kept in `~/.config/valkyrie/repos.json`.

## Another machine over SSH

`valk --remote HOST` opens the TUI here on HOST's sessions. HOST is anything
`ssh` takes: `box`, `me@box`, an alias from `~/.ssh/config`. Each connection
runs `ssh HOST valk bridge`, which starts HOST's daemon if need be. Pings, the
clipboard, desktop notifications and links happen on the machine you sit at.
`ls`, `new`, `kill`, `rename`, `send` and `dump` work the same way
(`valk --remote box ls`). Anything that reads a repository or this machine's
settings doesn't: run it there with `ssh box valk …`.

- Both machines need the same release. If they differ, `valk --remote` says so;
  `valk upgrade` on each keeps every session.
- `ssh` may ask for a password before the TUI starts. After that the TUI reuses
  that connection, and if it drops, reconnecting needs a key or your SSH agent.
- `valk` is found on HOST's `PATH`, else in `~/.local/bin`.
- Editing a doc and adding a phone (`p`) need HOST's own TUI: `ssh -t box valk`.

The web app works the same way. `valk remote web HOST` starts HOST's `valk web`
(as `valk setup web --local` there), forwards a local port to it, and opens it
in your browser, paired. Each host keeps its own port (`valk remote ls`), so a
browser stays paired with each. In the desktop app, the server icon at the foot
of the rail switches hosts or connects to a new one, each in a window of its
own. While a host's window is open, its agents count on the tray and notify
you like this computer's, with the host in the title. The desktop app can't ask
for a password, so it needs your SSH key or agent.

## Agents drive their panes

An agent in a session can split its own tab and work the other panes, the way
herdr's `pane` commands do. A pane it starts never takes your keyboard.

```sh
valk pane list                     # panes of my tab: id, where it sits, state; * is me
valk pane split -d right -- npm run dev   # prints the new pane's id
valk pane run 4 "cargo test"       # type it and press Enter
valk pane wait 4 --match "test result" --timeout 300   # exits 124 on timeout
valk pane read 4 -n 40             # the last 40 lines, scrollback included
valk pane prompt 5 "fix the lint" --wait   # an agent's whole turn: prints how it
                                   # ended; exits 1 if it needs a human, 3 if the
                                   # prompt never landed
valk pane resize 4 30 · valk pane close 4
```

Agents learn of it from their session-start context.

## What happened while you were gone

`valk summary` gives a line for each agent session since you were last away, grouped
by repository: shipped, stuck, abandoned, waiting on PR/CI, still working or
done, with why ("PR #12 merged", "CI red on PR #9", "ended with uncommitted
changes") and the agent's own last line. It counts you as gone after 30
minutes without typing into any session. It's markdown, so you can paste it as
a standup.

```sh
valk summary                       # since you were last gone
valk summary --since yesterday --by day
valk summary --since 12h --json
```

The TUI has it too: `w` on the home screen. `w` steps through since you left,
12 hours, today and 3 days, `t` switches between repository and day, `↩` opens a
session that's still running and `y` copies the markdown. The web app has the
same view behind the clock on the home screen, with a Copy button.

## Is it helping?

`valk stats` reads what Valkyrie already records on this machine and shows the
current repository week by week, over the last 8 weeks (`--weeks N`, up to 13):

- **Sessions:** how often agents stopped and waited on you (questions,
  permissions, blocks, turn ends that asked you something), per session, and the
  stops a policy kept going instead.
- **Context:** the conversations that got the project's decisions, how many each
  read whole, what searches found, failed tool calls, and how many of those
  failed the same way as one a note or decision had already been written about.
- **Supervisor:** steps offered, taken as offered, taken by a policy, and the
  policy blocks you overrode.
- **Backlog:** items you and the agents filed, shipped, and the median days from
  filed to shipped, with a split by the agent and model each item last launched.

On top, a trend sets the later half of the window against the earlier half and
says better, worse or flat. The trend shows correlation, not cause: a better
number can come from anything else that changed that month too.

```sh
valk stats                       # this repository, the last 8 weeks
valk stats --repo ~/repos/app --weeks 4
valk stats --json
```

**Which features get used** is counted only with **Analytics** on (the settings
panel, `,`, or `analytics = on` in `settings.toml`; `VALK_ANALYTICS=on|off`
overrides it). It's off by default. With it on, each `valk` subcommand, MCP tool
call, and action taken in the TUI or web app (start, launch, review, search and so
on, not typing or scrolling) adds a line to `~/.local/state/valkyrie/usage.jsonl`:
when, where, the feature's name, and whether you or an agent used it. Arguments
are never recorded, nothing leaves the machine, and lines older than 90 days are
dropped. `valk stats` then lists what got used, by you and by agents, and the
commands nobody ran.

## Picking up a session you closed

`valk sessions` lists the Claude and Codex sessions of the last 31 days that
have ended, newest first: what each was about, where its work ended up and when.
`valk sessions resume N` starts number N again in a new tab, on its own
conversation, in its own directory, with the options it started with (its
first prompt isn't sent again). If its worktree has been taken away since, it
is added back on the session's branch first. A session whose conversation is
already running again isn't listed, and resuming one twice opens the same tab.

```sh
valk sessions                     # the list (also `valk sessions recent --json`)
valk sessions resume 2 --attach   # by number, or by the key `--json` gives
```

In the TUI it's `h` on the home screen, then `↩` on a session. On the phone,
the arrow-in-a-circle on the home screen, then tap one.

## Find the session where you said something

`valk search` finds the sessions whose history holds every word you give it,
in any case: your prompts, each agent's closing reply at the end of a turn, the
files it wrote or edited, and what it was about (its backlog item, topic,
branch and pull request). `"Quote"` a phrase to keep it whole. You get one hit
per session, newest first, with the time and the line that matched. Each hit
also says how to get back to that session: `valk attach N` while it's running,
or for a closed one a `valk new … -- claude --resume <id>` that picks its
conversation up again in a new session where it worked. It looks back up to 31
days, the same logs the summary reads.

```sh
valk search zoom pane                 # every session, the last 31 days
valk search '"merge guard"' --since 3d
valk search layout.rs --repo .        # only this repository
valk search flaky test --json
```

The agent's mid-turn messages aren't searched, only the reply it ended each
turn with. A hit whose worktree has since been removed says so: the agent may
not find its conversation from another folder. In the web app, the magnifier on
the home screen (or `/`) opens the same search. A running session opens on a
tap, and a closed one has a Resume button.

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

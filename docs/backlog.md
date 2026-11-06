# Backlog

Write a bug or an idea down when it comes up, on your phone or in a terminal, and
start an agent on it later with one tap. In the TUI, `Alt-a` writes one down from
anywhere, a session included, and takes you back.

```sh
valk todo add -k bug -f shot.png "Tab rename eats the last letter"
valk todo                  # what's open, and where each one is
valk todo launch 1         # claude in a worktree on fix/1-tab-rename-…, with a brief
valk todo edit 1 -k chore  # reword it (-t, -b), change its kind or repository
valk todo ship 1           # done: the worktree goes if it's clean, the branch if merged
valk todo take 4 -b fix/x  # an agent says it's on #4 (and --pr URL once it's open)
valk todo take 4 --steal   # one session per item: hand #4 to your session anyway
valk todo adopt 12         # a claude session you started by hand becomes an item (or A at home)
valk todo add --take -k bug "Off-by-one in resize" --branch fix/resize  # an agent tracks a fix nobody planned
valk todo ship --merged    # fetch, then list the items whose branch is merged; --yes ships them
valk todo note -i 4 "left: X"   # a handoff, kept for the item's next launch
valk todo accept 5         # also: show, drop, reopen, rm
```

On the phone, the list icon in the inbox opens the board: Backlog, In progress,
Review, Done. Cards move on their own as their agent works, waits on you or
finishes; one with a pull request open waits in Review unless its agent needs
you. Edit a card there, with `e` on the TUI's backlog page (`b`), or with
`valk todo edit`. A bug's brief tells the agent to reproduce it first; a feature's, to plan
and ask before writing code. Agents can add items too, as proposals you accept
before anything can start them. An agent that fixes something along the way files
and takes it in one go (`--take`), so its card follows the work; `ship --merged`
lists it as agent-filed, and shipping it accepts it.

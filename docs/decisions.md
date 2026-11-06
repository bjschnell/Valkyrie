# Project decisions

Tell Valkyrie something once, and every agent in the repo starts with it.

```sh
valk decide -k constraint "Never pkill by pattern" "It killed tool shells; kill exact pids."
valk decisions            # this repo's decisions (--all: retired ones too)
valk decisions preview    # what an agent gets at session start
valk decisions export     # copy them into .valkyrie/decisions/ (skills: .claude/skills/) for git
valk handoff 3 --to codex -a   # start codex on session 3's work, with a resume of it
valk decisions health     # what waits on you; decisions that may be out of date or not working
valk decisions mine       # propose decisions from what you keep telling agents
valk decisions learn 3    # propose a skill from what session 3 did
valk decisions scope 7 'crates/valkyrie-tui/**'   # agents get #7 when they open a TUI file
valk decisions scope --propose   # once: a small model proposes scopes for your decisions
claude mcp add valkyrie -- valk mcp   # optional: let agents search decisions
```

A decision can have a *scope*: path globs it's about. Its body then reaches an
agent the first time the agent reads or edits a file in them, instead of every
session up front. Each proposal comes with one (else it's the files its text
names); you change it on review or with `valk decisions scope`.

How it works:

1. **Agents get the rules.** Claude sessions receive the active decisions when they
   start, after `/clear`, and after compaction. Nothing is written to CLAUDE.md.
2. **Agents propose, you decide.** Agents are told they can propose decisions with
   `valk decide`, but a proposal applies only after you accept it: on the phone, on
   the TUI home screen, or with `valk decisions accept <id>`. An agent can't accept
   its own.
3. **Corrections become proposals.** When you correct an agent ("no, we use pnpm"),
   a small model (Haiku, on your own Claude login, with no tools) reads that
   exchange and proposes the rule in it. This is on for Claude sessions by default;
   `valk decisions auto all|off` changes it.
4. **What you repeat becomes proposals.** `valk decisions mine` reads this repo's
   transcripts from the last 30 days (`--days`), finds what you said three or more
   times across sessions ("did you push?"), and has the same small model propose the
   decisions that would make saying it again unnecessary. `--dry-run` only shows what
   you repeat. Codex transcripts are read only with `valk decisions auto all`, and
   with `auto off` nothing is sent. Transcripts from worktrees you've since removed
   can't be traced to the repo, so they're skipped.
5. **What goes wrong becomes proposals.** When an agent's turn ends after a tool
   failed, the same small model reads the failed call, a few lines of its error,
   and what worked next, and proposes the gotcha or fix in it, quoting the failure.
   Most failures teach nothing and are skipped. Each failure is remembered by a
   signature, so the ones that keep coming back are asked about first, and
   `valk decisions health` says when a decision's failure kept happening after you
   accepted it. Same consent as corrections: `valk decisions auto`.
6. **Procedures become skills.** A skill is a decision of kind `skill`: its title
   says when to use it, its body holds the steps. Agents get a one-line index of
   the project's skills and read one in full with `valk decisions show <id>` (or
   the MCP `get_decision`), so a long procedure costs a session one line until it's
   needed. Skills come from you (`valk decide -k skill "Cut a release" -` with the
   steps on stdin), from agents, from `valk decisions mine` when what you repeat is
   a procedure, and from `valk decisions learn <session>`, which reads what a
   session did. The daemon does that by itself when you approve a long job ("perfect,
   ship it" after 15+ tool calls). When a prompt asks for what a skill covers, the
   agent is told to read it first. When a session that followed a skill hits a
   failure, the retro may propose the skill rewritten; accepting it replaces the old
   one. A skill no agent has read in 45 days asks whether it's still needed, and is
   retired at 90. `valk decisions export` writes active skills to `.claude/skills/`
   where Claude Code loads them. In the TUI, PgUp/PgDn scrolls a long proposal;
   a skill is accepted once you've read it to the end.
7. **Policies finish routine work without asking you.** A policy is a decision of
   kind `policy` that picks a template Valkyrie's hooks enforce (ADR-0013). The
   first is `ship`: when a Claude agent is about to end its turn with work short of
   where you ship it, the hook keeps it going instead, with fixed text that has it
   run your checks and then commit, push, or open a pull request. It never merges
   and never pushes the default branch. Its settings live in two places
   (ADR-0016). How far you ship is your habit, recorded once for every project
   with `--user`. Each repository's checks are its own:

   ```sh
   printf '%s\n' 'template: ship' 'until: pr' |
     valk decide --user -k policy "Ship finished work as a pull request" -
   printf '%s\n' 'template: ship' 'ci: gh' \
     'checks: cargo fmt --check && cargo clippy && cargo test' |
     valk decide -k policy "Checks to run before shipping" -
   ```

   (The body comes on stdin with `-`, which works the same in bash, zsh and fish.)

   You rarely need to write the habit yourself. When an agent stops short of
   shipping and you answer with a routine "commit it" or "ship it", a small model
   (under `valk decisions auto`, short replies only) notes how far you sent the
   work, as the next stop shows it got there. After that happens in two
   sessions, in any repositories, the habit is proposed for you to accept, with
   your replies quoted. Reject it and it isn't offered again for 30 days.

   A repository's facts are learned the same way, from what runs in it. The
   check commands an agent runs before a push that goes through (a known test or
   lint tool, or a script, package script or make target the repository has) are
   proposed as its `checks` once two sessions ran them. CI looked up with the
   agent's own tools after a push (an MCP tool or a request about builds), or
   you asking "is CI green?" after a clean stop with a PR, in two sessions,
   proposes `ci: agent`. Nothing here asks a model: it's counting.

   `until` is `commit`, `push` or `pr`; `checks` and `ci` (`gh`, `agent` or
   `none`) are optional. A repository's settings lie over your habit, and one that
   sets `until` itself wins there. `valk decisions show <id>` on a policy prints
   what's in effect in the project and where each setting came from; `valk
   decisions --user` lists your habits. The agent gets the template, never your
   body as prose, and only once a turn. The queue doesn't show *finished* for a
   turn a policy kept going, and `valk pings` counts those stops apart. Retire the
   policy, or turn Context off, to stop it.

   The supervisor learns the same way. When a PR goes red or conflicts with its
   base after its agent stopped, the session's queue row offers one step
   (`merge-base`, `ci-fix` or `rerun`). Take the same step on the same kind of
   flag in two sessions, with no decline of it since, and a policy such as
   `template: merge-base` / `on: conflicting` is proposed. Once you accept it, that
   step runs without asking, and `valk decisions health` flags it if you keep
   prompting those sessions differently before the PR comes back clean. `valk
   pings` counts the steps it took. Turning Context off stops the automatic steps,
   and `valk supervise off` stops the supervisor altogether. The flags still show.
   It watches sessions with no backlog item too: an agent session whose branch has
   an open PR you opened is flagged the same way (someone else's PR, checked out
   to review, never is). `valk decisions health` compares how often you leave
   those flags, and override the steps on them, with flags on backlog items, so
   you can tell whether it reaches too far.

   It also keeps the backlog moving. When an accepted item nobody's on has every
   blocker shipped or dropped, and fewer than `max_running` agent sessions run in
   its repository (3 unless `settings.toml` says `max_running = N`; yours count
   too), the board offers to start it on its last agent and model: `R` starts it
   and `X` leaves it in the TUI, or Start and Leave on the web board. When nothing
   is ready, a model is asked once whether the first waiting item really needs its
   unfinished blocker. Start two items this way and a `template: launch` / `on:
   ready` policy is proposed, and once accepted it starts them on its own.
8. **Agents know about each other.** With each prompt, an agent also hears what the
   other agents in the same repo are doing: what you asked them, the files they're
   editing, and any file you've both touched. Two agents don't end up silently
   editing the same file.

## Hooks, turning it off, and where decisions are kept

This also works for a `claude` you type into a Valkyrie shell. `valk` keeps its
hooks in Claude Code's own settings (`~/.claude/settings.json`). They do nothing
outside a Valkyrie session, and they give a typed `claude` the same state tracking,
correction reading and sibling notes as a session Valkyrie started.

To switch context off, use the **Context** row in the settings (`,`), set
`context = off` in `settings.toml`, or set `VALK_PROJECT_CONTEXT=off`.
`valk setup claude --remove` takes the hooks out of Claude's settings, and
`valk setup claude` puts them back.

Every worktree of a repo shares its decisions, which are kept in
`~/.local/state/valkyrie/context` (ADR-0007 in [docs/adr](adr)).

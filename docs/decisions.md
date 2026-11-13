# Project decisions

Tell Valkyrie something once, and every agent in the repo starts with it.

```sh
valk decide -k constraint "Never pkill by pattern" "It killed tool shells; kill exact pids."
valk decisions            # this repo's decisions (--all: retired ones too)
valk decisions retire 7   # agents stop getting #7; `restore 7` brings it back
valk decisions preview    # what an agent gets at session start
valk decisions export     # copy them into .valkyrie/decisions/ (skills: .claude/skills/) for git
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
5. **What goes wrong becomes notes, then proposals.** When an agent's turn ends
   after a tool failed, the same small model reads the failed call, a few lines of
   its error, and what worked next, and writes the gotcha or fix in it as an
   [agent note](notes.md), quoting the failure. Most failures teach nothing and are
   skipped. Each failure is remembered by a signature, so the ones that keep coming
   back are asked about first. A note reaches you as a proposal once agents in
   three sessions have hit the same trap, and `valk decisions health` says when a
   decision's failure kept happening after you accepted it. Same consent as
   corrections: `valk decisions auto`.
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
7. **Policies and the supervisor finish routine work.** A decision of kind
   `policy` has the hooks keep an agent going until its work is shipped, and lets
   the supervisor take steps you always take. See [Policies](policies.md) and
   [The supervisor](supervisor.md).
8. **Agents know about each other.** With each prompt, an agent also hears what the
   other agents in the same repo are doing: what you asked them, the files they're
   editing, and any file you've both touched. Two agents don't end up silently
   editing the same file.

## Hooks, turning it off, and where decisions are kept

This also works for a `claude` you type into a Valkyrie shell. `valk` keeps its
hooks in Claude Code's own settings (`~/.claude/settings.json`). They do nothing
outside a Valkyrie session, and they give a typed `claude` the same state tracking,
correction reading and sibling notes as a session Valkyrie started.

It also sits in front of your Claude status line, which runs as before, to read how
much of your plan is used. The home screens show it, so you know there's room
before starting a long session.

To switch context off, use the **Context** row in the settings (`,`), set
`context = off` in `settings.toml`, or set `VALK_PROJECT_CONTEXT=off`.
`valk setup claude --remove` takes the hooks out of Claude's settings, and
`valk setup claude` puts them back.

Every worktree of a repo shares its decisions, which are kept in
`~/.local/state/valkyrie/context` (ADR-0007 in [docs/adr](adr)).

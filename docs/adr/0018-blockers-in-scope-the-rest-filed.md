# ADR-0018: Fixing what an agent finds is the default; filing names why it wasn't fixed

Status: accepted (2026-11-02)
Date: 2026-11-02
Amends: [ADR-0012](0012-agents-track-their-own-work.md) §4 (the instructions' wording)

## Context
ADR-0012 freed agents to fix small things they find and track them with `valk todo add --take`, and kept plain `valk todo add` for findings they "won't fix now". In practice the two read as equal options, and filing is cheaper, so agents filed fixable things: backlog #33 (valk panics when its output pipe closes) has an exact repro and a fix of a few lines, and was filed instead of fixed. "Won't fix now" gave no threshold, and a filed card didn't say why it was left, so its reader couldn't tell a deliberate skip from a dropped one.

The backlog is meant to organise work across projects and agents, not to gate it. Any wording that makes a card feel like permission to fix something pushes agents to do less.

## Decision
1. **Fixing is the default.** The session-start block and the launch brief say that fixing what an agent finds broken along the way is the default, in this work or a pull request of its own, recorded with `valk todo add --take`. They say outright that nothing needs a card before it's fixed.
2. **Filing without fixing names its reason.** Plain `valk todo add` is for findings that are too big for now, risky, need the user's call, or sit in files another agent is editing. The agent says which in the card's body, next to what and how to reproduce it.
3. **No field, no gate.** The reason is a line of prose. Nothing validates it or refuses a card without one, because friction on filing makes agents drop findings instead.

## Consequences
- More fixes land as agent-filed, taken items with branches; fewer bare cards.
- A bare card says why it was left, so whoever picks it up knows whether it's waiting on a decision or just on time.
- Skills and decisions that told agents to file rather than fix (skill #17 step 4) are revised to match.
- To check it works: compare agent-filed items taken with `--take` against bare filings over a week.

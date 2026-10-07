# ADR-0001: alacritty_terminal as the VT engine

Status: accepted (M0), revisit after M0 latency/throughput numbers
Date: 2026-10-06

## Context
The daemon keeps a parsed screen model per session (DESIGN §4.1). Candidates: `alacritty_terminal` 0.26 (Apache-2.0), `vte` 0.15 + own grid, `vt100` 0.16 (MIT), `termwiz` 0.23.

## Decision
Use `alacritty_terminal`, wrapped behind our own `overseer-term` crate so no alacritty type leaks into the protocol or clients.

Reasons, from reading the 0.26 source:
- `Term::damage()` / `reset_damage()` give per-line damage bounds → row diffs for the wire come almost free.
- `Event::PtyWrite` answers terminal queries (DA, DSR/cursor position, colors). Agent TUIs (Ink-based Claude Code, Codex) send these; a grid that cannot answer them breaks real agents.
- `Event::Bell` and `Event::Title` are cheap attention signals for M1 state detection.
- Configurable scrollback, wide-char/spacer handling, full mode tracking (`TermMode`: app cursor, bracketed paste, mouse, focus) battle-tested by a production terminal.
- `vte` alone means writing the grid ourselves; `vt100` is simpler but less complete; `termwiz` drags in wezterm's larger surface.

## Consequences
- Apache-2.0 dependency; compatible with MIT/Apache dual licensing.
- API is not stable across minor versions; the wrapper crate contains the blast radius.
- Kitty keyboard protocol left disabled in M0 (agents fall back to legacy key encoding).

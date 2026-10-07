# ADR-0002: Own PTYs in the daemon, no tmux

Status: accepted for M0 spike; confirm or reverse at end of M0
Date: 2026-10-06

## Context
AoE and others are tmux-backed. DESIGN §3 lists owned PTY + VT state as differentiator 5 (latency, reliable state detection, phone rendering).

## Decision
The daemon spawns agents with `portable-pty` 0.9 and owns the master fd. Clients never touch the PTY; they receive screen state and send input bytes over the daemon protocol. Sessions outlive clients (dtach/abduco model, with a parsed screen instead of raw replay).

## Exit criteria for M0
Keep this decision if: Claude Code and Codex render and are drivable through `overseer attach`; detach/reattach restores the screen; keystroke→echo p99 through the daemon < 16 ms locally (measured by `overseer bench latency`).

## Consequences
- We own resize, query responses, mode mirroring, and reattach repaint (tmux did these for free).
- No dependency on the user's tmux config; one screen model serves TUI, web, and heuristics.

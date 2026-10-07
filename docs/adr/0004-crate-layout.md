# ADR-0004: Cargo workspace layout

Status: accepted
Date: 2026-10-06

```
crates/
  overseer-proto/   wire types, frame codec, async client   (no alacritty, no pty)
  overseer-term/    alacritty_terminal wrapper → proto::Screen snapshots/diffs
  overseer-daemon/  session manager, PTY host, socket server, transcripts
  overseer-tui/     ratatui client: session list + attach view
  overseer/         the single binary: daemon | new | ls | attach | kill | dump | bench
```

Rules: clients depend only on `overseer-proto` (enforces "TUI uses the public API"). Adapter trait, event bus, queue, and context store arrive as new crates in M1/M2.

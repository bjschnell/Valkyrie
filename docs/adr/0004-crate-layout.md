# ADR-0004: Cargo workspace layout

Status: accepted
Date: 2026-10-06

```
crates/
  valkyrie-proto/   wire types, frame codec, async client   (no alacritty, no pty)
  valkyrie-term/    alacritty_terminal wrapper → proto::Screen snapshots/diffs
  valkyrie-daemon/  session manager, PTY host, socket server, transcripts
  valkyrie-tui/     ratatui client: session list + attach view
  valkyrie/         the single binary (`valk`): daemon | new | ls | attach | kill | dump | bench
```

Rules: clients depend only on `valkyrie-proto` (enforces "TUI uses the public API"). Adapter trait, event bus, queue, and context store arrive as new crates in M1/M2.

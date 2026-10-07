# ADR-0003: M0 wire protocol — length-prefixed JSON over a unix socket

Status: accepted (M0); HTTP/WS transport added in M3 carrying the same messages
Date: 2026-10-06

## Decision
- Transport: unix socket at `$XDG_RUNTIME_DIR/overseer/overseer.sock` (mode 0600 dir). *Superseded 2026-10-07: now `~/.local/state/valkyrie/run/<hostname>.sock`, so it survives logout and is found over SSH (DESIGN §8.1).*
- Framing: 4-byte big-endian length + JSON body (serde, tagged enums). Max frame 16 MiB.
- Messages live in `valkyrie-proto`; it is the only crate clients and daemon share.
- Screen updates are **row-granular**: `Screen { full: bool, rows: [(index, cells)], cursor, modes, size }`. A newly attached or lagging client gets `full: true`; otherwise only damaged rows are sent. Cells are run-length grouped by style (`Span { text, style }`) to keep JSON small.
- Input is raw bytes (`Vec<u8>`, a JSON number array; keystrokes are tiny so the overhead is irrelevant).
- Every request carries an id; responses echo it. Server-pushed frames (`Screen`, `Exited`) have no id.

## Consequences
- JSON is slower than CBOR/MessagePack; acceptable for M0 locally, re-evaluated with numbers (DESIGN §9).
- Same schema can later be TS-generated for the PWA.

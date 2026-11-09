# Contributing

Issues and pull requests are welcome. To run the tests:

```sh
cargo test --workspace             # Rust tests
cd web && npm ci && npm test       # web tests; `npm run build` refreshes web/dist
```

To try the web app against a throwaway daemon, give it its own state and socket and
start it with `--dev`; its `valk web` then takes `dev-token` at the pairing screen
([ADR-0010](adr/0010-dev-token.md)). Never use `--dev` for the daemon you use.

```sh
S=$(mktemp -d); chmod 700 $S
export XDG_STATE_HOME=$S VALK_SOCKET=$S/d.sock
valk daemon --dev &
valk web --listen 127.0.0.1:8833 --url http://127.0.0.1:8833
```

[DESIGN.md](../DESIGN.md) is the design and its history, and [docs/adr](adr)
holds the architecture decisions. Commits follow Conventional Commits, and versions
follow semver; [docs/releasing.md](releasing.md) says how both work and how a
release is cut.

## How it's built

Valkyrie is a Rust workspace:

| Crate | Role |
|---|---|
| `valkyrie` | the `valk` binary and CLI |
| `valkyrie-daemon` | PTY host, session manager, socket server, upgrade handoff |
| `valkyrie-term` | per-session screen model on `alacritty_terminal` |
| `valkyrie-agents` | agent adapters, state tracking, queue ranking (pure logic) |
| `valkyrie-proto` | wire protocol: messages, framing, async client |
| `valkyrie-context` | project decisions: store, project roots, the injected block |
| `valkyrie-tui` | the terminal client (ratatui) |
| `valkyrie-web` | serves the web app and bridges it to the daemon |

The web app (`web/`, React + Vite) is built into `web/dist`, which is committed and
embedded in the binary, so building `valk` needs no Node.

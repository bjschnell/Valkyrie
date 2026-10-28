---
name: verify
description: Run an isolated valk daemon + web server with fake agent sessions to observe a change at its real surface.
---

# Verifying valkyrie changes

## Isolated daemon (never the user's)
Agent shells run inside a valk session, so `VALK_SOCKET` points at the user's real
daemon. Pass an explicit socket on every call, and unset the session env.

```bash
S=<scratch dir>; mkdir -p $S/state; chmod 700 $S $S/state   # daemon refuses non-0700 dirs
V="env -u VALK_SESSION -u VALK_CONTEXT XDG_STATE_HOME=$S/state ./target/debug/valk --socket $S/d.sock"
cargo build --bin valk
$V daemon --dev &                             # foreground dev daemon (ADR-0010)
$V web --listen 127.0.0.1:<free port> --url http://127.0.0.1:<port> &   # 8790/8799 may be taken
$V new --name x -- <script>                   # fake agent: a bash script that prints a screen, then sleeps
$V dump <id>                                  # the screen the web app parses
```

- Sessions spawn at 80 columns: keep fake screen lines under 80, or the PTY wraps them.
- Web app changes ship via committed `web/dist` (embedded by `crates/valkyrie-web`):
  `cd web && npm ci && npm run build`, then rebuild `valk`.

## Web UI: pair with `dev-token`
A `--dev` daemon's web server takes `dev-token` at the pairing screen, no code. Only
`--dev` daemons do: `valk web pair` / `valk web open` still refuse agents, and the
user's own daemon never takes the dev token. Don't work around either.

Drive the tab via the Claude in Chrome extension (check `list_connected_browsers`
first; if empty, ask the user to connect it). Open `http://127.0.0.1:<port>`, type
`dev-token`, press Pair.

- The window manager may ignore `resize_window`, and the app refuses iframes. For a
  phone layout, pin `#root` to 390×844 with an injected style, and set any `dvh`
  caps to what that viewport would give.
- Toasts stay in the DOM a few seconds: note the one already showing before you act.

## Cleanup
`pkill -f "$S/d.sock"` and kill fake-agent scripts.

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
$V daemon &                                   # foreground daemon
$V web --listen 127.0.0.1:<free port> --url http://127.0.0.1:<port> &   # 8790/8799 may be taken
$V new --name x -- <script>                   # fake agent: a bash script that prints a screen, then sleeps
$V dump <id>                                  # the screen the web app parses
```

- Sessions spawn at 80 columns: keep fake screen lines under 80, or the PTY wraps them.
- Web app changes ship via committed `web/dist` (embedded by `crates/valkyrie-web`):
  `cd web && npm ci && npm run build`, then rebuild `valk`.

## Web UI: needs the user
`valk web pair` / `valk web open` refuse when run by an agent (process ancestry check,
user decision). The user runs `valk web open --socket $S/d.sock` in their own terminal;
then drive that tab via the Claude in Chrome extension (check `list_connected_browsers`
first — if empty, ask the user to connect it). Don't work around the pairing block.

## Cleanup
`pkill -f "$S/d.sock"` and kill fake-agent scripts.

# ADR-0010: A development daemon takes a fixed token instead of pairing

Status: accepted (2026-10-27)

## Context

Pairing a browser takes a code from `valk web pair` or `valk web open`, and both
refuse when an agent runs them (ADR-0009): a device token is remote, lasting access
to every session. That is right for the daemon you use, but it also blocks agents
from checking web changes against a throwaway daemon in a scratch directory, so
every browser check needs the user to run a command in their own terminal and read
a code back.

A scratch daemon guards nothing an agent lacks: whatever can start one already has
a shell as the user. What must stay out of reach is the daemon you use, so the line
is drawn there, not at "development".

## Decision

- `valk daemon --dev` starts a development daemon. It says so in `Hello` (`dev`).
  Only the flag sets it, never an environment variable, and the daemon `valk`
  starts on its own never gets the flag.
- An upgrade hands the mode across in the handoff, from the image handing off.
  The new image is exec'd with only `--resume-fd`, so a dev daemon stays one and
  nobody can turn the daemon you use into one through `valk upgrade`. A restart
  (Windows upgrades, `valk daemon` again) comes back without it.
- `valk web` accepts the fixed token `dev-token` while the daemon it bridges says
  it is a development one, on the queue watch's current connection. It starts out
  refusing it and goes back to refusing as soon as that connection drops, until
  whatever holds the socket next says `dev` again.
- The dev token is never written to `devices.json`, so it can't be listed or
  revoked, and it can't outlive the daemon's mode. The same-origin check still
  applies.
- The pairing screen takes `dev-token` as typed and stores it as the token; a
  daemon that isn't a dev one closes the socket as for a revoked token.

## Consequences

Agents verify web changes end to end without the user: start a scratch daemon with
`--dev`, start `valk web` on it, type `dev-token`. The token is public, so a dev
daemon's web server must never be reachable from another machine; it binds to
localhost unless told otherwise, as every `valk web` does.

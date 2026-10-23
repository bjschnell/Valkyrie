# ADR-0009: Add a device from a trusted one, with a short code

Status: proposed (2026-10-15)

## Context

A browser pairs by opening `<url>/#pair=<code>` from `valk web pair`. The code works
once, for 10 minutes; the device token it buys never expires. On iOS that link opens
in Safari (or Brave), never in the installed Home Screen app, which has its own
storage. So the browser tab is paired and the installed app, the one that gets push
notifications, is not, and there is no way to open a pairing link inside it. A
browser on another computer has the same gap: pairing it means reaching the host's
terminal to run `valk web pair`.

## Decision

Any trusted device can add another by showing a short code that the new device types.

- **Add a device** in a paired web app's settings, `p` in the TUI, or `valk web pair` makes a short code (e.g.
  `KQ7-4M2`). It works once, for 5 minutes, and dies after 5 wrong tries in total,
  so its shortness doesn't make it guessable. Wrong tries don't count against
  pairing links, whose 256-bit codes can't be guessed. The CLI's QR link stays as
  the first device's way in.
- The new device enters the code in the pairing screen's existing box and trades
  it at `/api/pair` for a device token, as today. No pending-request state, no
  approval step.
- `/api/pair` accepts nothing while no code is open. The server's own start prints
  only the link, so a service's log never holds an open short code.
- Wrong tries count across everyone, so anyone who can reach the server can keep
  ending short codes with five bad guesses. That is accepted: it denies, never
  grants, and the QR link still works.
- `codes.json` is changed by the server and by `valk web pair` or the TUI, so every
  change holds a file lock (`File::lock`, hence Rust 1.89).
- A short code's hash on disk can be reversed by anything that reads the file;
  anything that can read it can already write `devices.json`.
- **Who may mint:** any paired device, or the local user (TUI/CLI, vouched as today,
  so an agent can't). A paired device can already do anything the user can on the
  host, so letting it add another grants nothing new. Each device records which
  device added it, and `valk web devices` shows it. The device showing the code
  sees which device used it ("Paired laptop").
- **Wording, everywhere** (CLI, pairing screen, docs): the time limit belongs to
  the code; a paired device stays paired until revoked.

Transport is unchanged. `valk web` binds where `--listen` says; Tailscale, a reverse
proxy, or a bare port in front of it is the user's choice. Nothing assumes a tailnet.

## Not now

Tailscale identity headers as a trust source, passkeys, enforced HTTPS, and a
device list with revoke inside the web app. The web server isn't the product; the
app is. Revisit when someone needs them.

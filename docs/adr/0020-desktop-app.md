# ADR-0020: The desktop app is a native window around the web app

Status: proposed (2026-11-09)
Date: 2026-11-09

## Context

Valkyrie has two front ends: the TUI, and the web app that `valk web` serves to
phones and browsers. Most developers work in GUI apps rather than terminal ones, so
a TUI-first tool reaches few of them. A desktop app would widen that, but a third
UI would split every design decision three ways.

What actually stands between a GUI user and Valkyrie is less the TUI than the
setup: run `install.sh`, then `valk setup web`, then pair a browser with
`valk web open`. Someone who wants a desktop app expects to double-click an
installer and see a window.

Most of the parts already exist:

- The web app is one React build (`web/dist`), embedded in `valk-web` with
  `include_dir!`. It speaks to the server over one WebSocket (`web/src/net/conn.ts`)
  and reloads itself when the server it talks to has a newer build.
- `valk setup web` installs `valk web` as a user service (systemd, launchd, Task
  Scheduler), so it outlives the terminal that started it.
- `valk web open` has the CLI, running as the user, mint a one-time pairing link
  and open it on the local address. The browser it opens becomes a paired device
  (ADR-0009). That is already the local-trust path a desktop app needs.

What the web app lacks for all-day desktop use is a terminal you type into. A
session's screen renders as styled rows from the daemon, and input goes through
the composer and a handful of phone keys (`web/src/lib/keys.ts`). Watching and
answering works, but driving a shell or an agent's own UI from the keyboard does
not.

## Decision

1. **One UI.** The desktop app shows the same web app the server serves to phones
   and browsers. It never ships its own copy of `web/dist`, and it has no desktop
   layout or components of its own. Where the desktop needs something the phone
   doesn't (a session list beside the session, a keyboard terminal), it goes into
   the web app behind its existing width and input checks, so a browser on a
   desktop gets it too.
2. **A Tauri shell, in the workspace.** `crates/valkyrie-desktop` is a Tauri app.
   It's Rust like the rest of the workspace and uses the system webview, so the
   installer stays small and no Node runtime ships with it. Electron would bundle
   a second Chromium to show a page the server already serves.
3. **The window loads the local server.** The shell opens `valk web`'s local
   address in its webview, not an asset bundle. Build checks and the reload after
   an upgrade then work as in a browser, and an upgrade of `valk` updates the
   desktop UI with no release of the shell. The address is always
   `http://127.0.0.1:<port>`, never `localhost`, because the device token lives in
   per-origin `localStorage`; a different spelling or port means pairing again.
   The shell injects a flag that tells the page it's in the desktop app, and the
   page then turns off web push (it may not deliver in a system webview, and it
   would double the shell's own notifications).
4. **The shell starts what it needs, locally.** On start it makes sure the daemon
   is running and `valk web` answers. If not, it runs `valk setup web --local` as
   a separate process: the same service, bound to 127.0.0.1, without
   `tailscale serve` and without printing a pairing code. A desktop install never
   opens the server beyond the machine. Before trusting what answers on the port,
   the shell checks it's `valk web` run by this user, not whatever grabbed the
   port first: it writes a random nonce to a file only the user can read and asks
   the server to prove it read it. A process that can read that file can already
   write `devices.json`.
5. **It pairs like `valk web open`, vouched, after someone presses Pair.**
   Pairing goes through the daemon's `vouch` first ("pairing the desktop app"),
   so an agent that starts the app gets no paired device. Only then does the
   shell mint a link with `valkyrie_web::pairing_link` and load it in its
   webview. It asks when the page reports it isn't paired, at most once per
   launch, so a page can't pull codes on demand.

   Amended 2026-11-10: `valk web open` is vouched for because it runs at a
   terminal of its own, and an app started from a dock or a menu has none, so
   the daemon refused it as detached. The human step is now in the window: the
   shell shows a native dialog, "Pair this app with Valkyrie on this computer?",
   and asks the daemon only once Pair is pressed. The daemon vouches for the
   desktop app by name (as it knows the `valk web` bridge by its command line)
   when it has no terminal and no agent above it, and only for "pairing the
   desktop app": a phone's or a browser's pairing code is still for a terminal.

   What this guards: an agent that starts the app, as agents that mean no harm
   do, is refused by the agent above it, and the app it started shows a dialog
   it can't press. It is not a boundary against a process that sets out to get
   round it. The name can be taken by any program, and such a program, running
   as the user, could write `devices.json` or put itself on a terminal of its
   own as easily. The dialog is the human step for the real app; the daemon's
   check keeps honest launchers honest. The token then persists in
   the webview's storage, and the app shows in `valk web devices` as one device
   that `valk web revoke` can remove. No new token kind.
6. **Native only where the web can't reach.** The shell owns the window, a tray
   icon with the queue's count, native notifications for sessions that need you,
   and starting at login. For the count and the notifications it is its own
   daemon client over the socket, like the TUI, rather than a bridge from the
   page. Its own shortcuts are Alt/Option chords (decision #5). Ctrl-letters go
   to the page, so the terminal gets Ctrl+W as delete-word, not a closed window.
   Everything a user sees inside the window stays in the web app.

## Phases

Each phase is its own backlog item and PR.

1. **Keyboard terminal in the web app.** A terminal mode for the session view on
   a desktop-width page: key events sent as bytes following the program's modes,
   selection and copy, paste, IME, scrollback, and rendering that keeps up with a
   burst of output. This is the biggest piece of UI work, and it improves the web
   app in a desktop browser before any shell exists.
2. **Desktop layout in the web app.** At desktop width: the session list beside
   the open session, keyboard navigation between them. Still the same components.
3. **The shell.** `crates/valkyrie-desktop`: window, local-server bootstrap
   (decision 4), self-pairing (decision 5), shortcuts, tray, native notifications,
   start at login. Check WebKitGTK on Linux first: the grid's speed and the
   WebSocket's behaviour there are not yet measured.
4. **Installers and updates.** `.dmg`, `.msi` and a Linux package (AppImage or
   `.deb`) built in release CI from the release-please tags (published `v*` tags
   never move). One updater per binary: the installer puts `valk` where
   `install.sh` does, and `valk`'s own update path keeps owning it. Tauri's
   updater updates only the shell, which carries no daemon code, so the two
   never fight over the binary the daemon runs. Uninstalling revokes the app's
   device.

## Consequences

- One UI to design: every desktop improvement lands in the browser as well, and
  every phone change shows on the desktop.
- The desktop app can't run without `valk web`. That's by design (decision 3),
  but it means the shell has to own starting that server and saying clearly when
  it can't. It finds the address from how the service was set up; a user who
  changes the port pairs the window again.
- `valk setup web` gains `--local` (no Tailscale, no printed code), and the
  server gains a way to prove it's the user's own (decision 4).
- The desktop UI depends on each OS's webview: WebView2 on Windows, WKWebView on
  macOS, WebKitGTK on Linux. A rendering or input bug on one of them is a web app
  bug to fix or work around, never a reason to fork.
- Until phase 4, the window works for anyone who already installed `valk`. Until
  then, a Chromium browser's "install as app" on the local address gives most of
  the same window, minus the native parts.

## Not now

Bundling `web/dist` into the shell for offline use, a desktop app that connects to
a daemon on another machine (a browser already does that via pairing), mobile
native shells, and a plugin or extension surface in the desktop app.

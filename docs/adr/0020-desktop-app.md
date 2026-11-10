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
   address in its webview, not an asset bundle. Build checks, the reload after an
   upgrade, and the service worker then behave exactly as in a browser, and an
   upgrade of `valk` updates the desktop UI with no release of the shell.
4. **The shell sets up what it needs.** On start it makes sure the daemon is
   running and `valk web` answers on localhost. If not, it installs and starts the
   service as `valk setup web` does, bound to localhost only. A desktop install
   never opens a port beyond the machine by itself.
5. **It pairs like `valk web open`.** On first launch, or when the page reports
   it isn't paired, the shell mints a pairing link with the same library call
   (`valkyrie_web::pairing_link`) and loads it in its own webview. The webview
   keeps its device token in its own storage, so it stays paired across launches
   and shows in `valk web devices` as one device that `valk web revoke` can
   remove. No new token kind or trust rule.
6. **Native only where the web can't reach.** The shell owns: the window and its
   keyboard shortcuts (no browser taking Ctrl+W or Ctrl+T), a tray icon with the
   queue's count, native notifications for sessions that need you, starting at
   login, and updating itself. Everything a user sees inside the window stays in
   the web app.

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
   never move), with Tauri's updater pointed at them. An installer puts `valk` on
   the PATH too, so the CLI, hooks and app run one binary.

## Consequences

- One UI to design: every desktop improvement lands in the browser as well, and
  every phone change shows on the desktop.
- The desktop app can't run without `valk web`. That's by design (decision 3),
  but it means the shell has to own starting that server and saying clearly when
  it can't.
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

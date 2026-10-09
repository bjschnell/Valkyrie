# Valkyrie

An attention-first manager for coding-agent CLIs. Run Claude Code, Codex and plain
shells as sessions under one daemon, and Valkyrie tells you which one needs you next
and why. Use it from the terminal (`valk`) or from your phone.

- **Attention queue.** The home screen is a ranked inbox of sessions waiting on you,
  not a grid of terminals. It shows states such as `needs input`, `blocked`, `done`
  and `working`, read from agent hooks and the screen itself. A chime plays when
  something new needs you.
- **Sessions that outlive the TUI.** A daemon owns every PTY. Close the TUI, SSH in
  from elsewhere, or `valk upgrade` to a new build, and every session keeps running.
  After a reboot, Claude and Codex conversations resume.
- **Tabs and splits.** Every session is a tab. Put the tabs on top, left or right, as
  underline, folder or card tabs. Split a tab into panes up, down, left or right.
- **Phone.** `valk web` serves a PWA over your tailnet. It has the inbox with
  one-tap answers to prompts, a chat view built from the agent's own transcript, a
  review of what an agent changed, a way to start new sessions, dictation, and
  notifications.
- **No tmux.** Valkyrie hosts the terminals itself (alacritty's VT engine), so it
  can read state reliably and redraw for a phone.

## Install

### Prebuilt binary (Linux, WSL2, macOS arm64)

One static binary that carries the web app. The repo is private, so you need the
GitHub CLI logged in:

```sh
gh auth login
gh release download -R bjschnell/Valkyrie -p install-release.sh -O - | bash
```

This installs `valk` to `~/.local/bin`. Rerun the same line to update. A running
daemon is handed to the new binary, and every session keeps running.

### From source

```sh
git clone https://github.com/bjschnell/Valkyrie.git ~/repos/valkyrie
cd ~/repos/valkyrie && ./install.sh
```

This needs a C toolchain. `install.sh` installs Rust with rustup if you don't have
it.

Valkyrie needs a Unix. On Windows it runs inside WSL2. [docs/install.md](docs/install.md)
covers each platform, a Windows Terminal profile, and keeping WSL awake.

## Use

```sh
valk                       # open the TUI
valk new -a -- claude      # start Claude Code in a new session and attach
valk new -- codex          # start Codex in the background
valk ls                    # list sessions, with their state
valk setup codex           # once, if you use Codex: adds Valkyrie's hooks
```

Claude Code needs no setup, because its hooks are added per session.

### Keys

| Where | Key | Does |
|---|---|---|
| Home | `↩` / `⇥` | attach to the selected session / the top of the queue |
| | `n` · `x` · `R` | new shell · kill · rename |
| | `s` / `S` | mark seen / all seen |
| | `,` · `t` · `m` | settings · theme · sound |
| Attached | `^\` | tab mode (below) |
| | `^]` | back home |
| Tab mode | `h`/`l` (or `j`/`k`) · `H`/`L` | move between tabs · reorder |
| | `1`–`9` · `0` · `⇥` | jump to a tab · home · next one needing you |
| | `s` / `v` · `o` | split · move between panes |
| | `n` · `r` · `x` · `,` | new tab · rename · close · settings |

The mouse works too: click a tab, drag it to reorder, right-click it for a menu,
right-click a pane to split it, and drag a divider to resize. Click a link in a
session to open it in your browser (Ctrl+click over a program that takes the mouse,
such as vim). Over SSH the link is copied to your clipboard instead.

### Settings

`,` opens the settings panel: theme (`dracula`, `cyberpunk`, `blackout`,
`catppuccin`, `nord`, `gruvbox`, `tokyonight`), whether sessions take the theme's
colors (`theme`) or your terminal's (`terminal`), tab style (`underline`, `folder`,
`cards`), tab side (`top`, `left`, `right`), and sound. With `theme`, a session's
background, default text and 16 ANSI colors come from the theme; colors a program
picks exactly (256-color or RGB) stay as it drew them. Settings are kept in
`~/.config/valkyrie/settings.toml`, which you can also edit by hand. `VALK_THEME`,
`VALK_SESSION_COLORS`, `VALK_TABS`, `VALK_TAB_SIDE` and `VALK_SOUND` override it for
one run.

### From your phone

```sh
tailscale serve --bg 8790          # HTTPS, reachable only from your tailnet
valk new --name web -- valk web    # keep the web server running as a session
```

Scan the QR code it prints. Each code works once, for ten minutes. `valk web pair`
prints another, `valk web devices` lists paired devices, and `valk web revoke <name>`
unpairs one. The server listens only on localhost, and the daemon never listens on a
network.

## How it's built

A Rust workspace:

| Crate | Role |
|---|---|
| `valkyrie` | the `valk` binary and CLI |
| `valkyrie-daemon` | PTY host, session manager, socket server, upgrade handoff |
| `valkyrie-term` | per-session screen model on `alacritty_terminal` |
| `valkyrie-agents` | agent adapters, state tracking, queue ranking (pure logic) |
| `valkyrie-proto` | wire protocol: messages, framing, async client |
| `valkyrie-tui` | the terminal client (ratatui) |
| `valkyrie-web` | serves the web app and bridges it to the daemon |

The web app (`web/`, React + Vite) is built into `web/dist`, which is committed and
embedded in the binary, so building `valk` needs no Node.

```sh
cargo test --workspace             # Rust tests
cd web && npm ci && npm test       # web tests; `npm run build` refreshes web/dist
```

Pushing a `v*` tag builds the release binaries
(`.github/workflows/release.yml`). [DESIGN.md](DESIGN.md) is the design and its
history, and [docs/adr](docs/adr) holds the architecture decisions.

## Status

Early and in daily use by its author. Built so far: the daemon, TUI, attention queue,
splits, upgrades, restore after a restart, and the phone app. Still to come, per
DESIGN.md: the project context layer (decisions that carry across sessions and
agents) and a supervisor agent.

## License

`Cargo.toml` declares MIT OR Apache-2.0. The license files aren't in the repo yet.

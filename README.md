<div align="center">

<img src="web/public/icons/icon-512.png" width="128" alt="Valkyrie logo: a horned, braided Viking helmet in violet neon">

# Valkyrie

**Run all your coding agents in one place, and know which one needs you next.**

[![CI](https://github.com/bjschnell/Valkyrie/actions/workflows/ci.yml/badge.svg)](https://github.com/bjschnell/Valkyrie/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/bjschnell/Valkyrie?include_prereleases&label=release&color=bd93f9)](https://github.com/bjschnell/Valkyrie/releases)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-ff79c6)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584?logo=rust)](https://www.rust-lang.org)
![Platforms](https://img.shields.io/badge/runs%20on-Linux%20%C2%B7%20macOS%20%C2%B7%20Windows%20%C2%B7%20WSL2-50fa7b)

[Quick start](#quick-start) · [Screenshots](#screenshots) · [Usage](#usage) · [Phone app](#on-your-phone) · [Project decisions](#project-decisions) · [Install guide](docs/install.md)

<br>

<img src="docs/images/demo.gif" alt="The Valkyrie home screen: an agent stops on a permission prompt and moves to the top of the queue, is answered and goes back to work, and another finishes" width="100%">

</div>

---

## What is this?

Run several coding agents at once and it turns into a game of hide and seek:
which one is waiting on you? Valkyrie runs all of them in one place and points
you to where you're needed, in your terminal or on your phone. No more
searching.

## Quick start

**1. Install.** Pick the line for your system.

```sh
# Linux, WSL2, macOS (Apple silicon): one static binary, installed to ~/.local/bin
curl -fsSL https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install-release.sh | bash
```

```powershell
# Windows x64 (Windows 10 version 1809 or newer), in PowerShell
irm https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install.ps1 | iex
```

**2. Run `valk`.** Press `n` for a new shell and start `claude` or `codex` in it.
Start as many as you like. `Alt-n` takes you to the next one that needs you, and
`^]` or `Alt-h` takes you home.

```sh
valk
```

Claude Code needs no setup, since Valkyrie adds its hooks per session. If you use
Codex, run `valk setup codex` once.

<details>
<summary><b>Updating, Windows notes, and building from source</b></summary>

<br>

**Updating.** Run `valk upgrade`. It downloads the latest release, swaps it in,
and hands the running daemon to it, so every session keeps running (Windows
restarts them). The home screen says when a newer release is out. From v0.0.1,
rerun the install line once to get `valk upgrade`.

**Windows.** The PowerShell installer puts a native `valk` on your user PATH and
adds a Windows Terminal profile. Use agents installed on Windows and your Windows
repos. WSL2 is still available through `install-wsl.ps1`.

**From source.** This needs a C toolchain. `install.sh` installs Rust with rustup
if you don't have it.

```sh
git clone https://github.com/bjschnell/Valkyrie.git ~/repos/valkyrie
cd ~/repos/valkyrie && ./install.sh
```

[docs/install.md](docs/install.md) covers each platform in detail.

</details>

## Highlights

- **Many agents, side by side.** Every Claude Code, Codex or shell session is a
  tab, and any tab splits into panes. A background daemon owns them, so they keep
  running when you close the TUI or your SSH connection drops. Claude and Codex
  conversations resume after a reboot.
- **Know where you're needed.** The home screen ranks sessions by who needs you:
  `needs input`, then `blocked`, then `done`. When one changes, it chimes and
  shows what the agent asked or finished. Inside a session, each tab shows its
  agent's state, and Home shows how many need you.
- **`Alt-n` for the next one.** From anywhere, `Alt-n` jumps to the next agent
  waiting on you. Keep pressing to work through them; it brings you back where
  you started.
- **[Your agents on your phone](#on-your-phone).** Answer prompts with a tap,
  follow an agent's work as a chat, and get notified when one needs you.
- **Project memory.** Tell Valkyrie a rule once ("we use pnpm"), and every agent
  in the repo starts with it.
- **No tmux.** Valkyrie hosts the terminals itself (with alacritty's VT engine), so
  it can read state reliably and redraw the screen for a phone.

## On your phone

`valk web` serves Valkyrie as an app for your phone or any browser. Instead of a
terminal squeezed onto a small screen, you get an app made for keeping up with
agents:

- The inbox shows who needs you, with a prompt's answers as buttons. One tap
  answers it, and **Allow all** clears a run of plain yes prompts.
- Each agent's work reads as a chat, built from its transcript, with the live
  terminal a tab away.
- When an agent finishes, **Review changes** shows its diff, and one tap asks it
  to commit, run the tests or explain.
- Start Claude Code, Codex or a shell in any folder, dictate messages, and send
  files and screenshots to an agent.
- Push notifications tell you when an agent needs you, finishes or stops while
  you're away.

<p align="center">
  <img src="docs/images/phone.png" alt="The phone app: the inbox with a prompt's answers as buttons, and the chat view of a session" width="640">
</p>

```sh
valk setup web    # keep valk web running as a service, and pair with a QR code
```

`valk web` listens only on localhost, and the daemon never listens on a network.
How your devices reach it is up to you: Tailscale, a reverse proxy or a tunnel.
If Tailscale is installed, `valk setup web` puts it behind `tailscale serve`;
otherwise pass `--url` with the address your devices open. The app installs and
sends notifications only over HTTPS.

Scan the QR code it prints. The code works once, for ten minutes; the device
stays paired until you revoke it.

To add another device, such as the app installed on your Home Screen (on iPhone
it can't open the QR link) or a laptop, tap **Add a device** in an app that's
already paired, or press `p` in `valk`. Type the short code it shows into the
new device's pairing screen.

```sh
valk web pair             # print another QR code and short code
valk web open             # open it in this computer's browser, paired
valk web devices          # list paired devices
valk web revoke <name>    # unpair one
```

## Screenshots

<sub>Made-up sessions, rendered from the app's own test fixtures.</sub>

**Inside a session.** Every session is a tab that shows its program and state. The
bar at the bottom has the keys, and the number of decisions waiting for you.

<img src="docs/images/session.svg" alt="An attached Claude Code session under a tab strip showing four sessions and their states" width="100%">

**Themes.** Dracula, Cyberpunk, Blackout, Catppuccin, Nord, Gruvbox and Tokyo
Night, plus Matrix and Violet, whose home screens have digital rain.

<img src="docs/images/themes.png" alt="The home screen in the tokyonight, matrix, cyberpunk and gruvbox themes" width="100%">

## Usage

```sh
valk                       # open the TUI
valk new -a -- claude      # start Claude Code in a new session and attach
valk new -- codex          # start Codex in the background
valk new -w --name "fix login" -a -- claude   # in its own git worktree and branch
valk ls                    # list sessions, with their state
valk setup codex           # once, if you use Codex: adds Valkyrie's hooks
```

### One worktree per agent

`-w`/`--worktree [BRANCH]` starts a session in a new git worktree, so agents in the
same repo never edit one checkout. The worktree goes beside the main checkout as
`<repo>-<branch>` (for example `~/repos/app-fix-login`).

- The branch is new, made from the current `HEAD`, unless BRANCH already exists.
  Without BRANCH, it's named after `--name`.
- Uncommitted changes and ignored files (`node_modules`, `.env`) don't come along.
- Every worktree shares the repo's decisions. The phone's New session screen has
  the same option.
- Valkyrie doesn't remove worktrees. Once the work is merged, run
  `git worktree remove <path>`.

### Keybinds

A few keys get you started: `↩` on the home screen opens a session, `^]`
or `Alt-h` takes you home, and `Alt-n` steps through each session that needs you.

<details>
<summary><b>All default keys, remapping, and the mouse</b></summary>

<br>

| Where | Key | Does |
|---|---|---|
| Home | `↩` | open the selected session |
| | `n` · `x` · `R` | new shell · kill · rename |
| | `,` · `t` · `.` | settings · theme · keys |
| Anywhere | `Alt-1`–`Alt-9` · `Alt-h` | jump to a tab · home |
| | `Alt-j` / `Alt-k` · `Alt-p` | next / previous tab · the tab you were last on |
| | `Alt-n` | each session that needs you, then back where you started |
| | `Alt-t` · `Alt-w` · `Alt-o` | new shell tab · close tab · next pane |
| Attached | `^\` | tab mode (below) |
| | `^]` | back home |
| Tab mode | `h`/`l` (or `j`/`k`) · `H`/`L` | move between tabs · reorder |
| | `1`–`9` · `0` · `⇥` | jump to a tab · home · next one needing you |
| | `s` / `v` · `o` | split · move between panes |
| | `n` · `r` · `x` · `,` | new tab · rename · close · settings |
| | `m` | minimal tabs: one row each; at the side, a rail of icons that opens under the mouse |
| | `d` | decisions to review, over the session (`a`/`d` accept/reject) |

**Remapping.** Press `.` on the home screen (or in the settings panel), then `↩`
and the new key. `⌫` clears a key, `r` puts back its default, and `R` twice puts
back every default. A key can be Alt and almost any key, or Ctrl and a letter,
apart from the ones programs need (Ctrl-C, Ctrl-D, Ctrl-Z, …). `^]` always goes
home, so a remap can't lock you out. Changes are kept in `settings.toml` as
`keys.<action> = "<key>"`.

**On a Mac,** set your terminal to send Option as Meta for the Alt keys
(Terminal.app: Settings → Profiles → Keyboard; iTerm2: Profiles → Keys → Left
Option key → Esc+). Tab mode works without it.

**The mouse.** Click a tab, drag it to reorder, right-click it for a menu,
right-click a pane to split it, and drag a divider to resize. Click a link in a
session to open it in your browser (Ctrl+click over a program that takes the mouse,
such as vim). Over SSH, the link is copied to your clipboard instead.

</details>

### Settings

Press `,` to open the settings panel. You can change:

- the theme: `dracula`, `cyberpunk`, `blackout`, `catppuccin`, `nord`, `gruvbox`,
  `tokyonight`, `matrix`, `violet`, `valkyrie` or `netrunner`
- whether sessions take the theme's colors (`theme`) or your terminal's
  (`terminal`)
- the tab style (`underline`, `folder`, `cards`), side (`top`, `left`, `right`)
  and detail (`full`, or `minimal`: one row per tab; at the side, a rail of numbers
  and state icons that opens over the session under the mouse or in tab mode)
- key hints (`on`/`off`): whether the bar under a session lists its keys. Tab
  mode shows its keys either way
- the ping sound (`chime`, `pop`, `wood`, `beep`, 8-bit `coin`, `powerup`, `zap`,
  `1up`, or `none`), and project context

With `theme` colors, a session's background, default text and 16 ANSI colors come
from the theme. Colors a program picks exactly (256-color or RGB) stay as it drew
them.

<details>
<summary><b>Where settings are kept</b></summary>

<br>

Settings are kept in `~/.config/valkyrie/settings.toml`
(`%APPDATA%\Valkyrie\settings.toml` on Windows), which you can also edit by hand.
`VALK_THEME`, `VALK_SESSION_COLORS`, `VALK_TABS`, `VALK_TAB_SIDE`, `VALK_TAB_DETAIL`, `VALK_SOUND` and
`VALK_PROJECT_CONTEXT` override it for one run.

</details>

### Project decisions

Tell Valkyrie something once, and every agent in the repo starts with it.

```sh
valk decide -k constraint "Never pkill by pattern" "It killed tool shells; kill exact pids."
valk decisions            # this repo's decisions (--all: retired ones too)
valk decisions preview    # what an agent gets at session start
valk decisions export     # copy them into .valkyrie/decisions/ for git
valk handoff 3 --to codex -a   # start codex on session 3's work, with a resume of it
valk decisions health     # what waits on you; decisions that may be out of date
claude mcp add valkyrie -- valk mcp   # optional: let agents search decisions
```

How it works:

1. **Agents get the rules.** Claude sessions receive the active decisions when they
   start, after `/clear`, and after compaction. Nothing is written to CLAUDE.md.
2. **Agents propose, you decide.** Agents are told they can propose decisions with
   `valk decide`, but a proposal applies only after you accept it: on the phone, on
   the TUI home screen, or with `valk decisions accept <id>`. An agent can't accept
   its own.
3. **Corrections become proposals.** When you correct an agent ("no, we use pnpm"),
   a small model (Haiku, on your own Claude login, with no tools) reads that
   exchange and proposes the rule in it. This is on for Claude sessions by default;
   `valk decisions auto all|off` changes it.
4. **Agents know about each other.** With each prompt, an agent also hears what the
   other agents in the same repo are doing: what you asked them, the files they're
   editing, and any file you've both touched. Two agents don't end up silently
   editing the same file.

<details>
<summary><b>Hooks, turning it off, and where decisions are kept</b></summary>

<br>

This also works for a `claude` you type into a Valkyrie shell. `valk` keeps its
hooks in Claude Code's own settings (`~/.claude/settings.json`). They do nothing
outside a Valkyrie session, and they give a typed `claude` the same state tracking,
correction reading and sibling notes as a session Valkyrie started.

To switch context off, use the **Context** row in the settings (`,`), set
`context = off` in `settings.toml`, or set `VALK_PROJECT_CONTEXT=off`.
`valk setup claude --remove` takes the hooks out of Claude's settings, and
`valk setup claude` puts them back.

Every worktree of a repo shares its decisions, which are kept in
`~/.local/state/valkyrie/context` (ADR-0007 in [docs/adr](docs/adr)).

</details>

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

## Contributing

Issues and pull requests are welcome. To run the tests:

```sh
cargo test --workspace             # Rust tests
cd web && npm ci && npm test       # web tests; `npm run build` refreshes web/dist
```

[DESIGN.md](DESIGN.md) is the design and its history, and [docs/adr](docs/adr)
holds the architecture decisions. Pushing a `v*` tag builds the release binaries
(`.github/workflows/release.yml`).

## License

[AGPL-3.0](LICENSE). You can use, change and share Valkyrie freely. If you run a
modified version as a service for others, you must offer them its source.

<div align="center">
<br>
<sub>If Valkyrie catches a prompt you would have missed, a ⭐ helps other people find it.</sub>
</div>

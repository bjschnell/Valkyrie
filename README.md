<div align="center">

<img src="web/public/icons/icon-512.png" width="128" alt="Valkyrie logo: a horned, braided Viking helmet in violet neon">

# Valkyrie

**Many agents, one focus.**

An agent manager that learns how you work, so you ship faster and get
interrupted less. Claude Code, Codex and your shells run side by side, with one
queue of who's waiting, on your terminal and your phone.

[![CI](https://github.com/bjschnell/Valkyrie/actions/workflows/ci.yml/badge.svg)](https://github.com/bjschnell/Valkyrie/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/bjschnell/Valkyrie?label=release&color=bd93f9)](https://github.com/bjschnell/Valkyrie/releases)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-ff79c6)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584?logo=rust)](https://www.rust-lang.org)
![Platforms](https://img.shields.io/badge/runs%20on-Linux%20%C2%B7%20macOS%20%C2%B7%20Windows%20%C2%B7%20WSL2-50fa7b)

[Why](#why-valkyrie) · [What it does](#what-it-does) · [Phone app](#the-phone-app) · [FAQ](#faq) · [Quick start](#quick-start) · [Docs](docs/README.md)

<br>

<img src="docs/images/demo.gif" alt="The Valkyrie home screen: an agent stops on a permission prompt and moves to the top of the queue, is answered and goes back to work, and another finishes" width="100%">

<br>

```sh
curl -fsSL https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install-release.sh | bash
```

<sub>Free and open source. One binary in `~/.local/bin`, no sudo. Linux, macOS and WSL2; Windows is in the [quick start](#quick-start).</sub>

</div>

---

## Why Valkyrie

Five agents in five tabs means one sat on a prompt for twenty minutes, another
needed "we use pnpm" for the third time today, and a PR went red after its
agent stopped. Valkyrie tracks all of them and tells you which one needs you.

## What it does

### Agents that work together

- **No stepping on toes.** Each agent sees what the others are editing.
- **Hand off mid-task.** Move work from Claude to Codex with its context.
- **They track their own work.** What they find lands on your backlog.

### Learns your project

- **Teach one, teach all.** Correct one agent and the rest learn the rule, once
  you approve it.
- **Picks up your habits.** Say "ship it" twice and it offers to make that the
  default.

### So you move faster

- **Never miss a waiting agent.** `Alt-n` jumps to whoever needs you.
- **A supervisor on your PRs.** When CI breaks or main moves, the fix is one
  tap away.
- **A backlog that starts agents.** Jot a bug, tap to start an agent on it, and
  get a build order proposed.
- **Main stays yours.** No agent merges without your yes.

## The phone app

Your whole queue, built for a phone. Run `valk setup web` and scan one QR code.

<p align="center">
  <img src="docs/images/phone.png" alt="The phone app: the inbox with a prompt's answers as buttons and a decision to review, and the chat view of a session" width="640">
  <br><sub>Answer prompts with a tap, and follow each agent's work as a chat.</sub>
</p>

<p align="center">
  <img src="docs/images/phone-work.png" alt="A finished agent's changes as a diff with Commit it, Run the tests and Explain buttons, and the backlog board with a ready item offered to start" width="640">
  <br><sub>Review a diff and send "Commit it" in one tap. Run the backlog from the couch.</sub>
</p>

<p align="center">
  <img src="docs/images/phone-flag.png" alt="A session flagged because its pull request conflicts with main, with the supervisor offering Merge the base in" width="260">
  <br><sub>A PR broke while you were out? The fix is already offered.</sub>
</p>

Plus push notifications, dictation, screenshots to agents, and new agents in any folder.

## Under the hood

- Valkyrie is one static Rust binary with the web app built in, so there's
  nothing else to install.
- It doesn't use tmux. It hosts the terminals itself on alacritty's VT engine,
  which lets it read agent state reliably and redraw a session for a phone.
- A background daemon owns every terminal. Sessions keep running when you close
  the TUI or lose SSH, and Claude and Codex conversations resume after a reboot.
- `valk upgrade` hands the running daemon to the new binary, and your sessions
  keep going (Windows restarts them).
- The daemon never listens on a network. You choose how your phone reaches it,
  and Valkyrie sets up Tailscale for you if you have it.
- Eighteen themes, three of them light and five with digital rain, plus splits and full mouse
  support.

<img src="docs/images/themes.png" alt="The home screen in the tokyonight, matrix, cyberpunk and gruvbox themes" width="100%">

## FAQ

**Which agents does it work with?**
Claude Code and Codex get full state tracking and their prompts on your phone.
Learning from your corrections is on for Claude Code by default. Any other
program runs as a regular terminal tab.

**Does it change my repo or my CLAUDE.md?**
No. Rules live in Valkyrie's own store and reach agents through hooks. You can
export them to the repo if you want them in git.

**Does my code go anywhere?**
Only where your agents already send it. The small model that drafts rules is
Haiku on your own Claude login, with no tools, and one setting turns it off.

**Will an agent ever merge or push to main by itself?**
No. That always waits for you.

**Does it run on Windows?**
Yes, natively, as well as on macOS, Linux and WSL2.

**How is it different from running agents in tmux?**
tmux shows you terminals; it doesn't know what's in them. Valkyrie reads each
agent's state, so it can rank who's waiting, chime, show the prompt on your
phone and give every agent your project's rules.

**What does it cost?**
Valkyrie is free and open source under the AGPL-3.0. The features that draft
rules and write summaries make small Haiku calls on your own Claude login, which
count toward your usage. `valk decisions auto off` stops the automatic ones.

## Quick start

```sh
# Linux, WSL2, macOS (Apple silicon)
curl -fsSL https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install-release.sh | bash
```

```powershell
# Windows 10 1809+ x64, in PowerShell
irm https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install.ps1 | iex
```

Run `valk`, press `n` for a shell, and start `claude` or `codex`. Start a few
more. `Alt-n` takes you to whichever one needs you. To add your phone, run
`valk setup web` and scan the code.

## Docs

[Install](docs/install.md) · [Using Valkyrie](docs/usage.md) ·
[Keys](docs/keys.md) · [Settings](docs/settings.md) ·
[Phone app](docs/phone.md) · [Project decisions](docs/decisions.md) ·
[Policies](docs/policies.md) · [Supervisor](docs/supervisor.md) ·
[Backlog](docs/backlog.md) · [Contributing](docs/contributing.md)

## License

[AGPL-3.0](LICENSE). You can use, change and share it freely. If you run a
modified version as a service for others, you have to offer them its source.

<div align="center">
<br>
<sub>If Valkyrie catches a prompt you would have missed, a ⭐ helps other people find it.</sub>
</div>

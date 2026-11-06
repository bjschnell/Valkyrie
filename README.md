<div align="center">

<img src="web/public/icons/icon-512.png" width="128" alt="Valkyrie logo: a horned, braided Viking helmet in violet neon">

# Valkyrie

**Run all your coding agents in one place, and know which one needs you next.**

[![CI](https://github.com/bjschnell/Valkyrie/actions/workflows/ci.yml/badge.svg)](https://github.com/bjschnell/Valkyrie/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/bjschnell/Valkyrie?label=release&color=bd93f9)](https://github.com/bjschnell/Valkyrie/releases)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-ff79c6)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584?logo=rust)](https://www.rust-lang.org)
![Platforms](https://img.shields.io/badge/runs%20on-Linux%20%C2%B7%20macOS%20%C2%B7%20Windows%20%C2%B7%20WSL2-50fa7b)

[Why](#why-valkyrie) · [Highlights](#highlights) · [Under the hood](#under-the-hood) · [FAQ](#faq) · [Quick start](#quick-start) · [Docs](docs/README.md)

<br>

<img src="docs/images/demo.gif" alt="The Valkyrie home screen: an agent stops on a permission prompt and moves to the top of the queue, is answered and goes back to work, and another finishes" width="100%">

</div>

---

## Why Valkyrie

One coding agent is a tool. Five is a juggling act. One has sat on a permission
prompt for twenty minutes. You tell another, again, that the project uses pnpm.
A pull request went red an hour after its agent stopped, and nobody noticed.

Valkyrie runs Claude Code, Codex and your shells side by side, and does the
watching for you. It tells you who needs you, follows you to your phone,
remembers your rules for every agent, and keeps work moving after the agent
stops.

## Highlights

**🔔 Never miss a prompt.** Every waiting agent goes to the top of one queue,
with what it asked. `Alt-n` jumps to the next one and brings you back when
you're done.

**📱 Your agents on your phone.** Answer prompts with a tap, read an agent's work
as a chat, review its diff, and get a push notification when one needs you.

<p align="center">
  <img src="docs/images/phone.png" alt="The phone app: the inbox with a prompt's answers as buttons, and the chat view of a session" width="640">
</p>

**🧠 Agents that learn your project.** Set a rule once and every agent in the repo
starts with it. Better, Valkyrie notices your corrections and the things you keep
repeating, and proposes the rule for you. You approve; agents can't.

**🚢 Work that finishes itself.** Agents that stop at "changes are ready" get sent
on to run your checks and open the PR. A PR that goes red or conflicts with main
comes back to you with the fix one key away, and steps you always take become
automatic.

**🛡️ You stay in charge of main.** No agent merges a pull request or pushes to
your default branch without your yes, typed at your own keyboard.

**📋 A backlog that starts agents.** Jot a bug from anywhere, including your
phone. Later, one tap starts an agent on it in its own git worktree. The card
moves on its own until the PR merges.

**🤝 Agents that know about each other.** Each agent hears what the others in the
repo are editing, so two of them don't quietly rewrite the same file.

**🎨 And it looks good.** Eleven themes, five with digital rain. Splits, tabs,
full mouse support.

<img src="docs/images/themes.png" alt="The home screen in the tokyonight, matrix, cyberpunk and gruvbox themes" width="100%">

## Under the hood

- **One static Rust binary.** The web app is embedded, so there's nothing else
  to install.
- **No tmux.** Valkyrie hosts the terminals itself on alacritty's VT engine, so
  it can read agent state reliably and redraw a session for a phone.
- **Sessions that don't die.** A daemon owns every terminal, so agents survive a
  closed TUI or a dropped SSH connection, and Claude and Codex conversations
  resume after a reboot.
- **Upgrades without a restart.** `valk upgrade` hands the running daemon to the
  new binary, and every session keeps going (Windows restarts them).
- **Local first.** The daemon never listens on a network, and you choose how your
  phone reaches it (Tailscale is set up for you if you have it).

## FAQ

**Which agents does it work with?**
Claude Code and Codex get full state tracking and their prompts on your phone.
Learning from your corrections is on for Claude Code by default. Anything else
runs as a regular terminal tab.

**Does it change my repo or my CLAUDE.md?**
No. Rules live in Valkyrie's own store and reach agents through hooks. You can
export them to the repo if you want them in git.

**Does my code go anywhere?**
Only where your agents already send it. The small model that proposes rules is
Haiku on your own Claude login, with no tools, and one setting turns it off.

**Will an agent ever merge or push to main by itself?**
No. That always waits for you.

**Does it run on Windows?**
Yes, natively, plus macOS, Linux and WSL2.

**What does it cost?**
Nothing. It's open source under the AGPL-3.0.

## Quick start

```sh
# Linux, WSL2, macOS (Apple silicon)
curl -fsSL https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install-release.sh | bash
```

```powershell
# Windows 10 1809+ x64, in PowerShell
irm https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install.ps1 | iex
```

Run `valk`, press `n` for a shell, and start `claude` or `codex`. Start a few.
`Alt-n` takes you to whichever needs you. Run `valk setup web` to add your phone.

## Docs

[Install](docs/install.md) · [Using Valkyrie](docs/usage.md) ·
[Keys](docs/keys.md) · [Settings](docs/settings.md) ·
[Phone app](docs/phone.md) · [Project decisions](docs/decisions.md) ·
[Policies](docs/policies.md) · [Supervisor](docs/supervisor.md) ·
[Backlog](docs/backlog.md) · [Contributing](docs/contributing.md)

## License

[AGPL-3.0](LICENSE). Use, change and share it freely. If you run a modified
version as a service for others, offer them its source.

<div align="center">
<br>
<sub>If Valkyrie catches a prompt you would have missed, a ⭐ helps other people find it.</sub>
</div>

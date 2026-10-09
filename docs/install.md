# Installing Valkyrie

There are two ways in:

- **From a release** (no source on the machine): `install-release.sh` downloads the
  prebuilt `valk`, a single static binary, to `~/.local/bin`. See
  [Prebuilt binary](#prebuilt-binary-no-checkout).
- **From a checkout**: `./install.sh` builds `valk` from this checkout and installs it
  to `~/.cargo/bin`. It installs Rust with rustup first if you don't have it (it asks).
  Rerun it to update: if a daemon is running, the script hands it to the new binary
  with `valk upgrade`, and every session keeps running.

Valkyrie needs a Unix: PTYs, unix sockets, and process inspection. On Windows it runs
inside WSL2, and `install.ps1` sets that up ([Windows](#windows-wsl2)).

## Linux

```sh
git clone https://github.com/bjschnell/Valkyrie.git ~/repos/valkyrie
cd ~/repos/valkyrie && ./install.sh
```

A C toolchain is needed for linking (`build-essential`, `base-devel`).

## macOS

```sh
xcode-select --install            # the C toolchain, once
git clone https://github.com/bjschnell/Valkyrie.git ~/repos/valkyrie
cd ~/repos/valkyrie && ./install.sh
```

- **Terminal.** Ghostty, kitty, WezTerm and iTerm2 all work. Ghostty and kitty also show
  images (yazi previews). In Terminal.app or iTerm2, make sure Option isn't eating
  `Ctrl-\`. It shouldn't by default.
- **Pings** play through `afplay`.
- **Status:** the macOS build compiles and has been checked against the macOS SDK.
  Foreground-agent detection, `cd`-following and the upgrade handoff use macOS calls
  (`proc_pidinfo`, `KERN_PROCARGS2`, an unlinked handoff file). They haven't been run
  on a Mac yet, so check them on your first run. Start a shell tab, `cd` somewhere, and
  run `claude`. The tab should be named after the directory and show `claude ·`. Then
  run `valk upgrade` and confirm the sessions survive.

## Prebuilt binary (no checkout)

Each `v*` tag builds `valk` for Linux x86_64 and arm64 (static, musl: it runs on any
distro and any WSL, whatever its glibc) and macOS arm64, and attaches the builds, each
with a `.sha256`, to a GitHub release with `install-release.sh`. The binary carries the
web app too, so it is the only file you need.

The repo is private, so you need the GitHub CLI logged in (`gh auth login`). That
grants read access to the repo, but nothing is cloned:

```sh
gh release download -R bjschnell/Valkyrie -p install-release.sh -O - | bash
```

It installs to `~/.local/bin` (set `VALK_BIN_DIR` for another directory, `VALK_VERSION`
for a tag other than the latest), checks the checksum, and hands a running daemon to
the new binary. Rerun the same line to update. Ubuntu puts `~/.local/bin` on PATH at
login once it exists, so open a new shell after the first install.

To cut a release, from a checkout: `git tag v0.0.2 && git push origin v0.0.2`. The
`release` workflow builds and publishes it in a few minutes.

## Windows (WSL2)

Valkyrie runs inside the WSL Linux VM, and you use it from Windows Terminal. Your
agents (Claude Code, Codex) and the repos you work on live in WSL too. A Windows-side
`claude` isn't visible to Valkyrie.

**The installer** does all of it from PowerShell. With the GitHub CLI logged in
(`winget install GitHub.cli`, open a new window, `gh auth login`):

```powershell
gh release download -R bjschnell/Valkyrie -p install.ps1 -O - | Out-String | iex
```

Or download `install.ps1` from the release page in your browser and run
`powershell -ExecutionPolicy Bypass -File install.ps1`. Then it signs in inside WSL
instead, with a code to paste into github.com.

It:
1. installs WSL2 and Ubuntu if you have no distro (Windows asks for permission, and
   may want a restart; then run the same line again), or uses your default one.
   `VALK_WSL_DISTRO` picks another.
2. inside it, installs `curl`, `gh` and `pulseaudio-utils` (for pings) if they're
   missing, reuses your Windows GitHub login, and runs `install-release.sh`.
3. offers to install Claude Code inside WSL if it isn't there.
4. adds a **Valkyrie** profile to Windows Terminal, as a fragment, so your
   `settings.json` is untouched. It runs `wsl.exe -d Ubuntu --cd ~ --exec bash -lc valk`.
5. offers to keep WSL running with no window open (below).

Rerun the same line to update. Clone repos into the Linux filesystem (`~/repos`), not
`/mnt/c`: it's much faster, and file watching works. Behind a corporate proxy, set
`HTTPS_PROXY` inside WSL. Windows Terminal binds neither `Ctrl-\` nor `Ctrl-]` by
default. If either does nothing, look for a conflicting action under Settings → Actions.

**WSL shuts its VM down** a little while after the last WSL window closes, and that
stops the daemon. Sessions come back on the next `valk` (the restore list resumes
Claude and Codex conversations), but running work stops. The installer offers to
raise the idle timeout in `%UserProfile%\.wslconfig`:

```ini
[wsl2]
vmIdleTimeout=-1
```

It takes effect the next time WSL starts, or after `wsl --shutdown`.

**By hand**, the installer's steps are: `wsl --install -d Ubuntu` from an
administrator PowerShell and reboot; inside Ubuntu,
`sudo apt install -y curl gh pulseaudio-utils`, `gh auth login`, and the
[prebuilt binary](#prebuilt-binary-no-checkout) line (or `./install.sh` from a
checkout in `~/repos`, which also needs `build-essential`); then a Windows Terminal
profile with the command line above and a font with the `⌂ ● ⠋` glyphs, such as
Cascadia Mono.

## After installing

- `valk` opens the TUI. `valk new -a -- claude` starts an agent and attaches.
- `valk setup codex` once, if you use Codex: it adds Valkyrie's hooks, for accurate
  states. Claude Code needs nothing, because its hooks are added per session.
- To update, rerun the `install-release.sh` line, or `git pull && ./install.sh` in a
  checkout.
- `,` opens the settings: the theme, whether sessions take the theme's colors or
  your terminal's, the tab style (`underline`, `folder` or `cards`), which side the
  tabs are on (`top`, `left`, `right`), and sound. They are kept in
  `~/.config/valkyrie/settings.toml`, which you can also edit by hand. `VALK_THEME`,
  `VALK_SESSION_COLORS`, `VALK_TABS`, `VALK_TAB_SIDE` and `VALK_SOUND` override it
  for one run.

## Phone and web

The web app reaches your phone over Tailscale. Turn on HTTPS certificates once in the
Tailscale admin console (DNS → HTTPS Certificates); without them the page won't open.
Then:

```sh
valk setup web
```

That runs `valk web` in the background as a service (systemd on Linux, launchd on
macOS) that starts again at login, puts it on your tailnet with `tailscale serve`
(HTTPS, reachable only by your devices), and prints a QR code that pairs your phone.
Run it again after upgrading to restart the service; `valk setup web --remove` takes it
away. On Linux, `tailscale serve` needs `sudo tailscale set --operator=$USER` once, and
`loginctl enable-linger` keeps the service running while you're logged out. To run it by
hand instead: `tailscale serve --bg 8790` and `valk web`.

Scan the QR code it prints. The code works once, for ten minutes; `valk web pair`
prints another. `valk web devices` lists paired devices, and `valk web revoke <name>`
unpairs one (and stops its notifications).

**Chat.** A Claude Code or Codex session opens on its Chat tab: the conversation from the
agent's own transcript, with each tool call as a card you can open for the command, diff
or output. The Terminal tab is the live screen. A `claude` typed at a shell prompt is
found too. A Codex typed at a prompt needs `valk setup codex` once.

**Start, review, dictate.** The `+` in the inbox starts Claude Code, Codex or a shell in a
folder, optionally with a first message. When an agent finishes, **Review changes** shows
its repo's diff, and one tap tells it to commit, run the tests, or explain. Every message
box has a mic for dictation. When several prompts wait for a plain yes, **Allow all**
answers them together, after showing you each one.

**Install it and get notifications.** On iPhone, open the link in Safari, tap Share →
Add to Home Screen, and open Valkyrie from the Home Screen. iOS only offers
notifications to an installed app. On Android, use Chrome's Install app. Then tap the
bell in the app and turn notifications on. You'll get one when an agent needs you,
finishes or stops while you're away: no typing at any session for 90 seconds, and the
app not open. Notifications need the HTTPS address (`tailscale serve`), and `valk web`
has to keep running to send them.

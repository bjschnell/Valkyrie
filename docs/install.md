# Installing Valkyrie

There are two ways in:

- **From a release** (no source on the machine): `install-release.sh` downloads the
  prebuilt `valk`, a single static binary, to `~/.local/bin`. See
  [Prebuilt binary](#prebuilt-binary-no-checkout).
- **From a checkout**: `./install.sh` builds `valk` from this checkout and installs it
  to `~/.cargo/bin`. It installs Rust with rustup first if you don't have it (it asks).
  Rerun it to update: if a daemon is running, the script hands it to the new binary
  with `valk upgrade`, and every session keeps running.

Windows x64 runs natively using ConPTY and a local named pipe. The PowerShell
installer needs no administrator access. WSL2 is also available, using the separate
`install-wsl.ps1` installer.

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
distro and any WSL, whatever its glibc), macOS arm64, and Windows x64, and attaches the builds, each
with a `.sha256`, to a GitHub release with `install-release.sh`. The binary carries the
web app too, so it is the only file you need.

Nothing is cloned:

```sh
curl -fsSL https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install-release.sh | bash
```

It installs to `~/.local/bin` (set `VALK_BIN_DIR` for another directory, `VALK_VERSION`
for a tag other than the latest), checks the checksum, and hands a running daemon to
the new binary. Rerun the same line to update. Ubuntu puts `~/.local/bin` on PATH at
login once it exists, so open a new shell after the first install.

To cut a release, from a checkout: `git tag v0.0.2 && git push origin v0.0.2`. The
`release` workflow builds and publishes it in a few minutes.

## Windows (native)

Use Windows x64 with Windows 10 version 1809 or newer, or Windows 11, and Windows
Terminal. Install Claude Code or Codex on Windows and work in Windows directories.
Claude hooks use Git Bash, so install Git for Windows when using Claude Code.
Standard npm launchers for Claude and Codex are supported. Other `.cmd` or `.bat`
programs should be started from a shell tab.

```powershell
irm https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install.ps1 | iex
```

The installer downloads the Windows release, verifies its SHA-256 checksum, installs
it under `%LOCALAPPDATA%\Programs\Valkyrie`, and adds a native `valk` command to
your user PATH and a Windows Terminal profile. `VALK_VERSION` selects a release tag;
`VALK_BIN_DIR` selects another install directory. Reopen Windows Terminal to see the
profile. No Rust, WSL, or administrator access is needed.

Rerun the line to update. Each install uses a new version directory, so a running
binary can stay open while its replacement is downloaded. The `valk.cmd` launcher
runs that native executable. Older version directories are retained; remove them
once no TUI, web service, or daemon uses them. If you use `valk setup web`, rerun it
after updating so the scheduled task uses the new executable.

Closing the TUI leaves sessions running in the background. Windows upgrades restart
the daemon: Claude and Codex conversations resume, and shell sessions reopen in their
last directories, but running commands stop. This differs from Unix's live upgrade
handoff. Logging out or rebooting also stops running programs.

State and transcripts live under `%LOCALAPPDATA%\Valkyrie`, and settings under
`%APPDATA%\Valkyrie`. Shell tabs use PowerShell 7 when it is on PATH, otherwise
Windows PowerShell; `SHELL` overrides the choice. Links open in your Windows browser,
and pings play through the Windows sound API.

From source, install Rust and the Visual Studio C++ build tools, then run:

```powershell
cargo build --locked --release -p valkyrie
.\target\release\valk.exe
```

The Windows runtime has been cross-checked from Linux. The Windows CI job runs the
named-pipe and ConPTY integration tests; a Windows release should be published only
after that job passes. Local cross-compilation does not validate console input or
process lifetime behavior on a real Windows machine.

## Windows (WSL2)

Valkyrie runs inside the WSL Linux VM, and you use it from Windows Terminal. Your
agents (Claude Code, Codex) and the repos you work on live in WSL too. A Windows-side
`claude` isn't visible to Valkyrie.

**The installer** does all of it from PowerShell:

```powershell
irm https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install-wsl.ps1 | iex
```

It:
1. installs WSL2 and Ubuntu if you have no distro (Windows asks for permission, and
   may want a restart; then run the same line again), and asks for a Linux username
   and password. Otherwise it uses your default distro; `VALK_WSL_DISTRO` picks
   another.
2. inside it, installs `curl` and `pulseaudio-utils` (for pings) if they're missing,
   and runs `install-release.sh`.
3. offers to install Claude Code inside WSL if it isn't there.
4. adds a **Valkyrie** profile to Windows Terminal, as a fragment, so your
   `settings.json` is untouched. It runs `wsl.exe -d Ubuntu --cd ~ --exec bash -lc valk`.
5. adds a `valk` command for PowerShell and cmd (`%LocalAppData%\Programs\Valkyrie`,
   on your user PATH) that runs the one inside WSL.
6. offers to keep WSL running with no window open (below).

Windows Terminal reads new profiles, and new terminals pick up PATH, only when they
start. If it was open while you installed, restart it to see the profile and `valk`.

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
`sudo apt install -y curl pulseaudio-utils` and the
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
  `~/.config/valkyrie/settings.toml` (`%APPDATA%\Valkyrie\settings.toml` on Windows),
  which you can also edit by hand. `VALK_THEME`,
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
macOS, Task Scheduler on Windows) that starts again at login, puts it on your tailnet with `tailscale serve`
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

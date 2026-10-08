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
inside WSL2.

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
agents (Claude Code, Codex) and the repos you work on live in WSL too.

1. **Install WSL2** from an administrator PowerShell, then reboot:
   ```powershell
   wsl --install -d Ubuntu
   ```
   On a managed laptop this may need IT. `wsl --status` shows whether it's already
   enabled.
2. **Tools inside Ubuntu:**
   ```sh
   sudo apt update && sudo apt install -y build-essential git curl pulseaudio-utils
   ```
   `build-essential` is only needed to build from a checkout. `pulseaudio-utils` gives
   `paplay`. WSLg forwards it to Windows audio, so pings make a sound.
3. **Install valk.** Either take the [prebuilt binary](#prebuilt-binary-no-checkout),
   which keeps the source off the machine:
   ```sh
   sudo apt install -y gh && gh auth login
   gh release download -R bjschnell/Valkyrie -p install-release.sh -O - | bash
   ```
   or **clone into the Linux filesystem** (`~/repos`), not `/mnt/c`: it's much faster,
   and file watching works. The repo is private, so authenticate first, with
   `gh auth login` or an SSH key inside WSL.
   ```sh
   git clone https://github.com/bjschnell/Valkyrie.git ~/repos/valkyrie
   cd ~/repos/valkyrie && ./install.sh
   ```
   Behind a corporate proxy, set `HTTPS_PROXY` before running it, so cargo can reach
   crates.io.
4. **Install your agents inside WSL** using their Linux instructions. A Windows-side
   `claude` isn't visible to Valkyrie.
5. **A Windows Terminal profile** that opens straight into Valkyrie. Go to Settings →
   Add a new profile, and set:
   - Command line: `wsl.exe -d Ubuntu --exec bash -lc valk`. The login shell puts
     `~/.local/bin` (prebuilt) or `~/.cargo/bin` (from a checkout) on PATH.
   - Font: Cascadia Mono, JetBrains Mono or another font with the `⌂ ● ⠋` glyphs.
   - Starting directory doesn't matter; sessions keep their own.

   Windows Terminal binds neither `Ctrl-\` nor `Ctrl-]` by default. If either does
   nothing, look for a conflicting action under Settings → Actions.

**WSL shuts its VM down** a little while after the last WSL window closes, and that
stops the daemon. Sessions come back on the next `valk` (the restore list resumes
Claude and Codex conversations), but running work stops. To keep agents working with
no window open, raise the idle timeout in `%UserProfile%\.wslconfig`:

```ini
[wsl2]
vmIdleTimeout=-1
```

Then run `wsl --shutdown` once for it to take effect.

## After installing

- `valk` opens the TUI. `valk new -a -- claude` starts an agent and attaches.
- `valk setup codex` once, if you use Codex: it adds Valkyrie's hooks, for accurate
  states. Claude Code needs nothing, because its hooks are added per session.
- To update, rerun the `install-release.sh` line, or `git pull && ./install.sh` in a
  checkout.
- `,` opens the settings: the theme, the tab style (`underline`, `folder` or
  `cards`), which side the tabs are on (`top`, `left`, `right`), and sound. They are
  kept in `~/.config/valkyrie/settings.toml`, which you can also edit by hand.
  `VALK_THEME`, `VALK_TABS`, `VALK_TAB_SIDE` and `VALK_SOUND` override it for one run.

## Phone and web

`valk web` serves the web app on `127.0.0.1:8790` and prints a QR code that pairs
your phone. To reach it from your phone, put it on your tailnet (HTTPS, reachable
only by your devices):

```sh
tailscale serve --bg 8790
valk web            # keep it running, e.g. as a session: valk new --name web -- valk web
```

Scan the QR code it prints. The code works once, for ten minutes; `valk web pair`
prints another. `valk web devices` lists paired devices, and `valk web revoke <name>`
unpairs one (and stops its notifications).

**Install it and get notifications.** On iPhone, open the link in Safari, tap Share →
Add to Home Screen, and open Valkyrie from the Home Screen. iOS only offers
notifications to an installed app. On Android, use Chrome's Install app. Then tap the
bell in the app and turn notifications on. You'll get one when an agent needs you,
finishes or stops while you're away: no typing at any session for 90 seconds, and the
app not open. Notifications need the HTTPS address (`tailscale serve`), and `valk web`
has to keep running to send them.

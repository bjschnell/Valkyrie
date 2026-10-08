# Installing Valkyrie

`./install.sh` builds `valk` from this checkout and installs it to `~/.cargo/bin`. It
installs Rust with rustup first if you don't have it (it asks). Rerun it to update: if
a daemon is running, the script hands it to the new binary with `valk upgrade`, and
every session keeps running.

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
   `pulseaudio-utils` gives `paplay`. WSLg forwards it to Windows audio, so pings make a
   sound.
3. **Clone into the Linux filesystem** (`~/repos`), not `/mnt/c`: it's much faster, and
   file watching works. The repo is private, so authenticate first, with `gh auth login`
   or an SSH key inside WSL.
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
     `~/.cargo/bin` on PATH.
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
- To update, `git pull && ./install.sh`.

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
unpairs one.

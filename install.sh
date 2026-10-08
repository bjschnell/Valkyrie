#!/usr/bin/env bash
# Builds and installs `valk` from this checkout: Linux, WSL2 on Windows, macOS.
# Safe to rerun: it rebuilds, reinstalls, and hands a running daemon to the new
# binary without ending any session. See docs/install.md.
set -euo pipefail
cd "$(dirname "$0")"

say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

case "$(uname -s)" in
    Linux) os=linux ;;
    Darwin) os=macos ;;
    *) die "valk runs on Linux and macOS. On Windows, run this inside WSL2 (docs/install.md)." ;;
esac

# Rust links with the system C toolchain.
if ! command -v cc >/dev/null 2>&1; then
    if [ "$os" = macos ]; then
        die "install the Xcode command line tools first: xcode-select --install"
    fi
    die "install a C toolchain first, e.g. sudo apt install -y build-essential"
fi

if ! command -v cargo >/dev/null 2>&1; then
    if [ -x "$HOME/.cargo/bin/cargo" ]; then
        export PATH="$HOME/.cargo/bin:$PATH"
    else
        printf 'Rust is not installed. Install it now with rustup (https://rustup.rs)? [Y/n] '
        read -r answer
        case "$answer" in [nN]*) die "valk needs Rust to build" ;; esac
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
        export PATH="$HOME/.cargo/bin:$PATH"
    fi
fi

say "building valk (release) with $(cargo --version)"
cargo install --locked --path crates/valkyrie

bin="$HOME/.cargo/bin/valk"
[ -x "$bin" ] || bin="$(command -v valk)"
say "installed $("$bin" --version 2>/dev/null || echo valk) at $bin"

# A daemon from an older build keeps running its sessions; switch it over in place.
if out="$("$bin" upgrade 2>&1)"; then
    say "$out"
elif printf '%s' "$out" | grep -q "nothing to upgrade"; then
    say "no daemon running yet; the first valk command starts one"
else
    printf '%s\n' "$out" >&2
fi

case ":$PATH:" in
    *":$HOME/.cargo/bin:"*) ;;
    *)
        say "add ~/.cargo/bin to your PATH:"
        echo "    fish: fish_add_path ~/.cargo/bin"
        echo "    bash/zsh: echo 'export PATH=\"\$HOME/.cargo/bin:\$PATH\"' >> ~/.profile"
        ;;
esac

if command -v codex >/dev/null 2>&1; then
    say "Codex found: run 'valk setup codex' once for accurate Codex states"
fi
say "done. Run 'valk' to open it, or 'valk new -a -- claude' to start an agent."

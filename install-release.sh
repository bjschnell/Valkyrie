#!/usr/bin/env bash
# Installs `valk` from a GitHub release, with no checkout: Linux, WSL2 on
# Windows, macOS. On a new machine:
#   curl -fsSL https://raw.githubusercontent.com/bjschnell/Valkyrie/main/install-release.sh | bash
# Rerun it to update; a running daemon is handed to the new binary.
# VALK_VERSION picks a tag (default: the latest), VALK_BIN_DIR the directory.
set -euo pipefail

repo=bjschnell/Valkyrie
version="${VALK_VERSION:-}"
bin_dir="${VALK_BIN_DIR:-$HOME/.local/bin}"

say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) target=x86_64-unknown-linux-musl ;;
    Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
    Darwin-arm64) target=aarch64-apple-darwin ;;
    MINGW* | MSYS* | CYGWIN*) die "on Windows, run install.ps1 from PowerShell instead: it sets up WSL2 (docs/install.md)." ;;
    *) die "no prebuilt valk for $(uname -sm). On Windows, use install.ps1 (docs/install.md)." ;;
esac

command -v curl >/dev/null 2>&1 || die "install curl first, e.g. sudo apt install -y curl"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "downloading valk ${version:-(latest)} for $target"
if [ -n "$version" ]; then
    base="https://github.com/$repo/releases/download/$version"
else
    base="https://github.com/$repo/releases/latest/download"
fi
for f in "valk-$target.tar.gz" "valk-$target.tar.gz.sha256"; do
    curl -fsSL --retry 3 -o "$tmp/$f" "$base/$f" || die "couldn't download $base/$f"
done
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }
(cd "$tmp" && sha256 -c --quiet "valk-$target.tar.gz.sha256") \
    || die "checksum mismatch for valk-$target.tar.gz"
tar -xzf "$tmp/valk-$target.tar.gz" -C "$tmp"

mkdir -p "$bin_dir"
# Install beside the old binary, then rename over it: a running daemon keeps
# its open file, and `valk upgrade` re-execs the new one.
install -m 755 "$tmp/valk" "$bin_dir/.valk.new"
mv -f "$bin_dir/.valk.new" "$bin_dir/valk"
bin="$bin_dir/valk"
say "installed $("$bin" --version 2>/dev/null || echo valk) at $bin"

if out="$("$bin" upgrade 2>&1)"; then
    say "$out"
elif printf '%s' "$out" | grep -q "nothing to upgrade"; then
    say "no daemon running yet; the first valk command starts one"
else
    printf '%s\n' "$out" >&2
fi

case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *)
        say "add $bin_dir to your PATH:"
        echo "    bash/zsh: echo 'export PATH=\"$bin_dir:\$PATH\"' >> ~/.profile"
        echo "    fish: fish_add_path $bin_dir"
        ;;
esac

if command -v codex >/dev/null 2>&1; then
    say "Codex found: run 'valk setup codex' once for accurate Codex states"
fi
say "done. Run 'valk' to open it, or 'valk new -a -- claude' to start an agent."

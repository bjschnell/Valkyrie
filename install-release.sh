#!/usr/bin/env bash
# Installs `valk` from a GitHub release, with no checkout: Linux, WSL2 on
# Windows, macOS. Each release carries this script, so on a new machine:
#   gh release download -R bjschnell/Valkyrie -p install-release.sh -O - | bash
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
    *) die "no prebuilt valk for $(uname -sm). On Windows, run this inside WSL2 (docs/install.md)." ;;
esac

# The repo is private, so downloads go through an authenticated gh.
command -v gh >/dev/null 2>&1 || die "install the GitHub CLI first (https://cli.github.com), then: gh auth login"
gh auth status >/dev/null 2>&1 || die "log in to GitHub first: gh auth login"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "downloading valk ${version:-(latest)} for $target"
gh release download ${version:+"$version"} -R "$repo" -D "$tmp" \
    -p "valk-$target.tar.gz" -p SHA256SUMS
(cd "$tmp" && grep " valk-$target.tar.gz\$" SHA256SUMS | sha256sum -c --quiet -) \
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

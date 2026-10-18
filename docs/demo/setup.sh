#!/usr/bin/env bash
# A throwaway Valkyrie for recording the demo: its own daemon, settings and a
# made-up repo, so none of your real sessions or repo names end up on screen.
#
#   docs/demo/setup.sh          # start the demo daemon and its sessions
#   source /tmp/valk-demo/env   # point valk in this shell at the demo daemon
#   docs/demo/setup.sh --stop   # end the demo daemon and delete everything
#
# Every valk call names the demo socket. Inside a valk session VALK_SOCKET points
# at your real daemon, and it outranks XDG_STATE_HOME, so the env alone isn't
# enough.
set -euo pipefail

DEMO=${VALK_DEMO_DIR:-/tmp/valk-demo}
REPO=$DEMO/acme-shop
SOCK=$DEMO/state/valkyrie/run/demo.sock

demo_valk() { valk --socket "$SOCK" "$@"; }

if [[ ${1:-} == --stop ]]; then
  [[ -d $DEMO ]] || exit 0
  # Only a socket inside $DEMO is ever touched; with none, there's nothing to stop.
  if [[ -S $SOCK ]]; then
    for id in $(demo_valk ls 2>/dev/null | awk '$1 ~ /^[0-9]+$/ { print $1 }'); do
      demo_valk kill "$id" || true
    done
    # The daemon's command line names its socket, which only the demo uses.
    for pid in $(pgrep -f -- "^[^ ]*valk --socket $SOCK daemon$" || true); do kill "$pid"; done
  fi
  rm -rf "$DEMO"
  exit 0
fi

mkdir -p "$DEMO/state" "$DEMO/config"  # valk makes the socket directory, private
cat >"$DEMO/env" <<ENV
unset VALK_SESSION VALK_CONTEXT
export VALK_SOCKET=$SOCK
export XDG_STATE_HOME=$DEMO/state
export XDG_CONFIG_HOME=$DEMO/config
export VALK_THEME=valkyrie
export VALK_UPDATES=off
ENV
# shellcheck source=/dev/null
source "$DEMO/env"

if [[ ! -d $REPO/.git ]]; then
  mkdir -p "$REPO/src" "$REPO/tests"
  cat >"$REPO/README.md" <<'MD'
# acme-shop

A small web shop. Run `npm test` to run the tests.
MD
  cat >"$REPO/src/login.js" <<'JS'
export function redirectAfterLogin(user, next) {
  if (!user) return "/login";
  return next || "/";
}
JS
  cat >"$REPO/tests/login.test.js" <<'JS'
import { redirectAfterLogin } from "../src/login.js";
test("guests go to /login", () => {
  expect(redirectAfterLogin(null, "/cart")).toBe("/login");
});
JS
  git -C "$REPO" init -q
  git -C "$REPO" add -A
  git -C "$REPO" -c user.name=demo -c user.email=demo@example.com commit -qm "Start acme-shop"
fi

# One agent that will stop on a permission prompt, one that finishes, and a
# plain shell that keeps printing.
demo_valk new --cwd "$REPO" --name "fix login" -- claude \
  "Add a test that a signed-in user is sent to the 'next' page, then run npm test."
demo_valk new --cwd "$REPO" --name "docs pass" -- claude \
  "Read README.md and list any typos. Don't edit anything."
demo_valk new --cwd "$REPO" --name "dev server" -- bash -c \
  'while :; do echo "GET /cart 200 $((RANDOM % 40 + 5))ms"; sleep 2; done'
demo_valk ls

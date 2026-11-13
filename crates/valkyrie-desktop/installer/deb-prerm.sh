#!/bin/sh
# The .deb's prerm (ADR-0020, phase 4): removing the app revokes its device, so
# the token left in its webview data stops working. It runs as root and doesn't
# know who ran the app, so it acts for each user with the app's webview data,
# as that user, with that user's `valk`. Nothing here may fail the removal: an
# upgrade keeps the device, and a user with no `valk` is skipped.
set -u

case "${1:-}" in
  remove | purge) ;;
  *) exit 0 ;;
esac

command -v runuser >/dev/null 2>&1 || exit 0

getent passwd | while IFS=: read -r user _ uid _ _ home _; do
  [ "$uid" -ge 1000 ] 2>/dev/null || continue
  [ -d "$home/.local/share/dev.valkyrie.desktop" ] || continue
  for valk in "$home/.local/bin/valk" "$home/.cargo/bin/valk"; do
    [ -x "$valk" ] || continue
    # A login shell, so the user's own XDG_STATE_HOME finds their devices.
    timeout 20 runuser -l "$user" -c "'$valk' web revoke 'Desktop app'" >/dev/null 2>&1 \
      && echo "valkyrie: unpaired the desktop app for $user"
    break
  done
done

exit 0

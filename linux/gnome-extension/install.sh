#!/usr/bin/env bash
# Install (or update) the Vortex Live Activities GNOME Shell extension.
#
# On GNOME Wayland a NEWLY ADDED extension is only picked up after a fresh
# login — so after the first install, log out and back in, then enable it.
set -euo pipefail

UUID="vortex-live@vortex"
SRC="$(cd "$(dirname "$0")" && pwd)/$UUID"
DST="$HOME/.local/share/gnome-shell/extensions/$UUID"

mkdir -p "$DST"
cp "$SRC"/metadata.json "$SRC"/extension.js "$SRC"/stylesheet.css "$DST/"
echo "Installed $UUID -> $DST"
echo
echo "Next:"
echo "  1. Log out and back in (Wayland loads new extensions only at login)."
echo "  2. gnome-extensions enable $UUID"
echo "  3. Restart the Vortex daemon so it suppresses its tray fallback."

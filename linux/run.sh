#!/usr/bin/env bash
#
# Vortex laptop app — build + run (Tauri UI + embedded l3 daemon backend).
#
# WHY a prod build and not `cargo run` / the debug binary:
#   The debug build runs Tauri in "dev" mode and expects the Vite dev-server on
#   http://localhost:5172. Without it the window shows
#       "Could not connect to localhost: Connection refused".
#   The prod build (`tauri build`) EMBEDS the compiled frontend into the binary,
#   so the webview loads standalone — no dev-server, no connection-refused.
#   (--no-bundle skips the slow AppImage/deb packaging; we only want the binary.)
#
# Usage:
#   ./run.sh          build (incremental) + run the prod app
#   ./run.sh dev      hot-reload dev mode (Vite + debug binary, foreground)
#
set -euo pipefail
cd "$(dirname "$0")/ui-tauri"

# Dev mode: the standard `tauri dev` already wires Vite + the debug binary
# together, so connection-refused can't happen here. Foreground, hot-reload.
if [ "${1:-}" = "dev" ]; then
  echo "▶ tauri dev (Vite + debug, hot-reload)…"
  exec npm run tauri dev
fi

# A stale debug/release instance holds the single-instance lock; a fresh launch
# would just forward its args to that instance and exit. Replace it.
pkill -f "vortex-ui-tauri" 2>/dev/null || true
sleep 1

echo "▶ building frontend + prod binary (embeds UI; --no-bundle skips packaging)…"
npm run tauri build -- --no-bundle

BIN="src-tauri/target/release/vortex-ui-tauri"
echo "▶ launching $BIN"
# GDK_BACKEND=x11 runs the app under XWayland: under GNOME *fractional* scaling
# (e.g. 1.333×) a native-Wayland WebKitGTK window mis-places its title-bar hit
# areas — the X / minimise buttons need several clicks to register. XWayland
# renders at 1× and lets the compositor scale the whole window, so the buttons
# line up and respond on the first click. WEBKIT_DISABLE_DMABUF_RENDERER=1 still
# avoids the blank-WebKitGTK render on some GPU stacks. DISPLAY/XAUTHORITY are
# inherited from your desktop session.
exec env GDK_BACKEND=x11 WEBKIT_DISABLE_DMABUF_RENDERER=1 "$BIN"

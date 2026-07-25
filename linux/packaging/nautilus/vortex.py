"""Vortex — GNOME Files (Nautilus 4.x) right-click "Share via Vortex".

Hands the selected file path(s) to the already-running Vortex app via its
single-instance arg forwarding (`vortex-ui-tauri --share <paths…>`), which
pushes them to the paired phone (AirDrop-style, laptop → phone).

Installed to ~/.local/share/nautilus-python/extensions/ by install_linux.sh.
The binary is resolved at runtime (PATH, then ~/.local/bin) so it never hard-
codes a build path that goes stale after a move/reinstall.
"""
import gi

gi.require_version("Nautilus", "4.1")
from gi.repository import Nautilus, GObject  # noqa: E402
import os  # noqa: E402
import shutil  # noqa: E402
import subprocess  # noqa: E402


def _vortex_bin():
    return shutil.which("vortex-ui-tauri") or os.path.expanduser(
        "~/.local/bin/vortex-ui-tauri"
    )


class VortexShareExtension(GObject.GObject, Nautilus.MenuProvider):
    def get_file_items(self, files):
        usable = [f for f in files if not f.get_uri().startswith("trash:")]
        if not usable:
            return []
        item = Nautilus.MenuItem(
            name="VortexShareExtension::Share",
            label="Share via Vortex",
            tip="Send to your phone via Vortex",
        )
        item.connect("activate", self.on_activate, usable)
        return [item]

    def on_activate(self, menu, files):
        paths = []
        for f in files:
            loc = f.get_location()
            p = loc.get_path() if loc else None
            if p:
                paths.append(p)
        if paths:
            try:
                subprocess.Popen([_vortex_bin(), "--share", *paths])
            except Exception as e:
                print(f"Vortex share failed: {e}")

#!/usr/bin/env bash
#
# install_android.sh — build the Vortex phone app and install it onto a
# USB/adb-connected Android device.
#
# Thin wrapper over android/install.sh (which holds the careful JDK selection,
# clean-build rationale, MIUI grants and launch). Run it from anywhere.
#
# Usage:
#   ./install_android.sh          clean build + install (safest)
#   ./install_android.sh --fast   skip `clean` for a quick reinstall
#
# Prereqs: a phone with USB debugging on. JDK 17/21 and adb are auto-installed
# via the distro package manager if missing (sudo, once), and the Android SDK
# is bootstrapped into ~/Android/Sdk when absent. After install, grant by hand
# (can't be done over adb): Autostart (MIUI) so the service survives reboots.
set -euo pipefail

REPO="$(cd "$(dirname "$0")" && pwd)"
exec "$REPO/android/install.sh" "$@"

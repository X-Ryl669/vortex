#!/usr/bin/env bash
# Build the uinput injector that gets pushed to the phone.
#
# `vortex_inject` is a ~20 KB arm64 binary that the laptop app pushes to
# /data/local/tmp over adb (see linux/ui-tauri/src-tauri/src/mirror_inject.rs).
# It is committed in built form, because `include_bytes!` needs it at compile
# time and asking every Linux user to install the Android NDK to get a working
# mouse would be absurd. A committed binary that nobody can reproduce is a
# different problem, though — hence this script.
#
#   android/inject/build.sh              # → linux/ui-tauri/src-tauri/assets/
#   OUT=/tmp/x android/inject/build.sh   # somewhere else, to compare
#
# REPRODUCIBLE: with NDK r26b and API 29 the output is byte-for-byte identical
# to the committed binary, so you can verify the one in the repo rather than
# trust it:
#
#   OUT=/tmp/vi android/inject/build.sh
#   cmp /tmp/vi linux/ui-tauri/src-tauri/assets/vortex_inject && echo identical
#
# A different NDK still produces a working injector, just not the same bytes.
set -euo pipefail

API=29           # minSdk — matches android/app/build.gradle.kts
NDK_PINNED=26.1.10909125   # what the committed binary was built with

SRC=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$SRC/../.." && pwd)
OUT=${OUT:-$REPO/linux/ui-tauri/src-tauri/assets/vortex_inject}

# The NDK, wherever this machine keeps it. The pinned version first — it is the
# one that reproduces the committed bytes — then whatever else is installed.
find_ndk() {
  for d in "${ANDROID_NDK_HOME:-}" "${ANDROID_NDK_ROOT:-}" \
           "${ANDROID_HOME:-$HOME/Android/Sdk}/ndk/$NDK_PINNED" \
           "$HOME/Android/Sdk/ndk/$NDK_PINNED"; do
    [[ -n $d && -x $d/toolchains/llvm/prebuilt/linux-x86_64/bin/clang ]] && { echo "$d"; return; }
  done
  for d in "${ANDROID_HOME:-$HOME/Android/Sdk}"/ndk/*; do
    [[ -x $d/toolchains/llvm/prebuilt/linux-x86_64/bin/clang ]] && { echo "$d"; return; }
  done
  return 1
}

NDK=$(find_ndk) || {
  echo "✗ no Android NDK found." >&2
  echo "  Install it (sdkmanager 'ndk;$NDK_PINNED') or set ANDROID_NDK_HOME." >&2
  echo "  scripts/setup-android.sh sets up the SDK if you have neither." >&2
  exit 1
}
CC=$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android$API-clang
[[ -x $CC ]] || { echo "✗ $NDK has no aarch64 API-$API compiler" >&2; exit 1; }

"$CC" -O2 -o "$OUT" "$SRC/vortex_inject.c"

# arm64 or it is useless on the phone, and the failure would only show up as a
# cryptic "not executable" from adb.
case $(file -b "$OUT" 2>/dev/null) in
  *aarch64*) ;;
  *) echo "✗ built for the wrong architecture: $(file -b "$OUT")" >&2; exit 1 ;;
esac

echo "✓ $OUT"
echo "  $(basename "$NDK") · API $API · $(stat -c%s "$OUT") bytes"

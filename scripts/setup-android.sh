#!/usr/bin/env bash

# 📱 Vortex-Lab Android environment helper
# NOTE: This script does not auto-download Android Studio.

set -u

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

say() {
  printf '%b%s%b\n' "$GREEN" "$1" "$NC"
}

warn() {
  printf '%b%s%b\n' "$YELLOW" "$1" "$NC"
}

check_adb() {
  if command -v adb >/dev/null 2>&1; then
    say "adb is available: $(adb --version 2>&1 | head -n 1)"
    printf '\n'
    adb devices || true
  else
    warn "adb is not available on PATH yet."
  fi
}

print_header() {
  printf '%s\n' "=============================================="
  printf '%s\n' "     📱 Vortex-Lab Android Setup Guide"
  printf '%s\n' "=============================================="
}

print_links() {
  printf '\n'
  say "Android Studio"
  printf '%s\n' "Download: https://developer.android.com/studio"
  printf '%s\n' "Recommendation: install Android Studio manually rather than scripting it."
  printf '%s\n' "Reason: SDK paths, emulators, licenses, and IDE components are easier to manage interactively."
}

print_sdk_requirements() {
  printf '\n'
  say "SDK Manager requirements"
  printf '%s\n' "- Android SDK Platform 34 or newer"
  printf '%s\n' "- Android SDK Build-Tools"
  printf '%s\n' "- Android SDK Platform-Tools"
  printf '%s\n' "- Android Emulator (optional)"
}

print_jdk_note() {
  printf '\n'
  say "JDK guidance"
  printf '%s\n' "- JDK 17 is the recommended baseline for Android builds"
  printf '%s\n' "- System Java 25 may exist and can work for some tools"
  printf '%s\n' "- If Gradle or Android Studio behaves inconsistently, prefer a dedicated JDK 17"
}

print_usb_debugging_steps() {
  printf '\n'
  say "USB debugging steps"
  printf '%s\n' "1. Open Settings on the Android device"
  printf '%s\n' "2. Tap Build Number multiple times to enable Developer Options"
  printf '%s\n' "3. Open Developer Options"
  printf '%s\n' "4. Enable USB debugging"
  printf '%s\n' "5. Connect the device over USB"
  printf '%s\n' "6. Accept the host fingerprint prompt on the device"
}

print_env_vars() {
  printf '\n'
  say "Environment variables"
  printf '%s\n' "Add lines similar to the following to ~/.zshrc or ~/.bashrc:"
  printf '\n'
  printf '%s\n' 'export ANDROID_HOME=$HOME/Android/Sdk'
  printf '%s\n' 'export PATH=$ANDROID_HOME/platform-tools:$ANDROID_HOME/emulator:$ANDROID_HOME/cmdline-tools/latest/bin:$PATH'
}

print_useful_commands() {
  printf '\n'
  say "Useful adb commands"
  printf '%s\n' "adb devices"
  printf '%s\n' "adb logcat"
  printf '%s\n' "adb install path/to/app.apk"
  printf '%s\n' "adb shell"
}

print_project_context() {
  printf '\n'
  say "Project-specific notes for Vortex-Lab"
  printf '%s\n' "- Prefer testing BLE flows on a real device"
  printf '%s\n' "- Emulator usage is optional and not sufficient for transport validation"
  printf '%s\n' "- Android 10+ should be part of the validation matrix"
  printf '%s\n' "- Foreground service behavior should be tested once a1/ code exists"
}

main() {
  print_header
  print_links
  print_sdk_requirements
  print_jdk_note
  print_usb_debugging_steps
  print_env_vars
  print_useful_commands
  print_project_context
  printf '\n'
  check_adb
  printf '\n'
  say "Next step: once Android Studio and SDK components are installed, create the a1/ Gradle project skeleton."
}

main "$@"

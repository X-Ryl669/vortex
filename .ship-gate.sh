#!/usr/bin/env bash
# Repo gate for vortex-lab — a polyglot monorepo (Rust daemon in linux/, Kotlin
# app in android/) with no build tool at the root, so the generic green-tests gate
# can't autodetect one. This runs the tests that actually verify the shared
# protocol/daemon logic on every push to main.
#
# Scope: the Rust daemon holds the cross-cutting protocol, crypto-hardening and
# log-redaction tests — fast and deterministic in this env. The Android unit
# suite needs JDK 21 (system java breaks gradle) and isn't run here; verify it
# locally with `cd android && JAVA_HOME=<jdk-21> ./gradlew testDebugUnitTest`.
set -euo pipefail

cd "$(dirname "$0")/linux/daemon"
exec cargo test

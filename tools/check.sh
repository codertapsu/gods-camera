#!/bin/sh
# Pre-merge check for the shared core: tests on the Mac, then cross-builds for
# Android and Windows so platform-specific assumptions are caught early.
set -eu
cd "$(dirname "$0")/../core"
export ANDROID_HOME="${ANDROID_HOME:-/opt/homebrew/share/android-commandlinetools}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/28.2.13676358}"
export PATH="/opt/homebrew/opt/llvm/bin:$PATH"

echo "== tests (macOS) =="
cargo test -q --workspace
echo "== lint =="
cargo clippy -q --workspace --all-targets -- -D warnings
echo "== Android arm64 =="
cargo ndk -t arm64-v8a build -q --workspace
echo "== Windows x64 =="
cargo xwin build -q --workspace --target x86_64-pc-windows-msvc
echo "all checks passed"

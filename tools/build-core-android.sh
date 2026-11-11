#!/bin/sh
# Build the shared core for the Android app.
#
# Produces, inside apps/android/app:
#   src/main/jniLibs/arm64-v8a/libgc_mobile.so       the core as a shared library
#   src/main/java/uniffi/gc_mobile/gc_mobile.kt      the Kotlin interface UniFFI generates
# and, for the app's unit tests on this Mac:
#   core/target/release/libgc_mobile.dylib           the same core for the Mac's own Java
#
# All three are build products and are not kept in git. Run this once after a
# fresh clone and again whenever core/gc-mobile or core/gc-pair changes. The
# Gradle build stops with a pointer to this script while they are missing.
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/apps/android/app"
work="$root/core/target/android-ffi"
export ANDROID_HOME="${ANDROID_HOME:-/opt/homebrew/share/android-commandlinetools}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/28.2.13676358}"
[ -d "$ANDROID_NDK_HOME" ] || { echo "The Android NDK was not found at $ANDROID_NDK_HOME. Set ANDROID_NDK_HOME." >&2; exit 1; }
cd "$root/core"

echo "== core (release, 64-bit ARM Android) =="
# API level 35 is the newest this NDK carries; the app's minimum is 36 (Android 16).
# The tablet and the test emulator are both 64-bit ARM, so that is the one build.
rm -rf "$work" && mkdir -p "$work/jni" "$work/gen"
cargo ndk -t arm64-v8a --platform 35 -o "$work/jni" build -q --release -p gc-mobile
mkdir -p "$app/src/main/jniLibs/arm64-v8a"
cp "$work/jni/arm64-v8a/libgc_mobile.so" "$app/src/main/jniLibs/arm64-v8a/libgc_mobile.so"

echo "== core (release, this Mac, for unit tests) =="
cargo build -q --release -p gc-mobile

echo "== Kotlin bindings =="
# The generator is built from the same UniFFI version the library uses. It
# reads the interface out of the library itself.
cargo run -q --release -p gc-ffi --bin uniffi-bindgen -- \
  generate --library target/release/libgc_mobile.dylib --language kotlin --no-format --out-dir "$work/gen"
rm -rf "$app/src/main/java/uniffi"
mkdir -p "$app/src/main/java/uniffi/gc_mobile"
cp "$work/gen/uniffi/gc_mobile/gc_mobile.kt" "$app/src/main/java/uniffi/gc_mobile/gc_mobile.kt"
echo "built $(du -h "$app/src/main/jniLibs/arm64-v8a/libgc_mobile.so" | cut -f1 | tr -d ' ') library and $(wc -l < "$app/src/main/java/uniffi/gc_mobile/gc_mobile.kt" | tr -d ' ') lines of Kotlin"

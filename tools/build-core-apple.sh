#!/bin/sh
# Build the shared core for the Mac app.
#
# Produces, inside apps/apple/GodsCameraKit:
#   Frameworks/GCCoreFFI.xcframework   the core as a static library with its C header
#   Sources/GCCore/gc_ffi.swift        the Swift interface UniFFI generates
#
# Both are build products and are not kept in git. Run this once after a
# fresh clone and again whenever anything under core/ changes.
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
kit="$root/apps/apple/GodsCameraKit"
work="$root/core/target/apple-ffi"
cd "$root/core"

echo "== core (release, Apple silicon) =="
cargo build -q --release -p gc-ffi --target aarch64-apple-darwin
lib="target/aarch64-apple-darwin/release"

echo "== Swift bindings =="
rm -rf "$work" && mkdir -p "$work/gen" "$work/headers"
cargo run -q --release -p gc-ffi --bin uniffi-bindgen -- \
  generate --library "$lib/libgc_ffi.dylib" --language swift --out-dir "$work/gen"
cp "$work/gen/gc_ffiFFI.h" "$work/headers/"
# Xcode looks for a file called module.modulemap.
cp "$work/gen/gc_ffiFFI.modulemap" "$work/headers/module.modulemap"

echo "== XCFramework =="
rm -rf "$kit/Frameworks/GCCoreFFI.xcframework"
mkdir -p "$kit/Frameworks" "$kit/Sources/GCCore"
xcodebuild -create-xcframework \
  -library "$lib/libgc_ffi.a" -headers "$work/headers" \
  -output "$kit/Frameworks/GCCoreFFI.xcframework" >/dev/null
cp "$work/gen/gc_ffi.swift" "$kit/Sources/GCCore/gc_ffi.swift"
echo "built $(du -sh "$kit/Frameworks/GCCoreFFI.xcframework" | cut -f1) framework and $(wc -l < "$kit/Sources/GCCore/gc_ffi.swift" | tr -d ' ') lines of Swift"

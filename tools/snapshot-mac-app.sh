#!/bin/sh
# Make pictures of every screen of the Mac app, filled with made-up content.
#
# The app draws its window off screen and leaves the camera, the hub and the
# camera extension alone, so this is safe beside a running copy of the app.
# Build first with tools/install-mac-app.sh --build-only.
#
# Usage: tools/snapshot-mac-app.sh <folder> [language]     language: en (default) or vi
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
product="$root/apps/apple/build/Build/Products/Debug/GodsCamera.app"
lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
[ $# -ge 1 ] || { echo "usage: $0 <folder> [language]" >&2; exit 2; }
[ -x "$product/Contents/MacOS/GodsCamera" ] || { echo "Build the app first: tools/install-mac-app.sh --build-only" >&2; exit 1; }
language="${2:-en}"
"$product/Contents/MacOS/GodsCamera" --snapshot "$1" -AppleLanguages "($language)" -AppleLocale "${language}_VN" &
pid=$!
# The app quits by itself when the pictures are written.
( sleep 45; kill "$pid" 2>/dev/null ) &
watchdog=$!
wait "$pid" 2>/dev/null || true
kill "$watchdog" 2>/dev/null || true
# Running the build folder's copy must not make it the copy that opens by name.
"$lsregister" -u "$product" >/dev/null 2>&1 || true
ls "$1"

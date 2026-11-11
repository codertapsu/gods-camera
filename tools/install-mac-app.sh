#!/bin/sh
# Build the Mac app and put it in /Applications, where macOS requires an app
# that installs a camera extension to live.
#
# It refuses to replace the app while a copy of it is running. Replacing an
# app under its running copy leaves that copy half old and half new, and
# macOS may then start a second copy from the build folder.
#
# Usage: tools/install-mac-app.sh [--core] [--build-only]
#   --core        rebuild the Rust core first
#   --build-only  build, and leave /Applications alone. This works while the app runs.
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
product="$root/apps/apple/build/Build/Products/Debug/GodsCamera.app"
lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
core=no
install=yes
for argument in "$@"; do
  case "$argument" in
    --core) core=yes ;;
    --build-only) install=no ;;
    *) echo "unknown option: $argument" >&2; exit 2 ;;
  esac
done

if [ "$install" = yes ] && pgrep -x GodsCamera >/dev/null; then
  echo "God's Camera is running. Quit it first (menu-bar icon, Quit God's Camera), then run this again." >&2
  exit 1
fi
[ "$core" = yes ] && "$root/tools/build-core-apple.sh"
cd "$root/apps/apple"
xcodegen generate >/dev/null
xcodebuild -project GodsCamera.xcodeproj -scheme GodsCamera -configuration Debug -derivedDataPath build \
  -allowProvisioningUpdates build 2>&1 | grep -E "warning: |error:|BUILD" | grep -v "appintentsmetadataprocessor\|gc_ffi.swift\|export " | sort -u
[ -d "$product" ] || { echo "The build did not produce the app." >&2; exit 1; }
# Xcode registers what it builds, so the build folder's copy could be started
# by name from Spotlight or the Dock. Only the copy in /Applications should be.
"$lsregister" -u "$product" >/dev/null 2>&1 || true
if [ "$install" = no ]; then
  echo "built $product"
  exit 0
fi
if pgrep -x GodsCamera >/dev/null; then
  echo "God's Camera was opened during the build. Quit it, then run this again." >&2
  exit 1
fi
rm -rf /Applications/GodsCamera.app
ditto "$product" /Applications/GodsCamera.app
echo "installed God's Camera $(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' /Applications/GodsCamera.app/Contents/Info.plist) build $(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' /Applications/GodsCamera.app/Contents/Info.plist)"

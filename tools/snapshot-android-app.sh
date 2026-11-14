#!/bin/zsh
# Make pictures of the Android app's screens on the test emulator gc_test.
#
# Starts gc_test without a window unless an emulator already runs on the
# port below, installs the debug build, shows each screen through the app's
# debug_screen switch, in English and in Vietnamese, and stops the emulator
# again if it started it. The screens show made-up content: no camera, photo
# library or real computer is used. Never runs on the tablet or on xq_api36.
#
# Build first: (cd apps/android && ./gradlew :app:assembleDebug)
# Usage: tools/snapshot-android-app.sh <folder> [screen ...]
#        screens: remote (default), connect-sample, address, code, and the
#        others ScanViewModel.debugShow knows
set -u
[ $# -ge 1 ] || { echo "usage: $0 <folder> [screen ...]" >&2; exit 2; }
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$1"; shift
screens=("$@"); [ ${#screens[@]} -gt 0 ] || screens=(remote)
export ANDROID_HOME="${ANDROID_HOME:-/opt/homebrew/share/android-commandlinetools}"
adb="$ANDROID_HOME/platform-tools/adb"
apk="$root/apps/android/app/build/outputs/apk/debug/app-debug.apk"
[ -f "$apk" ] || { echo "Build the app first: (cd apps/android && ./gradlew :app:assembleDebug)" >&2; exit 1; }
port=5580
serial="emulator-$port"
mkdir -p "$out"

started=""
if ! "$adb" devices | grep -q "^$serial"; then
  "$ANDROID_HOME/emulator/emulator" -avd gc_test -no-window -no-audio -no-boot-anim -no-snapshot-save -port $port > /tmp/gc-emulator.log 2>&1 &
  started=$!
fi
"$adb" -s $serial wait-for-device
for i in $(seq 1 150); do
  [ "$("$adb" -s $serial shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = 1 ] && break
  sleep 2
done
"$adb" -s $serial install -r "$apk" > /dev/null || { echo "could not install the app" >&2; exit 1; }

for language in en-US vi-VN; do
  "$adb" -s $serial shell cmd locale set-app-locales dev.godscamera.android --locales $language
  for screen in "${screens[@]}"; do
    "$adb" -s $serial shell am force-stop dev.godscamera.android
    "$adb" -s $serial shell am start -W -n dev.godscamera.android/.MainActivity --es debug_screen "$screen" > /dev/null
    sleep 6
    "$adb" -s $serial exec-out screencap -p > "$out/$screen-${language%%-*}.png"
  done
done
"$adb" -s $serial shell cmd locale set-app-locales dev.godscamera.android --locales en-US

if [ -n "$started" ]; then
  "$adb" -s $serial emu kill > /dev/null 2>&1
  wait $started 2>/dev/null
fi
ls "$out"

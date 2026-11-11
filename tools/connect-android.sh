#!/bin/sh
# Let an Android device on a USB cable reach the hub on this Mac.
#
# The hub in development mode listens only on this Mac's own address. This
# makes the device's localhost port 47800 lead to the Mac's, through the cable.
# Run it again after re-plugging the cable.
set -eu
port="${1:-47800}"
adb="$(command -v adb || true)"
[ -n "$adb" ] || adb="/opt/homebrew/share/android-commandlinetools/platform-tools/adb"
[ -x "$adb" ] || { echo "adb was not found. Install the Android platform tools first." >&2; exit 1; }
devices="$("$adb" devices | awk 'NR > 1 && $2 == "device" { print $1 }')"
[ -n "$devices" ] || { echo "No device found. Connect it by cable, unlock it and allow USB debugging." >&2; exit 1; }
for device in $devices; do
  "$adb" -s "$device" reverse "tcp:$port" "tcp:$port" >/dev/null
  echo "$("$adb" -s "$device" shell getprop ro.product.model | tr -d '\r') can now reach the hub at http://127.0.0.1:$port"
done

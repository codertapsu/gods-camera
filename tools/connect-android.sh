#!/bin/sh
# Let an Android device reach the hub on this Mac. The device must be
# connected for debugging, by USB cable or by Wi-Fi debugging; both work.
#
# The hub in development mode listens only on this Mac's own address. This
# makes the device's localhost port 47800 lead to the Mac's, through the
# debugging connection, and hands the hub's token to the app on the device,
# so it need not be typed. Run it again after the connection was lost.
#
# The token is read from the Keychain, where the Mac app keeps it. macOS asks
# for your permission the first time; choose Always Allow to skip the question
# next time. Turn the scanner on in the Mac app once before running this.
set -eu
port="${1:-47800}"
adb="$(command -v adb || true)"
[ -n "$adb" ] || adb="/opt/homebrew/share/android-commandlinetools/platform-tools/adb"
[ -x "$adb" ] || { echo "adb was not found. Install the Android platform tools first." >&2; exit 1; }
devices="$("$adb" devices | awk 'NR > 1 && $2 == "device" { print $1 }')"
[ -n "$devices" ] || { echo "No device found. Connect it by cable or turn on Wi-Fi debugging, and unlock it." >&2; exit 1; }
token="${GC_HUB_TOKEN:-$(security find-generic-password -s godscamera.hub -a token -w 2>/dev/null || true)}"
for device in $devices; do
  model="$("$adb" -s "$device" shell getprop ro.product.model | tr -d '\r')"
  "$adb" -s "$device" reverse "tcp:$port" "tcp:$port" >/dev/null
  echo "$model can now reach the hub at http://127.0.0.1:$port"
  if [ -z "$token" ]; then
    echo "No token found. Turn on the scanner in the Mac app, then run this again, or type the token into the app."
  elif "$adb" -s "$device" shell am start -n dev.godscamera.android/.MainActivity --es hub_token "$token" >/dev/null 2>&1; then
    echo "$model: the app was opened and given the hub's token"
  else
    echo "$model: the app is not installed, or the device is locked. Install it, unlock the device and run this again."
  fi
done

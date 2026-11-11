#!/bin/sh
# Bring the Mac app's string catalogue in line with the strings the app shows.
#
# The compiler lists every string in the app while building it. This adds the
# new ones to Localizable.xcstrings and marks the ones no longer used, then
# says which strings still lack a Vietnamese translation.
# Build first with tools/install-mac-app.sh --build-only.
#
# Usage: tools/sync-mac-strings.sh [--prune]     --prune removes strings the app no longer shows
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
catalogue="$root/apps/apple/GodsCamera/Localizable.xcstrings"
objects="$root/apps/apple/build/Build/Intermediates.noindex/GodsCamera.build/Debug/GodsCamera.build/Objects-normal/arm64"
ls "$objects"/*.stringsdata >/dev/null 2>&1 || { echo "Build the app first: tools/install-mac-app.sh --build-only" >&2; exit 1; }
set -- "${1:-}"
find "$objects" -name '*.stringsdata' ! -name 'ExtractedAppShortcutsMetadata.stringsdata' -print0 \
  | xargs -0 xcrun xcstringstool sync "$catalogue" --stringsdata
/usr/bin/python3 - "$catalogue" "$1" <<'PY'
import json, sys
path, option = sys.argv[1], sys.argv[2]
catalogue = json.load(open(path))
strings = catalogue["strings"]
stale = [key for key, value in strings.items() if value.get("extractionState") == "stale"]
if option == "--prune":
    for key in stale:
        del strings[key]
    json.dump(catalogue, open(path, "w"), ensure_ascii=False, indent=2, sort_keys=True)
    open(path, "a").write("\n")
    stale = []
missing = [
    key for key, value in strings.items()
    if key.strip() and value.get("localizations", {}).get("vi", {}).get("stringUnit", {}).get("state") != "translated"
    and "variations" not in value.get("localizations", {}).get("vi", {})
]
print(f"{len(strings)} strings, {len(missing)} without Vietnamese, {len(stale)} no longer used")
for key in missing:
    print("  untranslated:", json.dumps(key, ensure_ascii=False))
sys.exit(1 if missing else 0)
PY

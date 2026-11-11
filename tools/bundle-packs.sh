#!/bin/sh
# Copy the models the Mac app cannot work without into the app bundle.
#
# Run by Xcode as a build phase. Each file is checked against the SHA-256 in
# its manifest before it is copied, and a file already in place with the right
# size is left alone, so only the first build pays for the copy.
#
# The model store sits outside the repository, beside it, in ../packs.
set -eu
repo="${SRCROOT}/../.."
store="${GC_PACK_STORE:-$repo/../packs}"
destination="${TARGET_BUILD_DIR}/${UNLOCALIZED_RESOURCES_FOLDER_PATH}/Packs"
# The base pack: detector, identity encoder, and the swap model for the Neural Engine.
files="buffalo_l/det_10g.onnx buffalo_l/w600k_r50.onnx inswapper_128.reflectfree.fp16.onnx inswapper_128.emap.bin"

if [ ! -d "$store" ]; then
  echo "warning: model store not found at $store; the app is built without models and face swap will be unavailable"
  exit 0
fi
for file in $files; do
  source="$store/$file"
  target="$destination/$file"
  if [ ! -f "$source" ]; then
    echo "warning: $file is missing from the model store; face swap will be unavailable"
    continue
  fi
  if [ -f "$target" ] && [ "$(stat -f %z "$source")" = "$(stat -f %z "$target")" ]; then
    continue
  fi
  expected="$(/usr/bin/python3 - "$repo/packs" "$file" <<'PY'
import json, pathlib, sys
for manifest in pathlib.Path(sys.argv[1]).glob("*.json"):
    for entry in json.loads(manifest.read_text())["files"]:
        if entry["path"] == sys.argv[2]:
            print(entry["sha256"])
PY
)"
  actual="$(shasum -a 256 "$source" | cut -d' ' -f1)"
  if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
    echo "error: $file does not match its manifest (expected ${expected:-no entry}, found $actual)"
    exit 1
  fi
  mkdir -p "$(dirname "$target")"
  cp "$source" "$target"
  echo "bundled $file"
done

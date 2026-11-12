#!/usr/bin/env python3
"""Make the reference pictures the core's parity tests compare against.

The tests in core/gc-infer and core/gc-live compare the Rust pipeline with
InsightFace's own Python pipeline on its sample photo. The numbers from that
run are in git (golden.json, det_raw.bin); the pictures are not, because the
sample photo is not ours to redistribute. Each machine makes them once:

    python tools/make-reference.py

This runs tools/modelkit/reference_run.py through uv into a scratch folder,
copies the pictures into core/gc-infer/tests/golden, and says how far this
machine's numbers are from the ones in git. Small differences are expected:
the same model gives slightly different numbers on different processors.

It needs the model store (python tools/fetch-packs.py) and uv.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
MODELKIT = REPO / "tools" / "modelkit"
GOLDEN = REPO / "core" / "gc-infer" / "tests" / "golden"


def largest_difference(a, b) -> float:
    """The largest difference between two JSON values of the same shape."""
    if isinstance(a, dict) and isinstance(b, dict):
        return max((largest_difference(a[key], b[key]) for key in a if key in b), default=0.0)
    if isinstance(a, list) and isinstance(b, list):
        return max((largest_difference(x, y) for x, y in zip(a, b)), default=0.0)
    if isinstance(a, (int, float)) and isinstance(b, (int, float)) and not isinstance(a, bool):
        return abs(a - b)
    return 0.0


def floats(path: Path) -> list[float]:
    data = path.read_bytes()
    return list(struct.unpack(f"<{len(data) // 4}f", data))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--store", type=Path, default=Path(os.environ.get("GC_PACK_STORE") or REPO.parent / "packs"))
    args = parser.parse_args()
    if shutil.which("uv") is None:
        print("uv is not installed: https://docs.astral.sh/uv/")
        return 1
    with tempfile.TemporaryDirectory(prefix="make-reference-") as scratch:
        command = [
            "uv", "run", "--project", str(MODELKIT), "python", str(MODELKIT / "reference_run.py"),
            "--packs", str(args.store.resolve()), "--out", scratch,
        ]
        if subprocess.run(command).returncode != 0:
            print("the reference run failed")
            return 1
        made = Path(scratch)
        pictures = sorted(made.glob("*.png"))
        for picture in pictures:
            shutil.copyfile(picture, GOLDEN / picture.name)
        print(f"copied {len(pictures)} pictures to {GOLDEN}")

        # How this machine's numbers compare with the ones in git.
        here, kept = json.loads((made / "golden.json").read_text()), json.loads((GOLDEN / "golden.json").read_text())
        for part in ("det", "align", "swap"):
            if part in here and part in kept:
                print(f"  {part:6} largest difference from git: {largest_difference(here[part], kept[part]):.3f}")
        a, b = floats(made / "det_raw.bin"), floats(GOLDEN / "det_raw.bin")
        if len(a) == len(b):
            print(f"  detector outputs, largest difference from git: {max(abs(x - y) for x, y in zip(a, b)):.3f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

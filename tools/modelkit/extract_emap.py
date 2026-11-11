"""Write the identity projection matrix of inswapper_128 to its own file.

The matrix ("emap") is the last initialiser in the model file. InsightFace's
Python code reads it from there; the Rust core reads this small file instead,
so it does not need an ONNX parser.

Output: 512 x 512 little-endian float32, row-major.

Usage: uv run python extract_emap.py --packs ../../../packs
"""
import argparse
import hashlib
from pathlib import Path

import numpy as np
import onnx
from onnx import numpy_helper


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--packs", required=True, type=Path)
    args = ap.parse_args()
    model = onnx.load(str(args.packs / "inswapper_128.onnx"))
    emap = numpy_helper.to_array(model.graph.initializer[-1])
    assert emap.shape == (512, 512), emap.shape
    out = args.packs / "inswapper_128.emap.bin"
    emap.astype("<f4").tofile(out)
    digest = hashlib.sha256(out.read_bytes()).hexdigest()
    print(f"wrote {out} ({out.stat().st_size} bytes) sha256 {digest}")


if __name__ == "__main__":
    main()

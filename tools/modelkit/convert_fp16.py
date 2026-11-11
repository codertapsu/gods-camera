"""Write a half-precision copy of a model, keeping float32 inputs and outputs.

Half precision is what lets Core ML run a model on the Neural Engine. Always
compare the result against the original with check_swap.py before using it:
some models lose visible quality in half precision.

Usage: uv run python convert_fp16.py <in.onnx> <out.onnx>
"""
import sys

import onnx
from onnxconverter_common import float16


def main() -> None:
    src, dst = sys.argv[1], sys.argv[2]
    model = float16.convert_float_to_float16(onnx.load(src), keep_io_types=True)
    onnx.save(model, dst)
    print(f"wrote {dst}")


if __name__ == "__main__":
    main()

"""Spike S3: model speed through ONNX Runtime on this machine.

Times each model on random input with the CPU provider and with the Core ML
provider under several compute-unit settings, and reports how far each Core ML
output is from the CPU output.

Usage: uv run python bench_models.py --packs ../../../packs [--runs 60]
"""
import argparse
import json
import time
from pathlib import Path

import numpy as np
import onnxruntime as ort

MODELS = {
    "inswapper_128": ("inswapper_128.onnx", {"target": (1, 3, 128, 128), "source": (1, 512)}),
    "hyperswap_1b_256": ("hyperswap_1b_256.onnx", {"target": (1, 3, 256, 256), "source": (1, 512)}),
    "det_10g@640": ("buffalo_l/det_10g.onnx", {"input.1": (1, 3, 640, 640)}),
    "det_500m@320": ("buffalo_s/det_500m.onnx", {"input.1": (1, 3, 320, 320)}),
    "w600k_r50": ("buffalo_l/w600k_r50.onnx", {"input.1": (1, 3, 112, 112)}),
    "xseg_3": ("xseg_3.onnx", {None: (1, 256, 256, 3)}),
}


def coreml(units: str, fmt: str = "MLProgram"):
    return [("CoreMLExecutionProvider", {"ModelFormat": fmt, "MLComputeUnits": units}), "CPUExecutionProvider"]


CONFIGS = {
    "cpu": ["CPUExecutionProvider"],
    "coreml-all": coreml("ALL"),
    "coreml-gpu": coreml("CPUAndGPU"),
    "coreml-ane": coreml("CPUAndNeuralEngine"),
}


def feeds_for(sess, shapes, rng):
    feeds = {}
    for inp in sess.get_inputs():
        shape = None
        for key, s in shapes.items():
            if key is None or key == inp.name:
                shape = s
        if shape is None:  # match by rank when names differ
            shape = next(s for s in shapes.values() if len(s) == len(inp.shape))
        feeds[inp.name] = rng.standard_normal(shape).astype(np.float32) * 0.5
    return feeds


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--packs", required=True, type=Path)
    ap.add_argument("--runs", type=int, default=60)
    ap.add_argument("--out", type=Path)
    args = ap.parse_args()
    results = []
    for name, (rel, shapes) in MODELS.items():
        path = args.packs / rel
        ref = None
        for cfg, providers in CONFIGS.items():
            row = {"model": name, "config": cfg}
            try:
                so = ort.SessionOptions()
                so.log_severity_level = 3
                t0 = time.perf_counter()
                sess = ort.InferenceSession(str(path), sess_options=so, providers=providers)
                feeds = feeds_for(sess, shapes, np.random.default_rng(0))
                out = sess.run(None, feeds)
                row["load_s"] = round(time.perf_counter() - t0, 2)
                for _ in range(5):
                    sess.run(None, feeds)
                times = []
                for _ in range(args.runs):
                    t = time.perf_counter()
                    sess.run(None, feeds)
                    times.append((time.perf_counter() - t) * 1000)
                times.sort()
                row["median_ms"] = round(times[len(times) // 2], 2)
                row["p95_ms"] = round(times[int(len(times) * 0.95) - 1], 2)
                if cfg == "cpu":
                    ref = out
                    row["max_abs_diff_vs_cpu"] = 0.0
                elif ref is not None:
                    row["max_abs_diff_vs_cpu"] = round(
                        float(max(np.abs(a.astype(np.float64) - b).max() for a, b in zip(out, ref))), 4
                    )
            except Exception as exc:  # report and continue with the next configuration
                row["error"] = str(exc).splitlines()[0][:160]
            results.append(row)
            print(json.dumps(row), flush=True)
    if args.out:
        args.out.write_text(json.dumps(results, indent=1))


if __name__ == "__main__":
    main()

"""Spike S3: model speed through ONNX Runtime on this machine.

Times each model on random input with the CPU provider and with every
accelerated provider the installed ONNX Runtime offers, and reports how far
each accelerated output is from the CPU output.

- On a Mac the accelerated provider is Core ML, under three compute-unit settings.
- On Windows it is DirectML, and with the NVIDIA build of ONNX Runtime also
  CUDA, TensorRT and TensorRT for RTX.

Model variants made by rewrite_reflect_pad.py and convert_fp16.py are timed
too when `--variants` names the folder they are in. A variant is found by its
file name: `<model>.reflectfree.onnx`, `<model>.fp16.onnx`,
`<model>.reflectfree.fp16.onnx`.

Usage:
    uv run python bench_models.py --packs ../../../packs [--runs 60] [--variants DIR]

On Windows, pick the build of ONNX Runtime with uv:
    uv run --no-project --with onnxruntime-directml --with "numpy<3" python bench_models.py ...
    uv run --no-project --with "onnxruntime-gpu[cuda,cudnn]" --with "numpy<3" python bench_models.py ...
"""
import argparse
import json
import subprocess
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
VARIANTS = ["reflectfree", "fp16", "reflectfree.fp16"]


def coreml(units: str, fmt: str = "MLProgram"):
    return [("CoreMLExecutionProvider", {"ModelFormat": fmt, "MLComputeUnits": units}), "CPUExecutionProvider"]


def configs(cache: Path) -> dict:
    """Every way of running a model that this build of ONNX Runtime offers."""
    available = set(ort.get_available_providers())
    found = {"cpu": ["CPUExecutionProvider"]}
    if "CoreMLExecutionProvider" in available:
        found["coreml-all"] = coreml("ALL")
        found["coreml-gpu"] = coreml("CPUAndGPU")
        found["coreml-ane"] = coreml("CPUAndNeuralEngine")
    if "DmlExecutionProvider" in available:
        found["directml"] = ["DmlExecutionProvider", "CPUExecutionProvider"]
    if "CUDAExecutionProvider" in available:
        found["cuda"] = [("CUDAExecutionProvider", {"cudnn_conv_algo_search": "EXHAUSTIVE"}), "CPUExecutionProvider"]
    if "TensorrtExecutionProvider" in available:
        for name, half in (("tensorrt", False), ("tensorrt-fp16", True)):
            options = {
                "trt_fp16_enable": half,
                "trt_engine_cache_enable": True,
                "trt_engine_cache_path": str(cache / name),
            }
            found[name] = [("TensorrtExecutionProvider", options), "CUDAExecutionProvider", "CPUExecutionProvider"]
    if "NvTensorRTRTXExecutionProvider" in available:
        found["tensorrt-rtx"] = ["NvTensorRTRTXExecutionProvider", "CPUExecutionProvider"]
    return found


def video_memory_mb():
    """Video memory in use on the first NVIDIA card, or None without one."""
    try:
        text = subprocess.run(
            ["nvidia-smi", "--query-gpu=memory.used", "--format=csv,noheader,nounits"],
            capture_output=True, text=True, timeout=10,
        ).stdout
        return int(text.split()[0])
    except (OSError, ValueError, IndexError, subprocess.SubprocessError):
        return None


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
    ap.add_argument("--variants", type=Path, help="folder with reflect-free and half-precision copies")
    ap.add_argument("--runs", type=int, default=60)
    ap.add_argument("--only", nargs="*", help="model names to time; the default is all")
    ap.add_argument("--configs", nargs="*", help="configurations to time; the default is all")
    ap.add_argument("--cache", type=Path, default=Path("bench-cache"), help="where TensorRT keeps built engines")
    ap.add_argument("--out", type=Path)
    args = ap.parse_args()
    if hasattr(ort, "preload_dlls"):
        # The NVIDIA build finds CUDA and cuDNN in the pip packages beside it.
        ort.preload_dlls()
    found = configs(args.cache)
    print(json.dumps({"onnxruntime": ort.__version__, "configurations": list(found)}), flush=True)
    results = []
    for name, (rel, shapes) in MODELS.items():
        if args.only and name not in args.only:
            continue
        files = {"": args.packs / rel}
        if args.variants:
            for variant in VARIANTS:
                candidate = args.variants / f"{Path(rel).stem}.{variant}.onnx"
                if candidate.is_file():
                    files[variant] = candidate
        ref = None
        for variant, path in files.items():
            for cfg, providers in found.items():
                if args.configs and cfg not in args.configs:
                    continue
                row = {"model": name, "variant": variant or "as downloaded", "config": cfg}
                try:
                    so = ort.SessionOptions()
                    so.log_severity_level = 3
                    if cfg == "directml":
                        # DirectML takes a session only with these two switched off.
                        so.enable_mem_pattern = False
                        so.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
                    before = video_memory_mb()
                    t0 = time.perf_counter()
                    sess = ort.InferenceSession(str(path), sess_options=so, providers=providers)
                    # ONNX Runtime falls back to the processor without saying so.
                    row["ran_on"] = sess.get_providers()[0].replace("ExecutionProvider", "")
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
                    after = video_memory_mb()
                    if before is not None and after is not None and cfg != "cpu":
                        row["video_memory_mb"] = after - before
                    if cfg == "cpu" and not variant:
                        ref = out
                        row["max_abs_diff_vs_cpu"] = 0.0
                    elif ref is not None:
                        row["max_abs_diff_vs_cpu"] = round(
                            float(max(np.abs(a.astype(np.float64) - b).max() for a, b in zip(out, ref))), 4
                        )
                    del sess
                except Exception as exc:  # report and continue with the next configuration
                    row["error"] = str(exc).splitlines()[0][:160]
                results.append(row)
                print(json.dumps(row), flush=True)
    if args.out:
        args.out.write_text(json.dumps(results, indent=1))


if __name__ == "__main__":
    main()

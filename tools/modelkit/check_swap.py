"""Spike S3, part two: swap models on a real face crop.

For each model variant and provider this reports the time per run and how far
the output is from the full-precision CPU output, in 8-bit pixel levels. It
also writes the outputs as PNG files so they can be looked at.

Usage: uv run python check_swap.py --packs ../../../packs --golden ../../core/gc-infer/tests/golden --out <dir>

Copies of the models made by convert_fp16.py or mixed_precision.py are checked
too when `--variants` names the folder they are in; a file belongs to the
model its name starts with.

The providers are whatever the installed ONNX Runtime offers: Core ML on a
Mac; on Windows DirectML, or CUDA and TensorRT with the NVIDIA build:
    uv run --no-project --with onnxruntime-directml --with insightface --with onnx         --with opencv-python-headless --with scikit-image --with "numpy<3" python check_swap.py ...
"""
import argparse
import json
import time
from pathlib import Path

import cv2
import numpy as np
import onnx
import onnxruntime as ort
from insightface.model_zoo import get_model
from insightface.utils import face_align
from onnx import numpy_helper

CPU = ["CPUExecutionProvider"]


def coreml(units: str):
    return [("CoreMLExecutionProvider", {"ModelFormat": "MLProgram", "MLComputeUnits": units}), "CPUExecutionProvider"]


def providers() -> dict:
    """Every way of running a model that this build of ONNX Runtime offers."""
    available = set(ort.get_available_providers())
    found = {"cpu": CPU}
    if "CoreMLExecutionProvider" in available:
        found.update({"coreml-gpu": coreml("CPUAndGPU"), "coreml-all": coreml("ALL"), "coreml-ane": coreml("CPUAndNeuralEngine")})
    if "DmlExecutionProvider" in available:
        found["directml"] = ["DmlExecutionProvider", "CPUExecutionProvider"]
        # Without the card maker's own fast paths: slower, and a way to tell whose arithmetic is at fault.
        found["directml-plain"] = [("DmlExecutionProvider", {"disable_metacommands": "true"}), "CPUExecutionProvider"]
    if "CUDAExecutionProvider" in available:
        found["cuda"] = ["CUDAExecutionProvider", "CPUExecutionProvider"]
    if "TensorrtExecutionProvider" in available:
        found["tensorrt"] = [("TensorrtExecutionProvider", {"trt_fp16_enable": False}), "CUDAExecutionProvider", "CPUExecutionProvider"]
        found["tensorrt-fp16"] = [("TensorrtExecutionProvider", {"trt_fp16_enable": True}), "CUDAExecutionProvider", "CPUExecutionProvider"]
    if "NvTensorRTRTXExecutionProvider" in available:
        found["tensorrt-rtx"] = ["NvTensorRTRTXExecutionProvider", "CPUExecutionProvider"]
    return found


def timed(sess, feeds, runs: int):
    out = sess.run(None, feeds)
    for _ in range(5):
        sess.run(None, feeds)
    times = []
    for _ in range(runs):
        t = time.perf_counter()
        sess.run(None, feeds)
        times.append((time.perf_counter() - t) * 1000)
    times.sort()
    return out, times[len(times) // 2], times[int(len(times) * 0.95) - 1]


def to_u8(chw: np.ndarray, mean: float, std: float) -> np.ndarray:
    """Model output (RGB, CHW) to an 8-bit BGR image."""
    img = np.clip(chw.transpose(1, 2, 0) * std + mean, 0.0, 1.0) * 255.0
    return img[:, :, ::-1]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--packs", required=True, type=Path)
    ap.add_argument("--golden", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--runs", type=int, default=40)
    ap.add_argument("--only", default="", help="run only variants whose label contains this text")
    ap.add_argument("--variants", type=Path, help="folder with other copies of the models to check")
    ap.add_argument("--providers", nargs="*", help="providers to run; the default is all")
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    if hasattr(ort, "preload_dlls"):
        ort.preload_dlls()
    found = providers()

    img = cv2.imread(str(args.golden / "input.png"))
    golden = json.loads((args.golden / "golden.json").read_text())
    faces = golden["det"]["faces"]
    target_kps = np.array(faces[0]["kps"], dtype=np.float32)
    source_kps = np.array(faces[-1]["kps"], dtype=np.float32)

    rec = get_model(str(args.packs / "buffalo_l" / "w600k_r50.onnx"), providers=CPU)
    rec.prepare(0)
    emb = rec.get_feat(face_align.norm_crop(img, source_kps, 112)).ravel()
    normed = (emb / np.linalg.norm(emb)).astype(np.float32).reshape(1, -1)

    def crop_blob(size: int, mean: float, std: float):
        aimg, _ = face_align.norm_crop2(img, target_kps, size)
        rgb = aimg[:, :, ::-1].astype(np.float32) / 255.0
        return aimg, ((rgb - mean) / std).transpose(2, 0, 1)[None].astype(np.float32)

    # inswapper: latent is the unit embedding times emap, normalised again.
    emap = numpy_helper.to_array(onnx.load(str(args.packs / "inswapper_128.onnx")).graph.initializer[-1])
    latent = normed @ emap
    latent = (latent / np.linalg.norm(latent)).astype(np.float32)
    aimg128, blob128 = crop_blob(128, 0.0, 1.0)
    aimg256, blob256 = crop_blob(256, 0.5, 0.5)
    cv2.imwrite(str(args.out / "target_256.png"), aimg256)

    variants = [
        # (label, file, feeds, mean, std)
        ("inswapper_128", "inswapper_128.onnx", {"target": blob128, "source": latent}, 0.0, 1.0),
        # Reflect padding rewritten first, then half precision: the Neural Engine variant.
        ("inswapper_128 reflect-free fp16", "inswapper_128.reflectfree.fp16.onnx", {"target": blob128, "source": latent}, 0.0, 1.0),
        # HyperSwap already stores half-precision weights, so it has no separate fp16 variant.
        ("hyperswap_1b_256", "hyperswap_1b_256.onnx", {"target": blob256, "source": normed}, 0.5, 0.5),
    ]
    if args.variants:
        feeds_of = {"inswapper_128": ({"target": blob128, "source": latent}, 0.0, 1.0), "hyperswap_1b_256": ({"target": blob256, "source": normed}, 0.5, 0.5)}
        for extra in sorted(args.variants.glob("*.onnx")):
            for family, (feeds, mean, std) in feeds_of.items():
                if extra.name.startswith(family + "."):
                    label = family + " " + extra.name[len(family) + 1 : -len(".onnx")]
                    if label not in [v[0] for v in variants]:
                        variants.append((label, extra, feeds, mean, std))
    reference = {}
    rows = []
    if args.only:
        for label, file, feeds, mean, std in variants:
            family = label.split()[0]
            if family not in reference and args.only.split()[0] == family:
                sess = ort.InferenceSession(str(args.packs / file), providers=CPU)
                reference[family] = to_u8(sess.run(None, feeds)[0][0], mean, std)
    for label, file, feeds, mean, std in variants:
        path = file if isinstance(file, Path) else args.packs / file
        if not path.exists() or args.only not in label:
            continue
        family = label.split()[0]
        for cfg, chosen in found.items():
            if args.providers and cfg not in args.providers:
                continue
            row = {"model": label, "provider": cfg}
            try:
                so = ort.SessionOptions()
                so.log_severity_level = 3
                if cfg.startswith("directml"):
                    # DirectML takes a session only with these two switched off.
                    so.enable_mem_pattern = False
                    so.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
                sess = ort.InferenceSession(str(path), sess_options=so, providers=chosen)
                row["ran_on"] = sess.get_providers()[0].replace("ExecutionProvider", "")
                out, median, p95 = timed(sess, feeds, args.runs)
                image = to_u8(out[0][0], mean, std)
                if family not in reference:  # the first run of a family is fp32 on the CPU
                    reference[family] = image
                diff = np.abs(image - reference[family])
                row.update(
                    median_ms=round(median, 1), p95_ms=round(p95, 1),
                    max_level_diff=round(float(diff.max()), 2), mean_level_diff=round(float(diff.mean()), 3),
                    finite=bool(np.isfinite(out[0]).all()),
                )
                name = f"{label.replace(' ', '_')}__{cfg}.png"
                cv2.imwrite(str(args.out / name), image.astype(np.uint8))
            except Exception as exc:
                row["error"] = str(exc).splitlines()[0][:140]
            rows.append(row)
            print(json.dumps(row), flush=True)
    (args.out / "results.json").write_text(json.dumps(rows, indent=1))


if __name__ == "__main__":
    main()

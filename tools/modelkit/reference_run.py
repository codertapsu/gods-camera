"""Reference run with InsightFace's own Python code.

Writes golden files that the Rust core's parity tests compare against:
detector raw outputs and decoded faces, the aligned crop, the swap model's
output and the pasted-back frame.

Usage: uv run python reference_run.py --packs ../../../packs --out <dir>
"""
import argparse
import json
from pathlib import Path

import cv2
import numpy as np
import onnxruntime as ort
from insightface.data import get_image
from insightface.model_zoo import get_model
from insightface.utils import face_align

DET_SIZE = 640


def letterbox(img: np.ndarray, size: int):
    """InsightFace's detector input: keep aspect, pad bottom/right with zeros."""
    h, w = img.shape[:2]
    scale = min(size / h, size / w)
    nh, nw = int(round(h * scale)), int(round(w * scale))
    # Match scrfd.py exactly: it derives one side from the aspect ratio.
    im_ratio, model_ratio = h / w, 1.0
    if im_ratio > model_ratio:
        nh, nw = size, int(size / im_ratio)
    else:
        nw, nh = size, int(size * im_ratio)
    det_scale = nh / h
    canvas = np.zeros((size, size, 3), dtype=np.uint8)
    canvas[:nh, :nw] = cv2.resize(img, (nw, nh))
    return canvas, det_scale


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--packs", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    cpu = ["CPUExecutionProvider"]

    img = get_image("t1")  # sample group photo shipped with the package (BGR)
    cv2.imwrite(str(args.out / "input.png"), img)

    # --- Detector: raw outputs on our own letterbox, and InsightFace's decode.
    det_path = args.packs / "buffalo_l" / "det_10g.onnx"
    canvas, det_scale = letterbox(img, DET_SIZE)
    blob = cv2.dnn.blobFromImage(canvas, 1.0 / 128.0, (DET_SIZE, DET_SIZE), (127.5, 127.5, 127.5), swapRB=True)
    sess = ort.InferenceSession(str(det_path), providers=cpu)
    raw = sess.run(None, {sess.get_inputs()[0].name: blob})
    raw_flat = np.concatenate([o.astype("<f4").ravel() for o in raw])
    raw_flat.tofile(args.out / "det_raw.bin")

    det = get_model(str(det_path), providers=cpu)
    det.prepare(0, input_size=(DET_SIZE, DET_SIZE), det_thresh=0.5)
    bboxes, kpss = det.detect(img, max_num=0, metric="default")
    order = np.argsort(bboxes[:, 0])  # left to right, for a stable order
    bboxes, kpss = bboxes[order], kpss[order]

    # --- Identity, alignment and swap on the leftmost face, using the rightmost as source.
    rec = get_model(str(args.packs / "buffalo_l" / "w600k_r50.onnx"), providers=cpu)
    rec.prepare(0)
    swapper = get_model(str(args.packs / "inswapper_128.onnx"), providers=cpu)

    class Face(dict):
        __getattr__ = dict.get

    def make_face(i):
        f = Face(bbox=bboxes[i, :4], kps=kpss[i], det_score=float(bboxes[i, 4]))
        emb = rec.get(img, f)
        f["embedding"] = emb
        f["normed_embedding"] = emb / np.linalg.norm(emb)
        return f

    target, source = make_face(0), make_face(len(bboxes) - 1)
    aimg, M = face_align.norm_crop2(img, target.kps, 128)
    cv2.imwrite(str(args.out / "aligned_128.png"), aimg)
    fake, M2 = swapper.get(img, target, source, paste_back=False)
    cv2.imwrite(str(args.out / "swap_128.png"), fake)
    pasted = swapper.get(img.copy(), target, source, paste_back=True)
    cv2.imwrite(str(args.out / "pasted.png"), pasted)

    latent = source.normed_embedding.reshape(1, -1) @ swapper.emap
    latent /= np.linalg.norm(latent)

    meta = {
        "image": {"height": int(img.shape[0]), "width": int(img.shape[1])},
        "det": {
            "input_size": DET_SIZE,
            "det_scale": float(det_scale),
            "output_shapes": [list(o.shape) for o in raw],
            "output_names": [o.name for o in sess.get_outputs()],
            "faces": [
                {"bbox": bboxes[i, :4].tolist(), "score": float(bboxes[i, 4]), "kps": kpss[i].tolist()}
                for i in range(len(bboxes))
            ],
        },
        "align": {"target_kps": target.kps.tolist(), "matrix_128": M.tolist()},
        "swap": {
            "emap_shape": list(swapper.emap.shape),
            "latent_first8": latent.ravel()[:8].tolist(),
            "source_embedding_norm": float(np.linalg.norm(source.embedding)),
        },
        "versions": {"onnxruntime": ort.__version__},
    }
    (args.out / "golden.json").write_text(json.dumps(meta, indent=1))
    print(f"faces: {len(bboxes)}  det_scale: {det_scale:.4f}  raw outputs: {[o.shape for o in raw]}")
    print(f"wrote golden files to {args.out}")


if __name__ == "__main__":
    main()

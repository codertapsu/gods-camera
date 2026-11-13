"""Read the text in pictures of a page, and print it as the Mac's `gc-ocr` does.

    uv run python -I read_pages.py --store ../../../packs <picture>... > pages.json

Two models work in turn: one finds the lines of text, whatever the language,
and one reads each line. This is where the models were chosen and the
settings measured. The reader the Windows app uses, core/gc-ocr, does the same
steps in Rust; the two agree to within a character or two a page.

Measure the result against a test page from tools/modelkit/make_document.py:

    gc merge-ocr pages.json --truth truth.txt

The model files are downloads: keep each in a folder of its own and run this
with -I, so that nothing beside them is taken for Python code.
"""
import argparse
import io
import json
import sys
import time
import unicodedata
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageOps


def session_options(threads: int):
    """How both models are run: on a few threads, and with the library's plain
    kernels on every processor. On Arm processors with the SME instructions,
    Apple's M4 among them, ONNX Runtime would otherwise compute convolutions
    with other kernels (KleidiAI), and the same picture could read differently."""
    import onnxruntime as ort

    options = ort.SessionOptions()
    options.intra_op_num_threads = threads
    options.add_session_config_entry("mlas.disable_kleidiai", "1")
    return options


class Detector:
    """Finds lines of text. A PP-OCR detection model: one number per pixel, how
    likely it is to be inside a line. Each island of likely pixels is a line."""

    MEAN = np.array([0.485, 0.456, 0.406], dtype=np.float32)
    DEVIATION = np.array([0.229, 0.224, 0.225], dtype=np.float32)

    def __init__(self, file: str, longest: int, inside: float, sure: float, grow: float, threads: int):
        import onnxruntime as ort

        self.session = ort.InferenceSession(file, session_options(threads), providers=["CPUExecutionProvider"])
        self.input = self.session.get_inputs()[0].name
        self.longest, self.inside, self.sure, self.grow = longest, inside, sure, grow

    def lines(self, picture: np.ndarray) -> list[tuple[np.ndarray, float]]:
        """The corners of every line found in a picture (height, width, 3 as
        blue-green-red), clockwise from the top-left, and how sure the model is."""
        height, width = picture.shape[:2]
        scale = min(1.0, self.longest / max(height, width))
        # The model halves the picture five times: both sides must divide by 32.
        small_w = max(32, round(width * scale / 32) * 32)
        small_h = max(32, round(height * scale / 32) * 32)
        small = cv2.resize(picture, (small_w, small_h), interpolation=cv2.INTER_AREA if scale < 1 else cv2.INTER_LINEAR)
        # PaddleOCR applies these numbers to blue-green-red as they stand.
        pixels = (small.astype(np.float32) / 255.0 - self.MEAN) / self.DEVIATION
        likely = self.session.run(None, {self.input: pixels.transpose(2, 0, 1)[None]})[0][0, 0]

        found = []
        islands, _ = cv2.findContours((likely > self.inside).astype(np.uint8), cv2.RETR_LIST, cv2.CHAIN_APPROX_SIMPLE)
        for island in islands[:3000]:
            (cx, cy), (w, h), angle = cv2.minAreaRect(island)
            if min(w, h) < 3:
                continue
            corners = cv2.boxPoints(((cx, cy), (w, h), angle))
            score = self.score(likely, corners)
            if score < self.sure:
                continue
            # The model marks the middle of a line; the whole line is this much wider all round.
            margin = (w * h) * self.grow / (2 * (w + h))
            w, h = w + 2 * margin, h + 2 * margin
            if min(w, h) < 5:
                continue
            corners = cv2.boxPoints(((cx, cy), (w, h), angle))
            # A line at the picture's edge can reach past it.
            corners[:, 0] *= width / small_w
            corners[:, 1] *= height / small_h
            found.append((self.clockwise(corners), float(score)))
        found.sort(key=lambda line: (round(float(line[0][:, 1].mean()) / 10), float(line[0][:, 0].min())))
        return found

    @staticmethod
    def score(likely: np.ndarray, corners: np.ndarray) -> float:
        """The mean likelihood inside a box."""
        h, w = likely.shape
        x0 = int(np.clip(np.floor(corners[:, 0].min()), 0, w - 1))
        x1 = int(np.clip(np.ceil(corners[:, 0].max()), 0, w - 1))
        y0 = int(np.clip(np.floor(corners[:, 1].min()), 0, h - 1))
        y1 = int(np.clip(np.ceil(corners[:, 1].max()), 0, h - 1))
        mask = np.zeros((y1 - y0 + 1, x1 - x0 + 1), dtype=np.uint8)
        cv2.fillPoly(mask, [(corners - [x0, y0]).astype(np.int32)], 1)
        return float(cv2.mean(likely[y0 : y1 + 1, x0 : x1 + 1], mask)[0])

    @staticmethod
    def clockwise(corners: np.ndarray) -> np.ndarray:
        """Top-left, top-right, bottom-right, bottom-left, for a line that is roughly level."""
        by_x = corners[np.argsort(corners[:, 0])]
        left = by_x[:2][np.argsort(by_x[:2, 1])]
        right = by_x[2:][np.argsort(by_x[2:, 1])]
        return np.array([left[0], right[0], right[1], left[1]], dtype=np.float32)


def straightened(picture: np.ndarray, corners: np.ndarray) -> np.ndarray:
    """The line cut out of the picture and made level."""
    width = int(round(max(np.linalg.norm(corners[0] - corners[1]), np.linalg.norm(corners[3] - corners[2]))))
    height = int(round(max(np.linalg.norm(corners[0] - corners[3]), np.linalg.norm(corners[1] - corners[2]))))
    width, height = max(width, 1), max(height, 1)
    level = np.array([[0, 0], [width, 0], [width, height], [0, height]], dtype=np.float32)
    move = cv2.getPerspectiveTransform(corners, level)
    return cv2.warpPerspective(picture, move, (width, height), flags=cv2.INTER_CUBIC, borderMode=cv2.BORDER_REPLICATE)


class AsterNet:
    """Reads one line: 32 pixels high, as wide as it comes, one guess for every
    four pixels of width (CTC), and no more than 256 guesses."""

    HEIGHT = 32
    WIDEST = 1024

    def __init__(self, model: str, characters: str, threads: int, stretch: float, shrink: str):
        import onnxruntime as ort

        self.chars = json.load(io.open(characters, encoding="utf-8"))["chars"]
        self.session = ort.InferenceSession(model, session_options(threads), providers=["CPUExecutionProvider"])
        self.input = self.session.get_inputs()[0].name
        self.stretch = stretch
        self.shrink = {"area": cv2.INTER_AREA, "cubic": cv2.INTER_CUBIC, "linear": cv2.INTER_LINEAR}[shrink]

    def read(self, line: np.ndarray) -> tuple[str, float]:
        height, width = line.shape[:2]
        wide = max(self.HEIGHT, round(width * self.HEIGHT / height * self.stretch))
        small = cv2.resize(line, (wide, self.HEIGHT), interpolation=self.shrink if height > self.HEIGHT else cv2.INTER_CUBIC)
        texts, sure = [], []
        for piece in self.pieces(small):
            text, each = self.read_piece(piece)
            if text:
                texts.append(text)
                sure.extend(each)
        return " ".join(texts), (sum(sure) / len(sure) if sure else 0.0)

    def pieces(self, small: np.ndarray) -> list[np.ndarray]:
        """A line too wide for the model, cut between words into pieces that fit."""
        wide = small.shape[1]
        if wide <= self.WIDEST:
            return [small]
        # Few enough pixels to a piece that a cut moved to a gap, by up to a
        # fifth of a piece at either end, still leaves every piece within the limit.
        count = -(-wide * 7 // (self.WIDEST * 5))
        grey = cv2.cvtColor(small, cv2.COLOR_BGR2GRAY)
        _, ink = cv2.threshold(grey, 0, 1, cv2.THRESH_BINARY_INV + cv2.THRESH_OTSU)
        empty = ink.sum(axis=0) == 0
        cuts = [0]
        for number in range(1, count):
            ideal = wide * number // count
            reach = wide // count // 5
            best, best_run = ideal, 0
            column = max(cuts[-1] + 1, ideal - reach)
            while column < min(wide, ideal + reach):
                if empty[column]:
                    run = column
                    while run < wide and empty[run]:
                        run += 1
                    if run - column > best_run:
                        best, best_run = (column + run) // 2, run - column
                    column = run
                else:
                    column += 1
            cuts.append(best)
        cuts.append(wide)
        return [small[:, a:b] for a, b in zip(cuts, cuts[1:]) if b - a >= 8]

    def read_piece(self, small: np.ndarray) -> tuple[str, list[float]]:
        pixels = cv2.cvtColor(small, cv2.COLOR_BGR2RGB).astype(np.float32) / 255.0
        scores = self.session.run(None, {self.input: pixels.transpose(2, 0, 1)[None]})[0][:, 0, :]
        best = scores.argmax(-1)
        text, sure, last = [], [], 0
        for step, index in enumerate(best):
            if index != last and index != 0:
                text.append(self.chars[index])
                sure.append(float(np.exp(scores[step, index])))
            last = index
        return "".join(text).strip(), sure


class Tesseract:
    def __init__(self, data: str, languages: str):
        import tesserocr

        self.api = tesserocr.PyTessBaseAPI(path=data, lang=languages, psm=tesserocr.PSM.SINGLE_LINE)

    def read(self, line: np.ndarray) -> tuple[str, float]:
        self.api.SetImage(Image.fromarray(cv2.cvtColor(line, cv2.COLOR_BGR2RGB)))
        text = unicodedata.normalize("NFC", self.api.GetUTF8Text().strip())
        return text, max(self.api.MeanTextConf(), 0) / 100.0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("pictures", nargs="+")
    parser.add_argument("--store", help="the model store, ../packs beside the repository: the files the app uses")
    parser.add_argument("--detector", help="another PP-OCR detection model as ONNX, to try it")
    parser.add_argument("--asternet", help="a folder with asternet.onnx and vocab.json as they are published")
    parser.add_argument("--tessdata", help="folder with Tesseract's language files, to read with Tesseract instead")
    parser.add_argument("--languages", default="vie")
    parser.add_argument("--longest", type=int, default=1920, help="the detector sees the picture no larger than this")
    parser.add_argument("--inside", type=float, default=0.2, help="a pixel this likely counts as text")
    parser.add_argument("--sure", type=float, default=0.45, help="a line must be this likely on average")
    parser.add_argument("--grow", type=float, default=1.4, help="how far a found line is widened")
    parser.add_argument("--least-sure", type=float, default=0.5, help="a line read with less confidence than this is dropped")
    parser.add_argument("--stretch", type=float, default=1.0, help="widen each line by this much before reading it")
    parser.add_argument("--shrink", choices=["area", "cubic", "linear"], default="area", help="how a line is made 32 pixels high")
    parser.add_argument("--threads", type=int, default=4)
    parser.add_argument("--save-lines", type=Path, help="folder to keep every cut-out line in, for looking at")
    args = parser.parse_args()
    detector_file = args.detector or (args.store and args.store + "/PP-OCRv6_small_det.onnx")
    if not detector_file or not (args.store or args.asternet or args.tessdata):
        parser.error("say where the model files are: --store, or --detector with --asternet or --tessdata")

    detector = Detector(detector_file, args.longest, args.inside, args.sure, args.grow, args.threads)
    if args.tessdata:
        reader = Tesseract(args.tessdata, args.languages)
    elif args.asternet:
        reader = AsterNet(args.asternet + "/asternet.onnx", args.asternet + "/vocab.json", args.threads, args.stretch, args.shrink)
    else:
        reader = AsterNet(args.store + "/asternet_vi.onnx", args.store + "/asternet_vi.vocab.json", args.threads, args.stretch, args.shrink)
    pages = []
    finding = reading = 0.0
    for number, path in enumerate(args.pictures):
        page = {"path": path, "width": 0, "height": 0, "lines": []}
        try:
            # A phone stores a photo as the sensor saw it, with a note on how to turn it.
            picture = cv2.cvtColor(np.asarray(ImageOps.exif_transpose(Image.open(path)).convert("RGB")), cv2.COLOR_RGB2BGR)
        except Exception:
            page["error"] = "not a readable picture"
            pages.append(page)
            continue
        page["height"], page["width"] = picture.shape[:2]
        started = time.perf_counter()
        found = detector.lines(picture)
        finding += time.perf_counter() - started
        started = time.perf_counter()
        # What is reported lies inside the picture; the line is cut out where it was found.
        inside = [np.clip(corners, 0, [page["width"] - 1, page["height"] - 1]) for corners, _ in found]
        heights = sorted(float(np.linalg.norm(quad[0] - quad[3])) for quad in inside)
        usual = heights[len(heights) // 2] if heights else 0.0
        for index, ((corners, _), quad) in enumerate(zip(found, inside)):
            line = straightened(picture, corners)
            if line.shape[0] > 1.5 * line.shape[1]:
                continue  # Text running down the page is not read.
            if args.save_lines:
                args.save_lines.mkdir(parents=True, exist_ok=True)
                cv2.imwrite(str(args.save_lines / f"{number:02d}-{index:03d}.png"), line)
            # The sliver of a line that the picture's top or bottom edge left over reads as nonsense.
            tall = float(np.linalg.norm(quad[0] - quad[3]))
            at_the_edge = quad[:, 1].min() < 0.012 * page["height"] or quad[:, 1].max() > 0.988 * page["height"]
            if at_the_edge and tall < 0.7 * usual:
                continue
            text, confidence = reader.read(line)
            # So does a shadow or a stain taken for text.
            if not text or confidence < args.least_sure:
                continue
            page["lines"].append({
                "text": text,
                "confidence": round(confidence, 4),
                "quad": [round(float(value), 1) for value in quad.reshape(-1)],
                "title": bool(tall > 1.25 * usual),
            })
        reading += time.perf_counter() - started
        pages.append(page)
    sys.stdout.buffer.write(json.dumps(pages, ensure_ascii=False).encode("utf-8") + b"\n")
    lines = sum(len(page["lines"]) for page in pages)
    print(f"{len(pages)} pictures, {lines} lines; finding {1000 * finding / len(pages):.0f} ms a picture, reading {1000 * reading / max(lines, 1):.0f} ms a line", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())

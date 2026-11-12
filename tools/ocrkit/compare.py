"""Compare text readers on drawn lines of Vietnamese: which keeps the tone marks?

    uv run python -I compare.py --store ../../../packs [--tessdata <folder>] [--fonts times,arial] [--size 46]

Each line of the shared test text is drawn in every font, once clean and once
as a phone would photograph it (a little turned, blurred, unevenly lit, saved
as JPEG), and read by every reader that was given its files. Prints the share
of characters each got right.

--store is the model store with the reader the app uses. --tessdata is a
folder with Tesseract's vie.traineddata (from tessdata_best), for comparison;
it needs `uv sync --extra tesseract`.

The model files are downloads: keep them out of this folder and run this
with -I, so that nothing beside them is taken for Python code.
"""
import argparse
import io
import json
import random
import sys
import time
import unicodedata
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import numpy as np
from PIL import Image, ImageFilter

from sample import TEXT, distance, draw, font, wrap

PAPER = (250, 248, 242)


def photographed(picture: Image.Image, rng: random.Random) -> Image.Image:
    """The line as a phone's camera would hand it over."""
    wide = Image.new("RGB", (picture.width + 40, picture.height + 24), "white")
    wide.paste(picture, (20, 12))
    paper = Image.new("RGB", wide.size, PAPER)
    wide = Image.composite(wide, paper, wide.convert("L").point(lambda v: 255 if v < 250 else 0))
    wide = wide.rotate(rng.uniform(-0.7, 0.7), resample=Image.BICUBIC, fillcolor=PAPER)
    shade = Image.linear_gradient("L").rotate(rng.choice([0, 90, 180, 270])).resize(wide.size)
    dark = Image.new("RGB", wide.size, (150, 145, 135))
    wide = Image.composite(wide, dark, shade.point(lambda v: 150 + v * 105 // 255))
    wide = wide.filter(ImageFilter.GaussianBlur(rng.uniform(0.6, 1.1)))
    stored = io.BytesIO()
    wide.save(stored, "JPEG", quality=85)
    return Image.open(io.BytesIO(stored.getvalue())).convert("RGB")


class Tesseract:
    def __init__(self, data: str, languages: str):
        import tesserocr

        self.name = f"Tesseract {languages}"
        self.api = tesserocr.PyTessBaseAPI(path=data, lang=languages, psm=tesserocr.PSM.SINGLE_LINE)

    def read(self, picture: Image.Image) -> str:
        self.api.SetImage(picture)
        return unicodedata.normalize("NFC", self.api.GetUTF8Text().strip())


class AsterNet:
    """A recogniser with one output per slice of the line (CTC), 32 pixels high."""

    def __init__(self, store: str):
        import onnxruntime as ort

        self.name = "AsterNet"
        self.chars = json.load(io.open(store + "/asternet_vi.vocab.json", encoding="utf-8"))["chars"]
        self.session = ort.InferenceSession(store + "/asternet_vi.onnx", providers=["CPUExecutionProvider"])
        self.input = self.session.get_inputs()[0].name

    def read(self, picture: Image.Image) -> str:
        width = max(32, round(picture.width * 32 / picture.height))
        pixels = np.asarray(picture.convert("RGB").resize((width, 32), Image.BICUBIC), dtype=np.float32) / 255.0
        scores = self.session.run(None, {self.input: pixels.transpose(2, 0, 1)[None]})[0]
        text, last = [], 0
        for index in np.squeeze(scores, 1).argmax(-1):
            if index != last and index != 0:
                text.append(self.chars[index])
            last = index
        return "".join(text).strip()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--store", help="the model store, ../packs beside the repository")
    parser.add_argument("--tessdata", help="folder with vie.traineddata")
    parser.add_argument("--languages", default="vie")
    parser.add_argument("--fonts", default="times,timesbd,timesi,arial,tahoma,verdana,calibri,cambria,pala,cour")
    parser.add_argument("--size", type=int, default=46, help="the letters' size in pixels; the scanner test page uses 46")
    parser.add_argument("--column", type=int, default=520, help="the column's width in pixels at size 30")
    parser.add_argument("--show", action="store_true", help="print every line a reader got wrong")
    args = parser.parse_args()

    readers = []
    if args.tessdata:
        readers.append(Tesseract(args.tessdata, args.languages))
    if args.store:
        readers.append(AsterNet(args.store))
    if not readers:
        parser.error("give at least one reader its files")

    totals = {(reader.name, kind): [0, 0, 0.0, 0] for reader in readers for kind in ("clean", "photo")}
    print(f"{'font':10}" + "".join(f"{reader.name + ', ' + kind:>24}" for reader in readers for kind in ("clean", "photo")))
    for name in args.fonts.split(","):
        face = font(args.size, name)
        lines = wrap(TEXT, face, args.column * args.size // 30)
        rng = random.Random(7)
        pictures = {"clean": [draw(text, face, args.size) for text in lines]}
        pictures["photo"] = [photographed(picture, rng) for picture in pictures["clean"]]
        row = f"{name:10}"
        for reader in readers:
            for kind in ("clean", "photo"):
                wrong = 0
                started = time.perf_counter()
                for text, picture in zip(lines, pictures[kind]):
                    read = reader.read(picture)
                    mistakes = distance(read, text)
                    wrong += mistakes
                    if mistakes and args.show:
                        sys.stdout.buffer.write(f"  {reader.name}, {name}, {kind}\n    wrote: {text}\n    read:  {read}\n".encode("utf-8"))
                total = sum(len(text) for text in lines)
                sums = totals[(reader.name, kind)]
                sums[0] += wrong
                sums[1] += total
                sums[2] += time.perf_counter() - started
                sums[3] += len(lines)
                row += f"{100 - 100 * wrong / total:>22.2f} %"
        print(row)
    print(f"{'all':10}" + "".join(f"{100 - 100 * totals[(reader.name, kind)][0] / totals[(reader.name, kind)][1]:>22.2f} %" for reader in readers for kind in ("clean", "photo")))
    print(f"{'ms a line':10}" + "".join(f"{1000 * totals[(reader.name, kind)][2] / totals[(reader.name, kind)][3]:>24.0f}" for reader in readers for kind in ("clean", "photo")))
    return 0


if __name__ == "__main__":
    sys.exit(main())

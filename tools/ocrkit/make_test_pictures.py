"""Draw the three small pictures that core/gc-ocr's tests read.

    uv run python make_test_pictures.py ../../core/gc-ocr/tests/data

page.png is a heading and five lines; long-line.png is one line too wide for
the recogniser to take in one go; speckled.png is the heading and three of the
lines as a tablet's document scanner gives a page in poor light, sprinkled
with specks. Their text is repeated in the test, core/gc-ocr/tests/reader.rs:
change both together.
"""
import sys
from pathlib import Path

import cv2
import numpy as np
from PIL import Image, ImageDraw

from sample import font

PAPER = 246
INK = 28

TITLE = "Chợ nổi buổi sớm"
LINES = [
    "Trời vừa hửng sáng, ghe xuồng đã tấp nập trên sông.",
    "Người bán treo hàng lên cây bẹo: khóm, bưởi, khoai lang.",
    "Giá hôm nay: 25.000 đồng một ký, rẻ hơn tuần trước.",
    "The market opens at 5 a.m. and closes before noon.",
    "Ở đây, mỗi chuyến đò đều chở đầy ắp tiếng cười.",
]
LONG = (
    "Mỗi sáng, bà Tư chèo chiếc xuồng nhỏ chở đầy rau muống, bông súng và cá lóc ra chợ nổi, "
    "bán hết hàng trước khi mặt trời lên cao rồi mới thong thả quay về nhà."
)


def speckled(page: Image.Image, seed: int, share: float = 0.10, depth: float = 0.7, soften: float = 0.5) -> Image.Image:
    """The page with thin grey dashes all over it, one pixel wide and a few
    long, lying across and down: what a scanner's black-and-white filter makes
    of the grain of a photo taken in poor light. The writing itself stays as
    it was. `share` of the page is under a dash before the dashes are softened
    by `soften` pixels; `depth` is how dark a dash is.

    The dashes are heavier than on the tablet's real page, so that the test
    can tell a cleaned reading from an uncleaned one beyond doubt: as it is,
    the picture reads with 19 characters wrong of 176; cleaned, with 7."""
    rng = np.random.default_rng(seed)
    clean = cv2.GaussianBlur(np.asarray(page, dtype=np.float32) / 255.0, (0, 0), 0.6)
    across = cv2.GaussianBlur(rng.standard_normal(clean.shape).astype(np.float32), (0, 0), sigmaX=1.3, sigmaY=0.35)
    down = cv2.GaussianBlur(rng.standard_normal(clean.shape).astype(np.float32), (0, 0), sigmaX=0.35, sigmaY=1.3)
    grain = np.maximum(across / across.std(), down / down.std())
    dashes = (grain > np.quantile(grain, 1 - share)).astype(np.float32)
    dashes = np.clip(cv2.GaussianBlur(dashes, (0, 0), soften) * 1.5, 0, 1)
    # The filter also makes the paper white and the ink black.
    scan = np.clip((clean - 0.2) / 0.6, 0, 1) * (1 - depth * dashes)
    return Image.fromarray(np.rint(scan * 255).astype(np.uint8), "L")


def main() -> int:
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    heading, body = font(64, "timesbd"), font(36)

    page = Image.new("L", (1000, 470), PAPER)
    draw = ImageDraw.Draw(page)
    draw.text((60, 40), TITLE, font=heading, fill=INK)
    for row, line in enumerate(LINES):
        draw.text((60, 150 + row * 56), line, font=body, fill=INK)
    page.save(out / "page.png", optimize=True)

    long_line = Image.new("L", (int(body.getlength(LONG)) + 120, 110), PAPER)
    ImageDraw.Draw(long_line).text((60, 32), LONG, font=body, fill=INK)
    long_line.save(out / "long-line.png", optimize=True)
    # Larger writing, as a page has it when it fills a tablet's photo.
    heading, body = font(84, "timesbd"), font(48)
    scan = Image.new("L", (int(max(body.getlength(line) for line in LINES[:3])) + 160, 500), PAPER)
    draw = ImageDraw.Draw(scan)
    draw.text((80, 50), TITLE, font=heading, fill=INK)
    for row, line in enumerate(LINES[:3]):
        draw.text((80, 200 + row * 76), line, font=body, fill=INK)
    speckled(scan, seed=6).save(out / "speckled.png", optimize=True)

    for name in ("page.png", "long-line.png", "speckled.png"):
        picture = Image.open(out / name)
        print(name, picture.size, (out / name).stat().st_size, "bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())

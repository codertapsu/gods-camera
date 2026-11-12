"""Draw the two small pictures that core/gc-ocr's tests read.

    uv run python make_test_pictures.py ../../core/gc-ocr/tests/data

page.png is a heading and five lines; long-line.png is one line too wide for
the recogniser to take in one go. Their text is repeated in the test,
core/gc-ocr/tests/reader.rs: change both together.
"""
import sys
from pathlib import Path

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
    for name in ("page.png", "long-line.png"):
        picture = Image.open(out / name)
        print(name, picture.size, (out / name).stat().st_size, "bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())

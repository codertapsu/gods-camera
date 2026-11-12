#!/usr/bin/env python3
"""Draw the Windows app's icon: apps/windows/app/Assets/app.ico.

The same picture as the Android app's launcher icon
(apps/android/app/src/main/res/drawable/ic_launcher_foreground.xml): a white
camera seen from the front, with an indigo lens and a small light, on an
indigo rounded square. Drawn here from its shapes at every size Windows asks
for, so nothing is scaled down from a large picture and goes soft.

    python tools/make-windows-icon.py

Needs nothing beyond Python. The icon is kept in git; run this again only
when the drawing changes.
"""
from __future__ import annotations

import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parents[1] / "apps" / "windows" / "app" / "Assets" / "app.ico"
SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
INDIGO = (0x3F, 0x51, 0xB5)
WHITE = (255, 255, 255)
# Each pixel is sampled this many times across and down, for smooth edges.
SAMPLES = 5


def in_rounded_box(x: float, y: float, left: float, top: float, right: float, bottom: float, radius: float) -> bool:
    if not (left <= x <= right and top <= y <= bottom):
        return False
    cx = min(max(x, left + radius), right - radius)
    cy = min(max(y, top + radius), bottom - radius)
    return (x - cx) ** 2 + (y - cy) ** 2 <= radius ** 2


def in_circle(x: float, y: float, cx: float, cy: float, radius: float) -> bool:
    return (x - cx) ** 2 + (y - cy) ** 2 <= radius ** 2


def colour_at(x: float, y: float) -> tuple[int, int, int] | None:
    """The colour at a point of the drawing, which is 100 units square. None outside the icon."""
    if not in_rounded_box(x, y, 2, 2, 98, 98, 22):
        return None
    # The Android drawing's camera sits in the middle 66 of its 108 units.
    # Here it is given the middle 70 of 100.
    u = 54 + (x - 50) * 66 / 70
    v = 54 + (y - 50) * 66 / 70
    if in_circle(u, v, 72, 47, 2):
        return INDIGO  # the small light
    if in_circle(u, v, 54, 57, 6):
        return WHITE  # the glass
    if in_circle(u, v, 54, 57, 11):
        return INDIGO  # the lens ring
    body = in_rounded_box(u, v, 28, 40, 80, 74, 6)
    # The raised part on top, narrower at its top than at its foot.
    hump = 35 <= v <= 40 and (43 + (40 - v) * 4 / 5) <= u <= (65 - (40 - v) * 4 / 5)
    return WHITE if body or hump else INDIGO


def picture(size: int) -> bytes:
    """The icon at one size, as rows of red, green, blue, alpha."""
    rows = bytearray()
    step = 100 / size / SAMPLES
    for py in range(size):
        rows.append(0)  # PNG: no filter for this row
        for px in range(size):
            red = green = blue = covered = 0
            for sy in range(SAMPLES):
                for sx in range(SAMPLES):
                    colour = colour_at((px * SAMPLES + sx + 0.5) * step, (py * SAMPLES + sy + 0.5) * step)
                    if colour:
                        covered += 1
                        red += colour[0]
                        green += colour[1]
                        blue += colour[2]
            if covered:
                rows += bytes((round(red / covered), round(green / covered), round(blue / covered), round(255 * covered / SAMPLES ** 2)))
            else:
                rows += bytes(4)
    return bytes(rows)


def png(size: int) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)  # 8 bits, colour with alpha
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(picture(size), 9)) + chunk(b"IEND", b"")


def main() -> None:
    pictures = [png(size) for size in SIZES]
    # An .ico is a list of pictures: a six-byte head, sixteen bytes about each picture, then the pictures.
    out = struct.pack("<HHH", 0, 1, len(SIZES))
    offset = 6 + 16 * len(SIZES)
    for size, data in zip(SIZES, pictures):
        out += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(out + b"".join(pictures))
    # And the largest one alone, for a person to look at.
    import tempfile

    preview = Path(tempfile.gettempdir()) / "gods-camera-icon-256.png"
    preview.write_bytes(pictures[-1])
    print(f"wrote {OUT} ({len(out) + sum(map(len, pictures))} bytes); to look at: {preview}")


if __name__ == "__main__":
    main()

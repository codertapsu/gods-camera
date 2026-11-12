"""The Vietnamese text the reading trials share, and how they draw and score it."""
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

TEXT = (
    "Sáng nay, mực nước trên sông Tiền tại Tân Châu đã lên thêm mười hai xăng-ti-mét so với hôm qua. "
    "Ông Nguyễn Văn Bảy, sáu mươi hai tuổi, ở xã Phú Hội, cho biết năm nay nước về sớm hơn mọi năm gần nửa tháng. "
    "Giá cá linh đầu mùa dao động từ một trăm hai mươi đến một trăm năm mươi nghìn đồng một ký. "
    "Bà Trần Thị Hạnh, chủ một quán ăn nhỏ gần bến Ninh Kiều, nói rằng khách du lịch thường hỏi món lẩu mắm. "
    "Mùi cá kho, mùi canh chua bông súng quyện vào nhau, báo hiệu một mùa lũ hiền hòa nữa đang về. "
    "Nghỉ ngơi, kỹ sư, quýt, thuở, khuỷu tay, ĐẶC BIỆT, Ở ĐÂY, ƯỚC MƠ, ỐNG NƯỚC, NGUYỄN, HUỲNH, PHẠM."
)

# A font is named by its file on Windows; the Mac has the first two under other names.
ON_THE_MAC = {
    "times": "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
    "arial": "/System/Library/Fonts/Supplemental/Arial.ttf",
}


def font(size: int, name: str = "times") -> ImageFont.FreeTypeFont:
    """A font by its file name in C:/Windows/Fonts, without the ending.

    Not every font has the Vietnamese letters: Georgia lacks 92 of them and
    draws boxes instead, which no reader can be blamed for.
    """
    for path in [f"C:/Windows/Fonts/{name}.ttf", f"C:/Windows/Fonts/{name}.ttc", ON_THE_MAC.get(name, "")]:
        if Path(path).is_file():
            return ImageFont.truetype(path, size)
    raise SystemExit(f"this computer has no font file for {name}")


def wrap(text: str, face: ImageFont.FreeTypeFont, width: int) -> list[str]:
    lines, line = [], ""
    for word in text.split():
        candidate = f"{line} {word}".strip()
        if face.getlength(candidate) <= width:
            line = candidate
        else:
            lines.append(line)
            line = word
    return lines + [line]


def draw(text: str, face: ImageFont.FreeTypeFont, size: int) -> Image.Image:
    picture = Image.new("RGB", (int(face.getlength(text)) + 16, size + 16), "white")
    ImageDraw.Draw(picture).text((8, 4), text, font=face, fill="black")
    return picture


def distance(a: str, b: str) -> int:
    """How many single-character changes turn one text into the other."""
    row = list(range(len(b) + 1))
    for i, x in enumerate(a, 1):
        previous, row[0] = row[0], i
        for j, y in enumerate(b, 1):
            previous, row[j] = row[j], min(row[j] + 1, row[j - 1] + 1, previous + (x != y))
    return row[-1]

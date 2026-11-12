"""Make a test document for the scanner: one newspaper-like page with known
text, and overlapping "photos" of it as a phone would take them.

Writes into --out:
  page.png            the whole page
  truth.txt           its text in reading order, one paragraph a line
  shots/shot-NN.jpg   overlapping photos, each slightly turned, skewed and unevenly lit

Usage: uv run python make_document.py --out <dir> [--language vi|en] [--columns 3] [--rows 3 --cols 2]
"""
import argparse
import random
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont



def font_file(*places: str) -> str:
    """Times New Roman, where this computer keeps it."""
    for place in places:
        if Path(place).is_file():
            return place
    raise SystemExit("Times New Roman was not found on this computer")


FONT = font_file("/System/Library/Fonts/Supplemental/Times New Roman.ttf", "C:/Windows/Fonts/times.ttf")
FONT_BOLD = font_file("/System/Library/Fonts/Supplemental/Times New Roman Bold.ttf", "C:/Windows/Fonts/timesbd.ttf")

# Text written for this test. It is plain invented news, with every Vietnamese tone mark in use.
VI = {
    "title": "Mùa nước nổi trở lại miền Tây",
    "paragraphs": [
        "Sáng nay, mực nước trên sông Tiền tại Tân Châu đã lên thêm mười hai xăng-ti-mét so với hôm qua. Người dân đầu nguồn bắt đầu chuẩn bị lưới, xuồng và dớn để đón mùa cá linh.",
        "Ông Nguyễn Văn Bảy, sáu mươi hai tuổi, ở xã Phú Hội, cho biết năm nay nước về sớm hơn mọi năm gần nửa tháng. Gia đình ông đã thả ba trăm ký cá giống vào ruộng từ tuần trước.",
        "Theo đài khí tượng thủy văn khu vực, đỉnh lũ năm nay có thể đạt mức báo động hai vào giữa tháng mười. Các địa phương được yêu cầu kiểm tra đê bao và những đoạn sạt lở trước ngày hai mươi.",
        "Ở chợ nổi Cái Răng, ghe chở bông điên điển, rau nhút và cá linh non đã xuất hiện nhiều hơn. Giá cá linh đầu mùa dao động từ một trăm hai mươi đến một trăm năm mươi nghìn đồng một ký.",
        "Bà Trần Thị Hạnh, chủ một quán ăn nhỏ gần bến Ninh Kiều, nói rằng khách du lịch thường hỏi món lẩu mắm ngay khi nghe tin nước về. Quán của bà mở cửa từ năm giờ sáng đến chín giờ tối.",
        "Ngành nông nghiệp tỉnh khuyến cáo nông dân không xuống giống vụ ba ở những vùng trũng. Thay vào đó, bà con có thể xả lũ lấy phù sa, vừa rửa phèn vừa diệt mầm bệnh trong đất.",
        "Nhiều trường học vùng ven sông đã lập kế hoạch đưa đón học sinh bằng đò. Mỗi chuyến đò đều có áo phao cho từng em, và giáo viên thay phiên nhau trực ở bến vào buổi sáng.",
        "Các nhà khoa học cho rằng lượng phù sa về đồng bằng đang giảm dần qua từng năm. Họ đề nghị theo dõi kỹ hơn những thay đổi ở thượng nguồn và chia sẻ số liệu giữa các nước trong lưu vực.",
        "Dù vậy, với người dân nơi đây, mùa nước nổi vẫn là mùa của sự sống. Tiếng máy đuôi tôm lại rộn ràng trên những cánh đồng trắng nước, và trẻ con lại tập bơi bên hiên nhà.",
        "Chiều xuống, khói bếp bay lên từ những mái nhà sàn. Mùi cá kho, mùi canh chua bông súng quyện vào nhau, báo hiệu một mùa lũ hiền hòa nữa đang về.",
    ],
}
EN = {
    "title": "Harbour ferry returns after forty years",
    "paragraphs": [
        "The first passenger ferry to cross the harbour since 1986 left the north pier at seven this morning. About two hundred people were on board, many of them carrying bicycles.",
        "The service will run every twenty minutes at peak times and every forty minutes through the middle of the day. A single ticket costs two pounds fifty and is valid for ninety minutes.",
        "City engineers spent eighteen months rebuilding the landing stages on both sides. The old timber piles were replaced with steel, and ramps were added so that wheelchairs can board without help.",
        "Maria Okafor, who has run the kiosk beside the south pier for eleven years, said she had ordered twice her usual amount of coffee. By nine o'clock she had sold most of it.",
        "Not everyone is pleased. Rowing clubs say the wash from the new boats makes the inner harbour harder to use, and they have asked for a speed limit near the boathouses.",
        "The harbour authority replied that the ferries already slow to six knots inside the breakwater. It has promised to publish wash measurements after the first month of service.",
        "The two boats were built at a yard three hundred miles up the coast and arrived under their own power in March. Each carries two hundred and twenty passengers and thirty bicycles.",
        "If passenger numbers hold, a third boat will be ordered next year, and the timetable will be extended until midnight on Fridays and Saturdays.",
    ],
}


def wrap(draw, text, font, width):
    lines, line = [], ""
    for word in text.split():
        candidate = f"{line} {word}".strip()
        if draw.textlength(candidate, font=font) <= width:
            line = candidate
        else:
            lines.append(line)
            line = word
    if line:
        lines.append(line)
    return lines


def render(content, columns, page=(2400, 3300)):
    """Draw the page. Returns the picture and the paragraphs in reading order."""
    image = Image.new("RGB", page, (250, 248, 242))
    draw = ImageDraw.Draw(image)
    title_font = ImageFont.truetype(FONT_BOLD, 110)
    body_font = ImageFont.truetype(FONT, 46)
    margin, gutter = 150, 80
    y = 170
    draw.text((margin, y), content["title"], font=title_font, fill=(20, 20, 20))
    y += 190
    draw.line([(margin, y), (page[0] - margin, y)], fill=(60, 60, 60), width=4)
    y += 70
    width = (page[0] - 2 * margin - (columns - 1) * gutter) // columns
    line_height, paragraph_gap = 62, 34
    # Lay the paragraphs out in order, moving to the next column when one is full.
    laid = [(p, wrap(draw, p, body_font, width)) for p in content["paragraphs"]]
    total = sum(len(lines) * line_height + paragraph_gap for _, lines in laid)
    column_height = total / columns + line_height * 2
    column, cy, order = 0, y, []
    for paragraph, lines in laid:
        needed = len(lines) * line_height
        if cy + needed > y + column_height and column < columns - 1:
            column, cy = column + 1, y
        x = margin + column * (width + gutter)
        for line in lines:
            draw.text((x, cy), line, font=body_font, fill=(25, 25, 25))
            cy += line_height
        cy += paragraph_gap
        order.append(paragraph)
    return image, [content["title"]] + order


def shots(image, rows, cols, overlap, rng):
    """Cut overlapping photos out of the page, each a little turned, skewed and shaded."""
    w, h = image.size
    tile_w = int(w / (cols - (cols - 1) * overlap))
    tile_h = int(h / (rows - (rows - 1) * overlap))
    out = []
    for r in range(rows):
        for c in range(cols):
            x0 = int(c * tile_w * (1 - overlap))
            y0 = int(r * tile_h * (1 - overlap))
            box = (x0, y0, min(w, x0 + tile_w), min(h, y0 + tile_h))
            # A margin around the tile gives the rotation something real to turn in.
            pad = 60
            padded = image.crop((max(0, box[0] - pad), max(0, box[1] - pad), min(w, box[2] + pad), min(h, box[3] + pad)))
            angle = rng.uniform(-2.5, 2.5)
            shot = padded.rotate(angle, resample=Image.BICUBIC, fillcolor=(250, 248, 242))
            sw, sh = shot.size
            # A gentle keystone, as when the phone is not held flat.
            k = rng.uniform(-0.02, 0.02)
            shot = shot.transform(
                (sw, sh), Image.QUAD,
                (0, 0, k * sw, sh, sw - k * sw, sh, sw, 0),
                resample=Image.BICUBIC, fillcolor=(250, 248, 242))
            # Uneven light: darker towards one corner.
            shade = Image.linear_gradient("L").rotate(rng.choice([0, 90, 180, 270])).resize((sw, sh))
            dark = Image.new("RGB", (sw, sh), (150, 145, 135))
            shot = Image.composite(shot, dark, shade.point(lambda v: 150 + v * 105 // 255))
            shot = shot.filter(ImageFilter.GaussianBlur(0.6))
            out.append(shot)
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--language", choices=["vi", "en"], default="vi")
    ap.add_argument("--columns", type=int, default=3)
    ap.add_argument("--rows", type=int, default=3)
    ap.add_argument("--cols", type=int, default=2)
    ap.add_argument("--overlap", type=float, default=0.3)
    ap.add_argument("--seed", type=int, default=7)
    args = ap.parse_args()
    content = VI if args.language == "vi" else EN
    image, truth = render(content, args.columns)
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "shots").mkdir(exist_ok=True)
    image.save(args.out / "page.png")
    (args.out / "truth.txt").write_text("\n".join(truth) + "\n", encoding="utf-8")
    rng = random.Random(args.seed)
    made = shots(image, args.rows, args.cols, args.overlap, rng)
    for i, shot in enumerate(made):
        shot.save(args.out / "shots" / f"shot-{i:02d}.jpg", quality=88)
    print(f"page {image.size}, {len(truth)} paragraphs, {len(made)} shots in {args.out}")


if __name__ == "__main__":
    main()

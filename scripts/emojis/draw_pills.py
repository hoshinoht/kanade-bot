"""Draw the difficulty pills `assets/emojis/diff_{n,h,c,x}.png`.

128x128 RGBA PNGs that stay legible at 16 px: a solid rounded plate with
one thick-stroked letter, in the portal's pill colours (`--pill-*`). Pure
standard library; deterministic, so re-running rewrites identical bytes.

    python3 scripts/emojis/draw_pills.py [OUT_DIR]
"""

import math
import struct
import sys
import zlib
from pathlib import Path

SIZE = 128
SAMPLES = 4  # per axis, for anti-aliasing
PLATE = (4.0, 12.0, 124.0, 116.0)  # left, top, right, bottom
RADIUS = 36.0
STROKE = 19.0
RING = 7.0  # accent ring on the black plates

WHITE = (255, 255, 255)
PILLS = {
    # name: (plate, letter colour, accent ring or None)
    "diff_n": ((0x2F, 0x93, 0xB3), WHITE, None),
    "diff_h": ((0xD9, 0x40, 0x7F), WHITE, None),
    "diff_c": ((0x17, 0x16, 0x1C), (0xF0, 0xA0, 0x30), (0xF0, 0xA0, 0x30)),
    "diff_x": ((0x17, 0x16, 0x1C), (0xFF, 0x5A, 0x3C), (0xFF, 0x5A, 0x3C)),
}

TOP, BOTTOM, LEFT, RIGHT = 36.0, 92.0, 40.0, 88.0


def arc(cx, cy, r, start, end, steps=48):
    return [
        (cx + r * math.cos(math.radians(a)), cy - r * math.sin(math.radians(a)))
        for a in (start + (end - start) * i / steps for i in range(steps + 1))
    ]


# Each glyph is a list of polylines, stroked with round caps and joins.
GLYPHS = {
    "diff_n": [[(LEFT, BOTTOM), (LEFT, TOP), (RIGHT, BOTTOM), (RIGHT, TOP)]],
    "diff_h": [
        [(LEFT, TOP), (LEFT, BOTTOM)],
        [(RIGHT, TOP), (RIGHT, BOTTOM)],
        [(LEFT, 64.0), (RIGHT, 64.0)],
    ],
    "diff_c": [arc(68.0, 64.0, 29.0, 50.0, 310.0)],
    "diff_x": [[(LEFT, TOP), (RIGHT, BOTTOM)], [(RIGHT, TOP), (LEFT, BOTTOM)]],
}


def rounded_rect_distance(x, y, rect, radius):
    """Signed distance to a rounded rectangle (negative inside)."""
    left, top, right, bottom = rect
    cx, cy = (left + right) / 2, (top + bottom) / 2
    hx, hy = (right - left) / 2 - radius, (bottom - top) / 2 - radius
    qx, qy = abs(x - cx) - hx, abs(y - cy) - hy
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    return outside + min(max(qx, qy), 0.0) - radius


def segment_distance(x, y, a, b):
    ax, ay = a
    bx, by = b
    dx, dy = bx - ax, by - ay
    length = dx * dx + dy * dy
    t = 0.0 if length == 0 else max(0.0, min(1.0, ((x - ax) * dx + (y - ay) * dy) / length))
    return math.hypot(x - (ax + t * dx), y - (ay + t * dy))


def in_glyph(x, y, lines):
    half = STROKE / 2
    return any(
        segment_distance(x, y, line[i], line[i + 1]) <= half
        for line in lines
        for i in range(len(line) - 1)
    )


def colour_at(x, y, plate, letter, ring, lines):
    """The sample's colour, or None outside the plate."""
    edge = rounded_rect_distance(x, y, PLATE, RADIUS)
    if edge > 0:
        return None
    if in_glyph(x, y, lines):
        return letter
    if ring is not None and edge > -RING:
        return ring
    return plate


def draw(name):
    plate, letter, ring = PILLS[name]
    lines = GLYPHS[name]
    rows = []
    step = 1.0 / SAMPLES
    for py in range(SIZE):
        row = bytearray([0])  # filter: none
        for px in range(SIZE):
            r = g = b = covered = 0
            for sy in range(SAMPLES):
                for sx in range(SAMPLES):
                    found = colour_at(
                        px + (sx + 0.5) * step, py + (sy + 0.5) * step, plate, letter, ring, lines
                    )
                    if found is not None:
                        r, g, b = r + found[0], g + found[1], b + found[2]
                        covered += 1
            if covered:
                row += bytes((r // covered, g // covered, b // covered))
                row.append(round(255 * covered / SAMPLES**2))
            else:
                row += b"\0\0\0\0"
        rows.append(bytes(row))
    return png(b"".join(rows))


def png(raw):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def main():
    root = Path(__file__).resolve().parents[2]
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else root / "assets/emojis"
    out.mkdir(parents=True, exist_ok=True)
    for name in PILLS:
        (out / f"{name}.png").write_bytes(draw(name))
        print(out / f"{name}.png")


if __name__ == "__main__":
    main()

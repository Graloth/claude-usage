#!/usr/bin/env python3
"""Draws the app icon (a usage ring on a dark rounded square) as a 1024x1024 PNG.

Standard library only. Feed the result to `pnpm tauri icon` to produce every
platform's icon files:

    python3 scripts/make-icon.py /tmp/icon.png
    pnpm tauri icon /tmp/icon.png
"""
import math
import struct
import sys
import zlib

SIZE = 1024
SUPERSAMPLE = 3  # per axis, for anti-aliased edges
BACKGROUND = (27, 27, 31)
BORDER = (58, 58, 67)
TRACK = (58, 58, 67)
FILL = (217, 119, 87)  # Claude orange, same as the tray ring
FRACTION = 0.72  # how much of the ring is filled
# macOS icons leave transparent padding around the shape; without it the icon
# looks oversized next to its neighbours in the Dock.
SCALE = 0.82
CORNER = 0.225 * SIZE
RING_OUTER = 0.34 * SIZE
RING_INNER = 0.22 * SIZE


def inside_rounded_square(x, y, inset=0.0):
    """True if (x, y) lies in the rounded square shrunk by `inset` pixels."""
    half = SIZE / 2 - inset
    radius = CORNER - inset
    dx = abs(x - SIZE / 2) - (half - radius)
    dy = abs(y - SIZE / 2) - (half - radius)
    if dx <= 0 or dy <= 0:
        return abs(x - SIZE / 2) <= half and abs(y - SIZE / 2) <= half
    return dx * dx + dy * dy <= radius * radius


def shade(x, y):
    """(r, g, b, a) of one sample point, 0..255 per channel."""
    # Draw the artwork at full size, then shrink it about the centre.
    x = (x - SIZE / 2) / SCALE + SIZE / 2
    y = (y - SIZE / 2) / SCALE + SIZE / 2
    if not inside_rounded_square(x, y):
        return (0, 0, 0, 0)
    colour = BORDER if not inside_rounded_square(x, y, inset=10.0) else BACKGROUND
    dx, dy = x - SIZE / 2, y - SIZE / 2
    distance = math.hypot(dx, dy)
    if RING_INNER <= distance <= RING_OUTER:
        turn = (math.atan2(dx, -dy) % (2 * math.pi)) / (2 * math.pi)
        colour = FILL if turn < FRACTION else TRACK
    return (*colour, 255)


def render():
    rows = []
    step = 1.0 / SUPERSAMPLE
    for py in range(SIZE):
        row = bytearray([0])  # PNG filter type 0 (none)
        for px in range(SIZE):
            r = g = b = a = 0
            for sy in range(SUPERSAMPLE):
                for sx in range(SUPERSAMPLE):
                    cr, cg, cb, ca = shade(px + (sx + 0.5) * step, py + (sy + 0.5) * step)
                    # Premultiplied accumulation so transparent samples do not tint edges.
                    r += cr * ca
                    g += cg * ca
                    b += cb * ca
                    a += ca
            if a:
                row += bytes((round(r / a), round(g / a), round(b / a), round(a / SUPERSAMPLE**2)))
            else:
                row += bytes((0, 0, 0, 0))
        rows.append(bytes(row))
    return b"".join(rows)


def png(raw):
    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)  # 8-bit RGBA
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "icon.png"
    with open(out, "wb") as f:
        f.write(png(render()))
    print(f"wrote {out}")

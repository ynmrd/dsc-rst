#!/usr/bin/env python3
"""Generate the Discord Native app icon (original artwork: speech bubble with three dots).

Outputs (stdlib only, deterministic):
  assets/icon.png        256x256
  assets/icon.ico        16/24/32/48/64/128/256 PNG-compressed entries (Windows exe resource)
  assets/icon64.rgba     raw 64x64 RGBA for the window and tray icon
"""
import os
import struct
import zlib

BG = (91, 110, 245)
FG = (255, 255, 255)
DOT = (91, 110, 245)


def inside_rounded_rect(x, y, x0, y0, x1, y1, r):
    if x < x0 or x > x1 or y < y0 or y > y1:
        return False
    cx = min(max(x, x0 + r), x1 - r)
    cy = min(max(y, y0 + r), y1 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r


def inside_triangle(x, y, a, b, c):
    def sign(p, q, r):
        return (p[0] - r[0]) * (q[1] - r[1]) - (q[0] - r[0]) * (p[1] - r[1])

    d1, d2, d3 = sign((x, y), a, b), sign((x, y), b, c), sign((x, y), c, a)
    neg = d1 < 0 or d2 < 0 or d3 < 0
    pos = d1 > 0 or d2 > 0 or d3 > 0
    return not (neg and pos)


def shade(u, v):
    """Colour at unit coordinates (0..1); None = transparent."""
    if not inside_rounded_rect(u, v, 0.0, 0.0, 1.0, 1.0, 0.22):
        return None
    bubble = inside_rounded_rect(u, v, 0.18, 0.22, 0.82, 0.68, 0.14) or inside_triangle(
        u, v, (0.30, 0.62), (0.46, 0.66), (0.26, 0.82)
    )
    if bubble:
        for cx in (0.35, 0.50, 0.65):
            if (u - cx) ** 2 + (v - 0.45) ** 2 <= 0.055 ** 2:
                return DOT
        return FG
    return BG


def render(size, ss=4):
    px = bytearray()
    for y in range(size):
        for x in range(size):
            acc = [0, 0, 0, 0]
            for sy in range(ss):
                for sx in range(ss):
                    c = shade((x + (sx + 0.5) / ss) / size, (y + (sy + 0.5) / ss) / size)
                    if c is not None:
                        acc[0] += c[0]
                        acc[1] += c[1]
                        acc[2] += c[2]
                        acc[3] += 255
            n = ss * ss
            a = acc[3] / n
            if a > 0:
                px += bytes((int(acc[0] / (acc[3] / 255)), int(acc[1] / (acc[3] / 255)), int(acc[2] / (acc[3] / 255)), int(round(a))))
            else:
                px += b"\x00\x00\x00\x00"
    return bytes(px)


def png(size, rgba):
    raw = b"".join(b"\x00" + rgba[y * size * 4:(y + 1) * size * 4] for y in range(size))

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def ico(images):
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    entries, blobs = b"", b""
    for size, data in images:
        dim = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        blobs += data
        offset += len(data)
    return header + entries + blobs


def main():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets")
    os.makedirs(root, exist_ok=True)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    rendered = {s: render(s) for s in sizes}
    with open(os.path.join(root, "icon.png"), "wb") as f:
        f.write(png(256, rendered[256]))
    with open(os.path.join(root, "icon.ico"), "wb") as f:
        f.write(ico([(s, png(s, rendered[s])) for s in sizes]))
    with open(os.path.join(root, "icon64.rgba"), "wb") as f:
        f.write(rendered[64])
    print("wrote", ", ".join(sorted(os.listdir(root))))


if __name__ == "__main__":
    main()

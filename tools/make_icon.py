"""Генерирует app/assets/icon.ico (PNG внутри ICO). Запуск: python tools/make_icon.py"""
import struct, zlib

def pip(x, y, poly):
    inside = False
    j = len(poly) - 1
    for i in range(len(poly)):
        xi, yi = poly[i]; xj, yj = poly[j]
        if (yi > y) != (yj > y) and x < (xj - xi) * (y - yi) / (yj - yi) + xi:
            inside = not inside
        j = i
    return inside

def rect(x0, y0, x1, y1):
    return [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]

SHAPES = [
    rect(0.20, 0.30, 0.62, 0.38), [(0.56, 0.20), (0.56, 0.48), (0.80, 0.34)],
    rect(0.38, 0.62, 0.80, 0.70), [(0.44, 0.52), (0.44, 0.80), (0.20, 0.66)],
]

def in_round(x, y, r=0.22):
    cx = min(max(x, r), 1 - r); cy = min(max(y, r), 1 - r)
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r

def render(n):
    ss = 4
    rows = []
    for py in range(n):
        row = bytearray([0])
        for px in range(n):
            a = w = 0
            for sy in range(ss):
                for sx in range(ss):
                    x = (px + (sx + .5) / ss) / n; y = (py + (sy + .5) / ss) / n
                    if in_round(x, y):
                        a += 1
                        if any(pip(x, y, s) for s in SHAPES):
                            w += 1
            t = ss * ss
            if a == 0:
                row += bytes([0, 0, 0, 0]); continue
            f = w / a
            r = round(0x2f * (1 - f) + 255 * f); g = round(0x6f * (1 - f) + 255 * f); b = round(0xe4 * (1 - f) + 255 * f)
            row += bytes([r, g, b, round(255 * a / t)])
        rows.append(bytes(row))
    raw = b"".join(rows)
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")

sizes = [16, 32, 48, 256]
imgs = [render(n) for n in sizes]
out = struct.pack("<HHH", 0, 1, len(sizes))
off = 6 + 16 * len(sizes)
for n, d in zip(sizes, imgs):
    out += struct.pack("<BBBBHHII", n % 256, n % 256, 0, 0, 1, 32, len(d), off)
    off += len(d)
open("app/assets/icon.ico", "wb").write(out + b"".join(imgs))
print("ok", sum(map(len, imgs)))

#!/usr/bin/env python3
"""app-icon.png üretir (1024x1024).

Harici bağımlılık yok — PNG'yi zlib + struct ile elle yazıyoruz.
Çıktı `cargo tauri icon app-icon.png` komutuna girdi olur; .ico/.icns/png
varyantlarını Tauri CLI türetir.

Tasarım: mor gradient zemin üzerinde iki kart ve aralarında aktarım oku.
"""

import struct
import zlib
from pathlib import Path

SIZE = 1024


def lerp(a, b, t):
    return a + (b - a) * t


def rounded_rect(x, y, w, h, r, px, py):
    """(px,py) noktası yuvarlatılmış dikdörtgenin içinde mi?"""
    if not (x <= px < x + w and y <= py < y + h):
        return False
    cx = min(max(px, x + r), x + w - r)
    cy = min(max(py, y + r), y + h - r)
    return (px - cx) ** 2 + (py - cy) ** 2 <= r * r


def build_pixels():
    rows = []
    # Kart geometrisi
    card_w, card_h, radius = 300, 380, 34
    left = (150, 300)
    right = (574, 300)

    for y in range(SIZE):
        row = bytearray()
        for x in range(SIZE):
            # Köşegen gradient: #667eea -> #764ba2
            t = (x + y) / (2 * SIZE)
            r = int(lerp(0x66, 0x76, t))
            g = int(lerp(0x7E, 0x4B, t))
            b = int(lerp(0xEA, 0xA2, t))

            # Sol kart (kaynak) — yarı saydam beyaz
            if rounded_rect(left[0], left[1], card_w, card_h, radius, x, y):
                r, g, b = 0xDD, 0xE3, 0xF2

            # Sağ kart (hedef) — tam beyaz
            if rounded_rect(right[0], right[1], card_w, card_h, radius, x, y):
                r, g, b = 0xFF, 0xFF, 0xFF

            # Kartların üstündeki koyu başlık şeridi (Mikro grid header'ı)
            for cx0 in (left[0], right[0]):
                if rounded_rect(cx0, 300, card_w, 74, radius, x, y):
                    r, g, b = 0x3C, 0x4A, 0x5E

            # Ortadaki aktarım oku (gövde + uç), kartların üstünde
            if 430 <= y < 500 and 392 <= x < 640:
                r, g, b = 0x22, 0xA3, 0x55
            if 392 <= y < 538:
                # Ok ucu: x arttıkça daralan üçgen
                half = (538 - 392) // 2
                cy = 465
                tip_w = half - abs(y - cy)
                if 620 <= x < 620 + tip_w:
                    r, g, b = 0x22, 0xA3, 0x55

            row += bytes((r, g, b))
        rows.append(bytes(row))
    return rows


def png_bytes(rows, size):
    # Tauri uygulama ikonlarında RGBA bekler; kaynak çizim RGB olduğu için
    # her piksele opak alfa kanalı ekliyoruz.
    rgba_rows = (
        b"".join(row[i : i + 3] + b"\xff" for i in range(0, len(row), 3))
        for row in rows
    )
    raw = b"".join(b"\x00" + row for row in rgba_rows)
    comp = zlib.compress(raw, 9)

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)  # 8-bit RGBA
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", comp)
        + chunk(b"IEND", b"")
    )


def write_png(path, rows, size):
    png = png_bytes(rows, size)
    Path(path).write_bytes(png)
    print(f"{path} yazıldı ({len(png):,} bayt)")


def scaled_rows(rows, size):
    """1024x1024 kaynak resmi nearest-neighbour ile küçültür."""
    step = SIZE // size
    return [
        b"".join(row[x * 3 : x * 3 + 3] for x in range(0, SIZE, step))
        for row in rows[::step]
    ]


def write_ico(path, png):
    """Windows 10/11'in desteklediği, PNG taşıyan tek-kare .ico dosyası."""
    header = struct.pack("<HHH", 0, 1, 1)
    # 0, ICO biçiminde 256 pikseli ifade eder.
    entry = struct.pack("<BBBBHHII", 0, 0, 0, 0, 1, 32, len(png), 22)
    Path(path).write_bytes(header + entry + png)
    print(f"{path} yazıldı ({len(header) + len(entry) + len(png):,} bayt)")


if __name__ == "__main__":
    root = Path(__file__).resolve().parent.parent
    rows = build_pixels()
    write_png(root / "app-icon.png", rows, SIZE)

    icons = root / "src-tauri" / "icons"
    icons.mkdir(parents=True, exist_ok=True)
    for size, name in ((32, "32x32.png"), (128, "128x128.png"), (256, "128x128@2x.png")):
        write_png(icons / name, scaled_rows(rows, size), size)
    write_ico(icons / "icon.ico", png_bytes(rows, SIZE))

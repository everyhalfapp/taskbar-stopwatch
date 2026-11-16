"""assets/app.ico 생성 (외부 라이브러리 없이).

    python scripts/make_icon.py

어두운 둥근 사각형 위에 파란 스톱워치 모양. 16~256px 여러 크기를 한 파일에 담는다.
"""
import math
import os
import struct
import zlib

ACCENT = (0x4F, 0x8C, 0xFF)
BG = (0x16, 0x19, 0x22)
WHITE = (0xF2, 0xF4, 0xF8)
SIZES = [16, 20, 24, 32, 40, 48, 64, 256]
SS = 4  # 슈퍼샘플링


def shade(x, y):
    """32 단위 설계 좌표 (x, y) 의 색 (r, g, b, a) — 없으면 None"""
    # 둥근 사각형 배경
    r = 7.0
    cx, cy = min(max(x, 1 + r), 31 - r), min(max(y, 1 + r), 31 - r)
    if (x - cx) ** 2 + (y - cy) ** 2 > r * r or not (1 <= x <= 31 and 1 <= y <= 31):
        return None
    dx, dy = x - 16.0, y - 17.4
    d = math.hypot(dx, dy)
    # 바늘: 중심에서 2시 방향으로 (전원 버튼처럼 보이지 않게)
    ang = math.radians(50)
    hx, hy = math.sin(ang) * 6.6, -math.cos(ang) * 6.6
    t = max(0.0, min(1.0, (dx * hx + dy * hy) / (hx * hx + hy * hy)))
    if math.hypot(dx - hx * t, dy - hy * t) < 1.3:
        return WHITE
    if d < 2.1:                                  # 축
        return WHITE
    if 8.4 < d < 10.9:                           # 테두리
        return ACCENT
    if abs(dx) < 2.2 and -13.4 < dy < -10.6:    # 꼭지
        return ACCENT
    return BG


def render(size):
    k = 32.0 / size
    px = bytearray()
    for y in range(size):
        for x in range(size):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(SS):
                for sx in range(SS):
                    c = shade((x + (sx + 0.5) / SS) * k, (y + (sy + 0.5) / SS) * k)
                    if c:
                        acc[0] += c[0]; acc[1] += c[1]; acc[2] += c[2]; acc[3] += 1
            n = SS * SS
            a = acc[3] / n
            if acc[3]:
                r, g, b = (acc[i] / acc[3] for i in range(3))
            else:
                r = g = b = 0
            px += bytes((int(r), int(g), int(b), int(round(a * 255))))
    return px  # RGBA, 위→아래


def png(size, rgba):
    raw = b"".join(b"\x00" + bytes(rgba[y * size * 4:(y + 1) * size * 4]) for y in range(size))

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def bmp(size, rgba):
    hdr = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    rows = []
    for y in range(size - 1, -1, -1):  # 아래→위
        row = bytearray()
        for x in range(size):
            i = (y * size + x) * 4
            row += bytes((rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]))
        rows.append(bytes(row))
    mask_row = ((size + 31) // 32) * 4
    return hdr + b"".join(rows) + b"\x00" * (mask_row * size)


def main():
    images = []
    for s in SIZES:
        rgba = render(s)
        images.append((s, png(s, rgba) if s >= 256 else bmp(s, rgba)))
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    body = b""
    for s, data in images:
        out += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset + len(body))
        body += data
    path = os.path.join(os.path.dirname(__file__), "..", "assets", "app.ico")
    with open(path, "wb") as f:
        f.write(out + body)
    print("wrote", os.path.normpath(path), len(out + body), "bytes")


if __name__ == "__main__":
    main()

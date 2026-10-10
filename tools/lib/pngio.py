"""Minimal PNG reader/writer (stdlib only) for screenshot tests.

Supports 8-bit RGB/RGBA/gray, non-interlaced images (what QEMU writes) and
PPM (P6) as a fallback screendump format.
"""
import struct
import zlib


class Image:
    """RGB image, row-major bytes (3 bytes per pixel)."""

    def __init__(self, width, height, data):
        self.width = width
        self.height = height
        self.data = data

    def pixel(self, x, y):
        i = (y * self.width + x) * 3
        return tuple(self.data[i:i + 3])

    def mean(self, x0, y0, x1, y1):
        """Mean RGB over the rectangle [x0, x1) x [y0, y1)."""
        acc = [0, 0, 0]
        n = 0
        for y in range(max(0, y0), min(self.height, y1)):
            row = y * self.width * 3
            for x in range(max(0, x0), min(self.width, x1)):
                i = row + x * 3
                acc[0] += self.data[i]
                acc[1] += self.data[i + 1]
                acc[2] += self.data[i + 2]
                n += 1
        return tuple(v / max(n, 1) for v in acc)

    def crop(self, x0, y0, x1, y1):
        out = bytearray()
        for y in range(y0, y1):
            row = (y * self.width + x0) * 3
            out += self.data[row:row + (x1 - x0) * 3]
        return Image(x1 - x0, y1 - y0, bytes(out))


def _paeth(a, b, c):
    p = a + b - c
    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    return b if pb <= pc else c


def read_png(path):
    with open(path, "rb") as f:
        blob = f.read()
    if blob[:2] == b"P6":
        return read_ppm_bytes(blob)
    if blob[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path}: not a PNG")
    pos = 8
    idat = bytearray()
    width = height = depth = ctype = None
    while pos < len(blob):
        (length,) = struct.unpack(">I", blob[pos:pos + 4])
        kind = blob[pos + 4:pos + 8]
        chunk = blob[pos + 8:pos + 8 + length]
        pos += 12 + length
        if kind == b"IHDR":
            width, height, depth, ctype, _, _, interlace = struct.unpack(">IIBBBBB", chunk)
            if depth != 8 or interlace != 0 or ctype not in (0, 2, 6):
                raise ValueError(f"{path}: unsupported PNG (depth {depth}, type {ctype})")
        elif kind == b"IDAT":
            idat += chunk
        elif kind == b"IEND":
            break
    bpp = {0: 1, 2: 3, 6: 4}[ctype]
    raw = zlib.decompress(bytes(idat))
    stride = width * bpp
    prev = bytearray(stride)
    out = bytearray(width * height * 3)
    o = 0
    p = 0
    for _ in range(height):
        ftype = raw[p]
        line = bytearray(raw[p + 1:p + 1 + stride])
        p += 1 + stride
        if ftype == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 255
        elif ftype == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 255
        elif ftype == 3:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 255
        elif ftype == 4:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                up_left = prev[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + _paeth(left, prev[i], up_left)) & 255
        if ctype == 2:
            out[o:o + stride] = line
        elif ctype == 6:
            out[o:o + width * 3] = bytes(b for i, b in enumerate(line) if i % 4 != 3)
        else:
            out[o:o + width * 3] = bytes(b for b in line for _ in range(3))
        o += width * 3
        prev = line
    return Image(width, height, bytes(out))


def read_ppm_bytes(blob):
    fields = []
    pos = 2
    while len(fields) < 3:
        while blob[pos:pos + 1].isspace():
            pos += 1
        if blob[pos:pos + 1] == b"#":
            while blob[pos:pos + 1] != b"\n":
                pos += 1
            continue
        start = pos
        while not blob[pos:pos + 1].isspace():
            pos += 1
        fields.append(int(blob[start:pos]))
    pos += 1
    width, height, _ = fields
    return Image(width, height, blob[pos:pos + width * height * 3])


def write_png(path, img):
    raw = bytearray()
    stride = img.width * 3
    for y in range(img.height):
        raw.append(0)
        raw += img.data[y * stride:(y + 1) * stride]

    def chunk(kind, data):
        c = struct.pack(">I", len(data)) + kind + data
        return c + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", img.width, img.height, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(bytes(raw), 6)))
        f.write(chunk(b"IEND", b""))


def diff(a, b, tolerance=8):
    """Return (fraction of pixels differing by more than `tolerance`, diff image)."""
    if (a.width, a.height) != (b.width, b.height):
        return 1.0, None
    bad = 0
    out = bytearray(len(a.data))
    for i in range(0, len(a.data), 3):
        d = max(abs(a.data[i] - b.data[i]), abs(a.data[i + 1] - b.data[i + 1]),
                abs(a.data[i + 2] - b.data[i + 2]))
        if d > tolerance:
            bad += 1
            out[i] = 255
        else:
            g = (a.data[i] + a.data[i + 1] + a.data[i + 2]) // 12
            out[i] = out[i + 1] = out[i + 2] = g
    return bad / (a.width * a.height), Image(a.width, a.height, bytes(out))

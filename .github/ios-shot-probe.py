#!/usr/bin/env python3
"""Report the colours in a rectangle of a PNG, for asserting things about a screenshot.

Prints one line: `<count> <r> <g> <b>` for each distinct colour in the rectangle,
commonest first. Exits non-zero if the file cannot be read.

Pure standard library on purpose. The macOS runners have no Pillow and no
ImageMagick worth relying on, and the alternative — asserting that two
screenshots are byte-identical — cannot say anything about a screen with a
turning figure on it, where no two launches ever land on the same frame.

Usage: ios-shot-probe.py <png> <x> <y> <w> <h>
"""

import struct
import sys
import zlib


def read_png(path):
    """Returns (width, height, rows) with rows as bytearrays of RGB(A) samples."""
    with open(path, "rb") as f:
        data = f.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError(f"{path} is not a PNG")

    width = height = depth = colour = None
    idat = bytearray()
    at = 8
    while at < len(data):
        (length,) = struct.unpack(">I", data[at : at + 4])
        kind = data[at + 4 : at + 8]
        body = data[at + 8 : at + 8 + length]
        at += 12 + length
        if kind == b"IHDR":
            width, height, depth, colour = struct.unpack(">IIBB", body[:10])
            if body[10:13] != b"\x00\x00\x00":
                raise ValueError("compressed, filtered or interlaced in a way this cannot read")
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break

    if depth != 8 or colour not in (2, 6):
        raise ValueError(f"only 8-bit RGB or RGBA is handled, got depth={depth} colour={colour}")
    channels = 3 if colour == 2 else 4
    stride = width * channels

    raw = zlib.decompress(bytes(idat))
    rows = []
    previous = bytearray(stride)
    at = 0
    for _ in range(height):
        filter_type = raw[at]
        line = bytearray(raw[at + 1 : at + 1 + stride])
        at += 1 + stride
        unfilter(filter_type, line, previous, channels)
        rows.append(line)
        previous = line
    return width, height, rows, channels


def unfilter(filter_type, line, previous, channels):
    """Undoes one PNG scanline filter in place, as the spec defines them."""
    if filter_type == 0:
        return
    for i in range(len(line)):
        left = line[i - channels] if i >= channels else 0
        up = previous[i]
        if filter_type == 1:
            line[i] = (line[i] + left) & 0xFF
        elif filter_type == 2:
            line[i] = (line[i] + up) & 0xFF
        elif filter_type == 3:
            line[i] = (line[i] + ((left + up) >> 1)) & 0xFF
        elif filter_type == 4:
            upleft = previous[i - channels] if i >= channels else 0
            guess = left + up - upleft
            a, b, c = abs(guess - left), abs(guess - up), abs(guess - upleft)
            nearest = left if a <= b and a <= c else (up if b <= c else upleft)
            line[i] = (line[i] + nearest) & 0xFF
        else:
            raise ValueError(f"unknown scanline filter {filter_type}")


def main():
    if len(sys.argv) != 6:
        sys.exit(__doc__)
    path, x, y, w, h = sys.argv[1], *map(int, sys.argv[2:])

    width, height, rows, channels = read_png(path)
    if x < 0 or y < 0 or x + w > width or y + h > height:
        sys.exit(f"{w}x{h} at {x},{y} does not fit in {width}x{height}")

    counts = {}
    for row in rows[y : y + h]:
        for i in range(x, x + w):
            pixel = tuple(row[i * channels : i * channels + 3])
            counts[pixel] = counts.get(pixel, 0) + 1
    for pixel, count in sorted(counts.items(), key=lambda kv: -kv[1]):
        print(count, *pixel)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Fail a shot of the unread-mention mark that framed the wrong state.

    scripts/shot-guard-unread-mentions.py <shot.png> --expect before|after

Run it straight after writing each PNG. A non-zero exit means the image is not
evidence of anything and must not be filed: the capture is retaken instead.

The frame it guards is the staged pair from `sync::fixture` — a mentioned chat
carrying its unread count with the `@` beside it, above an unread chat carrying
a count and no `@`. The control row is what makes the shot prove something, so
losing it is as much a failure as losing the mark.

    before  every accent pill stands alone: two rows, one count each, no `@`
    after   the upper row has two pills (the `@` and its count) and the
            control row below it still has exactly one

It reads the pixels rather than the app's own state on purpose: the app
reporting that it drew the mark is what a lost frame would also do. Pills are
found by their accent fill, grouped into blobs, and blobs on a shared baseline
are grouped into a row.

Pure standard library (zlib only), so it runs on the capture Mac as-is.
"""

import argparse
import struct
import sys
import zlib

# `theme.accent` as the renderer fills a pill. Matched with tolerance so a
# colour-managed capture or a different surface does not break the check.
ACCENT = (16, 132, 254)
TOLERANCE = 46
# Smaller than any pill at 1x; large enough to reject antialiasing specks.
MIN_BLOB_PX = 40
# The sidebar, as a fraction of width. The transcript has its own accent
# furniture (the `@` jump capsule), which this check must not count.
SIDEBAR_FRACTION = 0.30


def read_png(path):
    """Decode a non-interlaced 8-bit RGB/RGBA PNG to (width, height, rows)."""
    with open(path, "rb") as fh:
        data = fh.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"{path}: not a PNG")
    pos, idat, width, height, channels = 8, bytearray(), 0, 0, 0
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        if kind == b"IHDR":
            width, height, depth, colour = struct.unpack(">IIBB", body[:10])
            if depth != 8 or colour not in (2, 6):
                raise SystemExit(f"{path}: need an 8-bit RGB/RGBA PNG")
            if body[12] != 0:
                raise SystemExit(f"{path}: interlaced PNGs are not supported")
            channels = 3 if colour == 2 else 4
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break
        pos += 12 + length

    raw = zlib.decompress(bytes(idat))
    stride = width * channels
    rows, prev, at = [], bytearray(stride), 0
    for _ in range(height):
        filt = raw[at]
        line = bytearray(raw[at + 1 : at + 1 + stride])
        at += 1 + stride
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = prev[i]
            c = prev[i - channels] if i >= channels else 0
            if filt == 1:
                line[i] = (line[i] + a) & 0xFF
            elif filt == 2:
                line[i] = (line[i] + b) & 0xFF
            elif filt == 3:
                line[i] = (line[i] + (a + b) // 2) & 0xFF
            elif filt == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 0xFF
            elif filt != 0:
                raise SystemExit(f"{path}: bad PNG filter {filt}")
        rows.append(bytes(line))
        prev = line
    return width, height, rows, channels


def accent_mask(width, height, rows, channels, max_x):
    hit = bytearray(width * height)
    for y in range(height):
        line = rows[y]
        base = y * width
        for x in range(max_x):
            i = x * channels
            if (
                abs(line[i] - ACCENT[0]) <= TOLERANCE
                and abs(line[i + 1] - ACCENT[1]) <= TOLERANCE
                and abs(line[i + 2] - ACCENT[2]) <= TOLERANCE
            ):
                hit[base + x] = 1
    return hit


def blobs(hit, width, height, max_x):
    """Connected runs of accent pixels, as (top, bottom, left, right, area)."""
    seen = bytearray(width * height)
    out = []
    for y in range(height):
        for x in range(max_x):
            start = y * width + x
            if not hit[start] or seen[start]:
                continue
            stack, area = [start], 0
            seen[start] = 1
            top = bottom = y
            left = right = x
            while stack:
                p = stack.pop()
                py, px_ = divmod(p, width)
                area += 1
                top, bottom = min(top, py), max(bottom, py)
                left, right = min(left, px_), max(right, px_)
                for q, ok in (
                    (p - 1, px_ > 0),
                    (p + 1, px_ + 1 < max_x),
                    (p - width, py > 0),
                    (p + width, py + 1 < height),
                ):
                    if ok and hit[q] and not seen[q]:
                        seen[q] = 1
                        stack.append(q)
            if area >= MIN_BLOB_PX:
                out.append((top, bottom, left, right, area))
    return out


def rows_of(found):
    """Group blobs that share a baseline into sidebar rows, top first."""
    grouped = []
    for blob in sorted(found, key=lambda b: b[0]):
        top, bottom = blob[0], blob[1]
        for row in grouped:
            if top <= row[-1][1] and bottom >= row[-1][0]:
                row.append(blob)
                break
        else:
            grouped.append([blob])
    for row in grouped:
        row.sort(key=lambda b: b[2])
    return grouped


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("shot")
    ap.add_argument("--expect", required=True, choices=("before", "after"))
    args = ap.parse_args()

    width, height, rows, channels = read_png(args.shot)
    max_x = max(1, int(width * SIDEBAR_FRACTION))
    found = blobs(accent_mask(width, height, rows, channels, max_x), width, height, max_x)
    marked = rows_of(found)

    print(f"{args.shot}: {width}x{height}, {len(found)} accent pills in {len(marked)} rows")
    for i, row in enumerate(marked):
        shape = ", ".join(f"{b[3] - b[2] + 1}x{b[1] - b[0] + 1}px" for b in row)
        print(f"  row {i}: {len(row)} pill(s) at y={row[0][0]} [{shape}]")

    problems = []
    counts = [len(r) for r in marked]
    if args.expect == "after":
        if 2 not in counts:
            problems.append("no row carries the @ beside its count")
        elif counts.index(2) + 1 > len(counts) - 1 or 1 not in counts[counts.index(2) + 1 :]:
            problems.append("no control row with a bare count below the mentioned row")
    else:
        if any(n > 1 for n in counts):
            problems.append("a row already carries two marks; this is not a before shot")
        if len(marked) < 2:
            problems.append(f"expected two counted rows in frame, found {len(marked)}")

    if problems:
        print(f"\nFAIL ({args.expect}): " + "; ".join(problems), file=sys.stderr)
        print("The frame is not evidence. Retake it.", file=sys.stderr)
        return 1
    print(f"\nOK: the frame shows the {args.expect} state.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

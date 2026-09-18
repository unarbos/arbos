#!/usr/bin/env python3
"""Compare a still of the app with a Cursor reference, in numbers.

    style-pair.py <arbos.png> <cursor-reference.jpg>

The style rows have always been read by eye, and eyes are the wrong
instrument for the two things that actually decide whether two lists look
like siblings: what the ground is, and how tall a row is.

Colour is only comparable within a theme. Cursor's references are light and
this app is dark on purpose (M-202: the desktop's palette.rs uses the same
literal #161514), so the ground is printed for the record and never
compared. Row pitch is comparable across themes, and is printed as a
percentage of screen height so two phones of different sizes can be held
against each other at all.
"""
import sys

from PIL import Image


def ground(img, frac):
    """The commonest colour across one horizontal line."""
    w, h = img.size
    row = [img.getpixel((x, int(h * frac))) for x in range(0, w, 4)]
    return max(set(row), key=row.count)


def row_pitch(path):
    """Median distance between the faint rules that divide list rows."""
    img = Image.open(path).convert("L")
    w, h = img.size
    x = int(w * 0.5)
    col = [img.getpixel((x, y)) for y in range(h)]
    hits = []
    # Only the middle of the screen: the top bar and the composer have edges
    # of their own that are not row rules.
    for y in range(int(h * 0.15) + 2, int(h * 0.95) - 2):
        here, above, below = col[y], sum(col[y - 4:y - 1]) / 3, sum(col[y + 2:y + 5]) / 3
        if abs(here - above) > 3 and abs(here - below) > 3 and abs(above - below) < 4:
            if not hits or y - hits[-1] > 20:
                hits.append(y)
    gaps = [b - a for a, b in zip(hits, hits[1:]) if b - a > h * 0.04]
    return h, len(hits), (sorted(gaps)[len(gaps) // 2] if gaps else None)


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    for name, path in (("arbos", sys.argv[1]), ("cursor", sys.argv[2])):
        img = Image.open(path).convert("RGB")
        h, rules, pitch = row_pitch(path)
        share = f"{pitch / h * 100:.1f}% of the screen" if pitch else "no usable pitch"
        print(f"{name:7} {img.size[0]}x{img.size[1]}  ground {ground(img, 0.45)}  "
              f"{rules} rules  row pitch {pitch or '-'}px = {share}")
    print()
    print("Ground is printed, not compared: the references are light and this app is")
    print("dark by decision (M-202). Row pitch is the comparable number.")


if __name__ == "__main__":
    main()

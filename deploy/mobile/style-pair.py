#!/usr/bin/env python3
"""Compare a still of the app with a Cursor reference, in numbers.

    style-pair.py <arbos.png> <cursor-reference.jpg>
    style-pair.py --chat <arbos.png> <cursor-reference.jpg>

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


def content_rows(img, thresh=25):
    """Which horizontal lines hold something other than the ground."""
    g = img.convert("L")
    w, h = g.size
    base = ground(img.convert("RGB"), 0.5)
    base = int(0.299 * base[0] + 0.587 * base[1] + 0.114 * base[2])
    rows = []
    for y in range(h):
        n = sum(1 for x in range(0, w, 3) if abs(g.getpixel((x, y)) - base) > thresh)
        rows.append(n >= 3)
    return rows


def bands(img):
    """Runs of lines that hold something, as (top, bottom) pairs."""
    rows = content_rows(img)
    out, start = [], None
    for y, filled in enumerate(rows):
        if filled and start is None:
            start = y
        elif not filled and start is not None:
            if y - start > 4:          # a stray line is not a band
                out.append((start, y))
            start = None
    if start is not None:
        out.append((start, len(rows)))
    return out


def left_margin(img, thresh=25):
    """The first column from the left that holds something, in pixels."""
    g = img.convert("L")
    w, h = g.size
    base = ground(img.convert("RGB"), 0.5)
    base = int(0.299 * base[0] + 0.587 * base[1] + 0.114 * base[2])
    # The body only. A header's back arrow sits further left than the text
    # and would answer for the whole screen.
    top, bottom = int(h * 0.25), int(h * 0.85)
    for x in range(w):
        n = sum(1 for y in range(top, bottom, 3) if abs(g.getpixel((x, y)) - base) > thresh)
        if n >= 3:
            return x
    return None


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


def chat_pair(a_path, b_path):
    """Pair two chat screens by their parts, since they have no row pitch."""
    for name, path in (("arbos", a_path), ("cursor", b_path)):
        img = Image.open(path).convert("RGB")
        w, h = img.size
        seen = bands(img)
        margin = left_margin(img)
        head = seen[0] if seen else None
        air = (seen[1][0] - head[1]) if head and len(seen) > 1 else None
        print(f"{name:7} {w}x{h}  ground {ground(img, 0.5)}")
        print(f"        text starts    {margin}px = {margin / w * 100:.1f}% of the width"
              if margin is not None else "        text starts    nowhere — no body content found")
        print(f"        header ends    {head[1]}px = {head[1] / h * 100:.1f}% down"
              if head else "        header ends    nowhere — no bands found")
        print(f"        air under it   {air}px = {air / h * 100:.1f}% of the height"
              if air is not None else "        air under it   cannot say — one band only")
    print()
    print("Ground is printed, not compared (M-202). The two comparable numbers are")
    print("where text starts from the left and how much air sits under the header:")
    print("both are proportions, so two phones of different sizes can be held")
    print("against each other.")


def self_test():
    """Prove the measurements can fail before trusting what they say."""
    img = Image.new("RGB", (1000, 2000), (255, 255, 255))
    for y in range(100, 160):                      # a header band
        for x in range(60, 940):
            img.putpixel((x, y), (0, 0, 0))
    for y in range(300, 800):                      # a body band, 20% in
        for x in range(200, 900):
            img.putpixel((x, y), (0, 0, 0))
    seen, margin = bands(img), left_margin(img)
    ok = (margin == 200 and len(seen) == 2
          and abs(seen[0][1] - 160) <= 2 and abs(seen[1][0] - 300) <= 2)
    print(f"self-test: margin {margin} (want 200), bands {seen} (want ~[(100,160),(300,800)])")
    print("self-test: " + ("the measurements read a known picture correctly"
                           if ok else "WRONG — do not trust the numbers below"))
    return ok


def main():
    if sys.argv[1:2] == ["--self-test"]:
        sys.exit(0 if self_test() else 1)
    if sys.argv[1:2] == ["--chat"]:
        if len(sys.argv) != 4:
            sys.exit(__doc__)
        if not self_test():
            sys.exit(1)
        print()
        return chat_pair(sys.argv[2], sys.argv[3])
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    rules_seen = []
    for name, path in (("arbos", sys.argv[1]), ("cursor", sys.argv[2])):
        img = Image.open(path).convert("RGB")
        h, rules, pitch = row_pitch(path)
        rules_seen.append(rules)
        share = f"{pitch / h * 100:.1f}% of the screen" if pitch else "no usable pitch"
        print(f"{name:7} {img.size[0]}x{img.size[1]}  ground {ground(img, 0.45)}  "
              f"{rules} rules  row pitch {pitch or '-'}px = {share}")
    print()
    print("Ground is printed, not compared: the references are light and this app is")
    print("dark by decision (M-202). Row pitch is the comparable number.")

    # Row pitch means something on a list, where rows are a repeating unit of
    # one height. A chat has no such unit: the "rules" it finds are paragraph
    # edges on one side and message bubbles on the other, and the numbers come
    # out wildly apart while saying nothing. Pointed at a chat this printed
    # 5.3% against 24.3% as though that were a style difference.
    a_rules, b_rules = rules_seen[0], rules_seen[1]
    if min(a_rules, b_rules) < 4 or max(a_rules, b_rules) > 3 * max(min(a_rules, b_rules), 1):
        print()
        print(f"CAUTION: {a_rules} rules against {b_rules}. These do not look like the same")
        print("kind of screen. Row pitch compares a repeating unit, which a list has and")
        print("a chat does not — on a chat these numbers measure paragraph gaps against")
        print("message bubbles and mean nothing. Pair chats by their parts, not their pitch.")


if __name__ == "__main__":
    main()

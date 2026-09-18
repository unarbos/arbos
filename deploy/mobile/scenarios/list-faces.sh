#!/bin/bash
# Every project's face, against the rule that chooses it.
#
#   list-faces.sh <cycle>
#
# The coverage row is "projects list — **faces**, rows, sections". Rows were
# measured at 112 and sections at 135; the faces never were, because the
# accessibility tree cannot see them — a row is `const, Idle` whatever
# colour its folder is. So this reads pixels, which is the one thing in this
# harness that has to.
#
# `ProjectIdentity.defaults` picks a colour by FNV-1a over a key, modulo an
# eight-colour palette, so the same project wears the same face on every
# device — the desktop hashes the same way.
#
# The first draft of this computed that rule here and reported **0 of 8**
# matching, which would have read as every project wearing a chosen face.
# It was the check that was wrong: the key is the *target* —
# `hub:<machine>/<project>` — and the list shows a machine for almost no
# row, so the key cannot be built from what is on screen. Zero of eight
# should have been the giveaway; with eight colours, chance alone gives
# about one.
#
# So the rule is not checked here. What is checked is what the screen can
# answer: the face holds across two cold launches, and how many distinct
# faces the list is wearing.
#
# Reading pixels is what `find_row.py` did when it identified rows by glyph
# colour and opened the wrong project twice (M-...; cycle 49). The
# difference is what the number is used for: that script *navigated* by
# colour, this one only reports it, and it takes the row's position from
# the tree rather than guessing it from the image.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/list-faces"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

look() { # <tag> — reach the list, record the rows and a still
  xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
  xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
  sleep 11
  reach_the_list "$UDID" || return 1
  ui dump | grep -E "Button +[A-Za-z.][A-Za-z0-9._-]*, " > "$OUT/$1-rows.txt"
  xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1
}

look first || exit 1
# Twice, cold, because "the same project gets the same face" is a claim
# about two occasions and cannot be read off one.
look second || exit 1

python3 - "$OUT" <<'PY'
import sys
from pathlib import Path
try:
    from PIL import Image
except ImportError:
    print("no PIL on this machine — cannot read the faces"); raise SystemExit(1)

OUT = Path(sys.argv[1])
PALETTE = [("blue", 0x4C8DFF), ("orange", 0xF08A3C), ("purple", 0x9B7BFF),
           ("red", 0xE5533D), ("green", 0x3DBD6E), ("teal", 0x2FB7B0),
           ("pink", 0xE0609E), ("yellow", 0xE0B23C)]

def fnv1a(s):
    h = 14695981039346656037
    for b in s.encode():
        h = ((h ^ b) * 1099511628211) & 0xFFFFFFFFFFFFFFFF
    return h

def expected(name):
    return PALETTE[fnv1a(name) % len(PALETTE)][0]

def rows(tag):
    out = []
    for line in (OUT / f"{tag}-rows.txt").read_text().splitlines():
        parts = line.split(None, 3)
        if len(parts) < 4:
            continue
        out.append((parts[3].split(",")[0].strip(), int(parts[1])))
    return out

def face(tag, y):
    """The glyph's colour: the most saturated pixel near the row's left edge.
    The row's y comes from the tree, so nothing here guesses where a row is —
    only what colour sits at a place the tree already named."""
    im = Image.open(OUT / f"{tag}.png").convert("RGB")
    scale = im.size[1] / 852          # points -> pixels, this device
    cx, cy = int(36 * scale), int(y * scale)
    box = im.crop((cx - 20, cy - 20, cx + 20, cy + 20))
    best, score = None, -1
    for px in box.getdata():
        s = max(px) - min(px)         # saturation, cheaply
        if s > score:
            best, score = px, s
    return best, score

def nearest(rgb):
    def d(h):
        c = ((h >> 16) & 255, (h >> 8) & 255, h & 255)
        return sum((a - b) ** 2 for a, b in zip(rgb, c))
    return min(PALETTE, key=lambda p: d(p[1]))[0]

first, second = rows("first"), rows("second")
common = [n for n, _ in first if n in dict(second)]
print(f"rows read: {len(first)} then {len(second)}, {len(common)} in both")
print()

moved, faint = [], 0
seen = {}
for name in common:
    rgb1, s1 = face("first", dict(first)[name])
    rgb2, s2 = face("second", dict(second)[name])
    if s1 < 30 or s2 < 30:
        faint += 1
        print(f"  {name:34} too faint to read a colour at the glyph")
        continue
    a, b = nearest(rgb1), nearest(rgb2)
    if a != b:
        moved.append(f"{name}: {a} then {b}")
    seen.setdefault(a, []).append(name)
    print(f"  {name:34} {a:7} {'held' if a == b else 'CHANGED to ' + b}")

print()
shared = {c: ns for c, ns in seen.items() if len(ns) > 1}
print(f"distinct faces: {len(seen)} across {len(common) - faint} rows"
      f" — {', '.join(sorted(seen))}")
for c, ns in sorted(shared.items()):
    print(f"  {c} is worn by {len(ns)}: {', '.join(ns)}")
print("the name-derived rule is not checked here: it hashes the target"
      " (hub:<machine>/<project>),")
print("and the list shows a machine for almost no row, so the key cannot be"
      " built from the screen.")
print()
if faint == len(common):
    print("VERDICT: none — no glyph was readable, so nothing was compared")
elif moved:
    print(f"VERDICT: {len(moved)} face(s) changed between two cold launches — "
          + "; ".join(moved) + ".")
    print("         A face is derived, not stored, and must not move.")
else:
    print(f"VERDICT: every face held across two cold launches"
          f" ({len(common) - faint} rows, {len(seen)} distinct colours)")
PY
echo "stills in $OUT"

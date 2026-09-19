#!/bin/bash
# COVERS: call — voice first, orb, colours
#
# Does the face under the orb name the project you called?
#
#   call-face.sh <cycle> [project] [project] ...
#
# The list draws each project a face of its own, hashed from its key. The
# call screen draws whatever identity the attached kernel sent, and one
# kernel serves several projects — so cycle 166 measured `phone` and `demo`
# with the identical glyph under the orb, 0xE5533D both, while the list tells
# them apart. `const`, whose kernel sent no identity, drew 0x4C8DFF, which is
# simply the first colour in the palette.
#
# This takes the colour of the glyph beside the project's name on each call
# screen and reports whether any two projects share one. It does not judge
# which behaviour is right — that is filed as a question
# (features-inbox/2026-09-19-one-project-two-faces-which-one-wins.md) — it
# measures whether the faces distinguish anything.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}; shift
PROJECTS=("$@"); [ ${#PROJECTS[@]} -eq 0 ] && PROJECTS=(phone demo const)
OUT="$HOME/mobile-out/$CYCLE/call-face"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

face() { # <png> — the most saturated pixel beside the project's name
  python3 - "$1" <<'PY'
import sys
from PIL import Image
im = Image.open(sys.argv[1]).convert("RGB").crop((440, 1580, 560, 1650))
best = max(list(im.getdata()), key=lambda p: max(p) - min(p))
print("0x%02X%02X%02X" % best)
PY
}

SEEN=$OUT/faces.txt; : > "$SEEN"
for p in "${PROJECTS[@]}"; do
  xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
  xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 10
  reach_the_list "$UDID" || exit 1
  ui tap "$p" >/dev/null 2>&1 || { echo "  no $p row"; continue; }
  sleep 5
  ui tap "More" >/dev/null 2>&1; sleep 2
  CALL=$(ui dump | grep -oE "Call $p\$" | head -1)
  [ -n "$CALL" ] || { echo "  the menu does not offer 'Call $p'"; continue; }
  ui tap "$CALL" >/dev/null 2>&1; sleep 8
  xcrun simctl io "$UDID" screenshot "$OUT/$p.png" >/dev/null 2>&1
  HEX=$(face "$OUT/$p.png")
  printf '  %-28s %s\n' "$p" "$HEX"
  echo "$HEX $p" >> "$SEEN"
  ui tap "End call" >/dev/null 2>&1; sleep 2
done

echo
COUNT=$(wc -l < "$SEEN" | tr -d ' ')
[ "$COUNT" -ge 2 ] || { echo "VERDICT: cannot say — only $COUNT call screen(s) reached"; exit 1; }
DISTINCT=$(awk '{print $1}' "$SEEN" | sort -u | wc -l | tr -d ' ')
SHARED=$(awk '{print $1}' "$SEEN" | sort | uniq -d | wc -l | tr -d ' ')
FIRST=$(grep -c "0x4C8DFF" "$SEEN" | tr -d ' ')
echo "$COUNT project(s), $DISTINCT distinct face(s) under the orb"
if [ "$SHARED" = 0 ] && [ "$FIRST" = 0 ]; then
  echo "VERDICT: every project called shows a face of its own"
else
  [ "$SHARED" != 0 ] && awk '{print $1}' "$SEEN" | sort | uniq -d | while read -r h; do
    echo "  $h is shown by: $(grep "^$h " "$SEEN" | awk '{print $2}' | tr '\n' ' ')"
  done
  [ "$FIRST" != 0 ] && echo "  $FIRST project(s) drew 0x4C8DFF, the palette's first colour — that is"
  [ "$FIRST" != 0 ] && echo "  what tint returns when it recognises no colour at all"
  echo "VERDICT: the face under the orb does not name the project — $DISTINCT face(s)"
  echo "         across $COUNT project(s). Filed as a question, not a fault:"
  echo "         features-inbox/2026-09-19-one-project-two-faces-which-one-wins.md"
fi
echo "stills in $OUT"

#!/bin/bash
# COVERS: the chrome — what each control is called
#
# Does the app's text follow the system size?
#
#   text-size.sh <cycle> [project]
#
# Nobody had asked until cycle 191. The answer is no: every font comes from
# `Font.system(size:)`, which is a fixed size, where SwiftUI's text *styles*
# are the ones that scale.
#
# This reports rather than judges. Whether the phone should follow the system
# setting is a product decision — the app's geometry is deliberately tight,
# the list's row pitch sits within half a point of Cursor's, and Dynamic Type
# would move that — so it is filed as a question:
# features-inbox/2026-09-19-the-text-does-not-grow-with-the-system-setting.md
#
# It puts the size back to `large` on the way out, whatever happens. Leaving
# a simulator at an accessibility size would quietly change what every later
# scenario measures, which is the same class of fault as cycle 173's stale
# app: a run that poisons the runs after it.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/text-size"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

restore() { xcrun simctl ui "$UDID" content_size large >/dev/null 2>&1; }
trap restore EXIT

reading() {  # <name> <size>
  xcrun simctl ui "$UDID" content_size "$2" >/dev/null 2>&1
  xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
  xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 11
  reach_the_list "$UDID" >/dev/null 2>&1 || { echo "  $1: no list"; return 1; }
  xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1
  printf '  %-34s rows %-4s ellipses %-4s\n' "$2" \
    "$(ui dump | grep -cE 'Button +[a-z0-9-]+, ')" \
    "$(ui dump | grep -c '…')"
}

echo "the list, at two system text sizes:"
reading small large || exit 1
reading big accessibility-extra-extra-extra-large || exit 1

SAME=$(python3 - "$OUT/small.png" "$OUT/big.png" <<'PY'
import sys
from PIL import Image
a = Image.open(sys.argv[1]).convert("RGB")
b = Image.open(sys.argv[2]).convert("RGB")
if a.size != b.size:
    print("different"); raise SystemExit
pa, pb = a.load(), b.load()
w, h = a.size
diff = sum(1 for y in range(0, h, 7) for x in range(0, w, 7) if pa[x, y] != pb[x, y])
total = len(range(0, h, 7)) * len(range(0, w, 7))
print(f"{diff} of {total}")
PY
)
echo "  pixels differing between the two: $SAME"
echo
case "$SAME" in
  0\ of\ *|1[0-9]\ of\ *|[1-9]\ of\ *)
    echo "VERDICT: the text does not follow the system size — the two screens are"
    echo "         the same picture. Every font is Font.system(size:), which is"
    echo "         fixed; SwiftUI's text styles are the ones that scale."
    echo "         Filed as a question, not a fault:"
    echo "         features-inbox/2026-09-19-the-text-does-not-grow-with-the-system-setting.md";;
  different)
    echo "VERDICT: cannot say — the two screenshots are different shapes";;
  *)
    echo "VERDICT: the text does follow the system size, and the two screens differ."
    echo "         If that is new, the checks built on this app's fixed geometry —"
    echo "         the style pair's row pitch above all — need re-reading.";;
esac
echo "stills in $OUT"

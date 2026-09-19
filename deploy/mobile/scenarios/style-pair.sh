#!/bin/bash
# COVERS: style pair vs Cursor stills
#
# Hold this app's two main surfaces against Cursor's, in numbers.
#
#   style-pair.sh <cycle> [project]
#
# `style-pair.py` does the measuring and has since cycle 83. What it never
# had was anything to hand it the pictures: every run of this row has been a
# person taking two screenshots and typing two commands, which is why the row
# aged eleven cycles between readings and why cycle 152 spent a cycle
# discovering the tool could not even run on the Mac.
#
# Two numbers are compared, one per surface, and both are proportions so
# phones of different sizes can be held against each other:
#
#   the list   row pitch, as a share of screen height. A list is a repeating
#              unit and its rhythm is the thing you see first.
#   the chat   where text starts from the left. A chat has no repeating unit
#              (the tool says so and refuses to pitch it), and the left
#              margin is the leftmost ink, robust to what the ink is.
#
# Ground is printed and never compared: the references are light and this app
# is dark by decision, both sides (M-202).
#
# A point of difference is the threshold. Cycle 83 measured the list at 8.7%
# against 8.3% and cycle 152 the chat at 5.3% against 4.9% — both under half a
# point, twice, sixty-nine cycles apart. A whole point is comfortably outside
# that and still far tighter than anything an eye would call a difference.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/style-pair"; mkdir -p "$OUT"
REF=${CURSOR_REFERENCE:-$HOME/mobile-docs/cursor-reference}
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

for f in 01-chat-view.jpg 02-all-agents-list.jpg; do
  [ -f "$REF/$f" ] || { echo "no reference still at $REF/$f."
                        echo "They are not in the checkout; they live beside the mirrored"
                        echo "ledgers. Set CURSOR_REFERENCE if they are somewhere else."
                        exit 1; }
done

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 11
reach_the_list "$UDID" || exit 1
xcrun simctl io "$UDID" screenshot "$OUT/list.png" >/dev/null 2>&1
ROWS=$(ui dump | grep -cE "Button +[a-z0-9-]+, ")
echo "the list:  $ROWS row(s)"
ui tap "$ROW" >/dev/null 2>&1 || { echo "  no $ROW row"; exit 1; }
sleep 5
xcrun simctl io "$UDID" screenshot "$OUT/chat.png" >/dev/null 2>&1
echo "the chat:  $(ui dump | grep -cE "StaticText") line(s)"

# A list with one row has no pitch to speak of, and comparing it would be a
# number about nothing.
[ "$ROWS" -ge 3 ] || { echo
                       echo "VERDICT: cannot say — $ROWS row(s) on the list, and row pitch needs"
                       echo "         a few rows to be a rhythm rather than a gap"
                       exit 1; }

echo
python3 "$HERE/../style-pair.py" "$OUT/list.png" "$REF/02-all-agents-list.jpg" > "$OUT/list.txt" 2>&1
python3 "$HERE/../style-pair.py" --chat "$OUT/chat.png" "$REF/01-chat-view.jpg" > "$OUT/chat.txt" 2>&1
grep -E "^(arbos|cursor)" "$OUT/list.txt" | sed 's/^/  /'
echo
grep -E "^(arbos|cursor)|text starts" "$OUT/chat.txt" | sed 's/^/  /'

pcts() { grep -oE "= *[0-9]+\.[0-9]+%" "$1" | grep -oE "[0-9]+\.[0-9]+" | tr '\n' ' '; }
LIST=($(grep -oE "[0-9]+\.[0-9]+% of the screen" "$OUT/list.txt" | grep -oE "^[0-9]+\.[0-9]+"))
CHAT=($(grep -oE "= [0-9]+\.[0-9]+% of the width" "$OUT/chat.txt" | grep -oE "[0-9]+\.[0-9]+"))

gap() { python3 -c "print(f'{abs($1 - $2):.1f}')"; }
within() { python3 -c "print(1 if abs($1 - $2) <= 1.0 else 0)"; }

echo
FAULTS=0
if [ "${#LIST[@]}" = 2 ]; then
  G=$(gap "${LIST[0]}" "${LIST[1]}")
  echo "  list row pitch:   ${LIST[0]}% against ${LIST[1]}%  — ${G} apart"
  [ "$(within "${LIST[0]}" "${LIST[1]}")" = 1 ] || FAULTS=$((FAULTS + 1))
else
  echo "  list row pitch:   not read from both stills — see $OUT/list.txt"
  FAULTS=$((FAULTS + 1))
fi
if [ "${#CHAT[@]}" = 2 ]; then
  G=$(gap "${CHAT[0]}" "${CHAT[1]}")
  echo "  chat left margin: ${CHAT[0]}% against ${CHAT[1]}%  — ${G} apart"
  [ "$(within "${CHAT[0]}" "${CHAT[1]}")" = 1 ] || FAULTS=$((FAULTS + 1))
else
  echo "  chat left margin: not read from both stills — see $OUT/chat.txt"
  FAULTS=$((FAULTS + 1))
fi

echo
if [ "$FAULTS" = 0 ]; then
  echo "VERDICT: both surfaces sit within a point of Cursor's — the list's rhythm"
  echo "         and the chat's left margin"
else
  echo "VERDICT: $FAULTS surface(s) are more than a point apart, or could not be read."
  echo "         Read the numbers above before calling it a drift: a still of the"
  echo "         wrong screen measures perfectly and means nothing."
fi
echo "stills in $OUT"

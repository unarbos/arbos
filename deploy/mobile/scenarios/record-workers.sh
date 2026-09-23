#!/bin/bash
# A recording of workers being asked for, appearing, and being opened.
#
#   record-workers.sh <cycle>
#
# The standing order asks for a recording every third cycle, and the last one
# was cycle 130. Recordings exist to catch what the accessibility tree cannot:
# the tree carries a label's whole string whether or not the screen shows it,
# so clipping, overlap and truncation are invisible to every other check here.
#
# Cycle 155 wrote this after a hand-rolled recording produced a false alarm.
# It scrolled the workers sheet once, the workers it had just started were not
# on that screen, and the review of the footage reported them missing from the
# app. They were six pages down. A recording that scrolls less than a check
# does will accuse the app of whatever it failed to reach, so this one pages
# to the end the same way the checks do, and says what it found before the
# footage is watched.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/recording"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 10
reach_the_list "$UDID" || exit 1

xcrun simctl io "$UDID" recordVideo --codec h264 --force "$OUT/workers.mp4" >/dev/null 2>&1 &
REC=$!
sleep 2

ui tap "$ROW" >/dev/null 2>&1 || { kill $REC; echo "no $ROW row"; exit 1; }
sleep 5

TAG=w$(date -u +%H%M%S)
echo "asking for two workers tagged $TAG"
# No quotation marks and no dashes: iOS turns a typed " into a curly quote and
# the read-back never matches (M-185). type_line refuses anything non-ASCII,
# which idb drops silently (M-464).
type_line "$UDID" "Start two workers at once, each waiting for neither the other. Their goals are exactly $TAG alpha and $TAG beta. Each says one short sentence." || { kill $REC; exit 1; }
ui tap "Send" >/dev/null 2>&1

for _ in $(seq 1 40); do
  ui dump | grep -qE "Working $TAG" && break
  sleep 3
done
echo "  in the chat: $(ui dump | grep -cE "Working $TAG") running line(s)"

PILL=$(ui dump | grep -E "Button +([^,]+, )?(Agents|Working) [0-9]+" | head -1 | awk '{print $1, $2}')
[ -n "$PILL" ] && idb ui tap $PILL --udid "$UDID"
sleep 4

# Page the whole sheet, on camera. The newest workers are at its end.
LABELS=$OUT/rows.txt
HOW=$(collect_rows "$UDID" 'Button +.+, ' "$LABELS")
MINE=$(grep -c "$TAG" "$LABELS")
echo "  in the sheet: $MINE of 2, paging $HOW over $(wc -l < "$LABELS" | tr -d ' ') rows"

ROWNAME=$(grep "$TAG" "$LABELS" | head -1)
if [ -n "$ROWNAME" ]; then
  echo "  opening $ROWNAME"
  ui tap "$ROWNAME" >/dev/null 2>&1
  sleep 5
  ui tap "Back" >/dev/null 2>&1; sleep 3
fi
ui tap "Back" >/dev/null 2>&1; sleep 3

sleep 2
kill -INT $REC 2>/dev/null; wait $REC 2>/dev/null
sleep 3
SIZE=$(stat -f %z "$OUT/workers.mp4" 2>/dev/null || echo 0)
echo
if [ "$MINE" = 2 ] && [ "$SIZE" -gt 100000 ]; then
  echo "VERDICT: two workers asked for, two running in the chat, two in the sheet,"
  echo "         and $SIZE bytes of footage in $OUT/workers.mp4"
else
  echo "VERDICT: $MINE of 2 reached the sheet and the footage is $SIZE bytes."
  echo "         Read the numbers above before reading the picture — a recording"
  echo "         that did not reach something will look like an app that lost it."
fi

#!/bin/bash
# Two counts of the same thing, from the same app, at the same moment.
#
#   pill-count-vs-sheet.sh <cycle> [project]
#
# The chat's pill says how many agents a project has. The sheet it opens
# lists them. At cycle 74 the pill read `Agents 19` and the sheet, paged to
# its end, held 12 rows — so one of the two is wrong, and a worker that has
# only just started is a plausible one to be missing (M-272, M-275).
#
# This exists so the question can be re-asked in one command rather than
# reconstructed. It diagnoses nothing; it puts the two numbers side by side.
#
# Two traps it avoids, both of which this loop has already fallen into:
#
#   * the sheet scrolls, and only rendered rows reach the tree, so the rows
#     are collected by paging to the end rather than read off one screen;
#   * two workers whose goal text truncates to the same string would collapse
#     in a naive unique count, so duplicate labels are reported rather than
#     silently folded away.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/pill-vs-sheet"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 9
ui tap "$ROW" >/dev/null || { echo "no $ROW row"; exit 1; }
sleep 5

PILL=$(ui dump | grep -oE "(Agents|Working) [0-9]+" | head -1)
COUNT=$(echo "$PILL" | grep -oE "[0-9]+")
[ -n "$COUNT" ] || { echo "no pill in this chat — nothing to compare"; exit 1; }
echo "the pill says:  $PILL"

# Tapped by frame: the pill's label is one the sheet's rows carry too.
AT=$(ui dump | grep -E "Button +(Agents|Working) [0-9]+" | head -1 | awk '{print $1, $2}')
idb ui tap $AT --udid "$UDID"; sleep 3
xcrun simctl io "$UDID" screenshot "$OUT/01-sheet.png" >/dev/null 2>&1

RAW=$OUT/rows-raw.txt; : > "$RAW"
for _ in $(seq 1 10); do
  ui dump | grep -E "Button +.+, " >> "$RAW"
  idb ui swipe 236 900 236 540 --duration 0.4 --udid "$UDID" >/dev/null 2>&1
  sleep 1.2
done
LABELS=$OUT/rows.txt
awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }' "$RAW" | sort -u > "$LABELS"
ROWS=$(wc -l < "$LABELS" | tr -d ' ')
echo "the sheet lists: $ROWS rows, paged to the end"

DUPES=$(awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }' "$RAW" | sort | uniq -d | wc -l | tr -d ' ')
echo "labels that appear more than once (would hide a row): $DUPES"

if [ "$ROWS" = "$COUNT" ]; then
  echo "VERDICT: the two agree on $COUNT."
else
  echo "VERDICT: they disagree — pill $COUNT, sheet $ROWS. One of the app's own two counts is wrong."
  echo "         Rows are in $LABELS if you want to see which are present."
fi

#!/bin/bash
# Several workers at once, and what the chat and the sheet say about them.
#
#   several-workers.sh <cycle> [project]
#
# Cycle 37 established the shape: four workers asked for together, the Agents
# pill climbing, a Done line each in the transcript, and the sheet listing
# them all by name — kept across going back and reopening. This re-measures
# it, and counts rather than describes.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"   # page_up: scroll in points
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/workers"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
pill() { ui dump | grep -oE "Agents [0-9]+|Working [0-9]+" | tr '\n' ' '; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 8
ui tap "$ROW" >/dev/null || { echo "no $ROW row"; exit 1; }
sleep 4
echo "pill before: $(pill)"
shot 01-before

LINE="Start four workers at once, each doing one trivial thing and waiting for none of the others: worker one says a sentence about rivers, worker two about mountains, worker three about the sea, worker four about the sky. Tell me when all four are done."
ui focus >/dev/null || { echo "no composer"; exit 1; }
sleep 0.7
idb ui text "$LINE" --udid "$UDID"
for _ in $(seq 1 80); do [ "$(ui field plain 2>/dev/null)" = "$LINE" ] && break; sleep 0.25; done
ui tap "Send" >/dev/null || { echo "no send button"; exit 1; }

echo "watching the pill for two minutes"
HIGH=0
for t in $(seq 1 40); do
  sleep 3
  n=$(ui dump | grep -oE "Agents [0-9]+" | grep -oE "[0-9]+" | tail -1)
  [ -n "${n:-}" ] && [ "$n" -gt "$HIGH" ] && { HIGH=$n; echo "  $((t*3))s: Agents $n"; }
  ui dump | grep -qE "Worked [0-9]" && [ "$t" -gt 8 ] && break
done
sleep 4; shot 02-after-the-work
echo "pill after: $(pill)   highest seen: $HIGH"
echo "Done lines in the transcript: $(ui dump | grep -cE 'Done [a-z]')"

echo
echo "== the sheet =="
ui tap "Agents $HIGH" >/dev/null 2>&1 || ui tap "Agents" >/dev/null 2>&1 || echo "  could not open the sheet"
sleep 2; shot 03-workers-sheet
# The labels read "<goal>, Done" — the state is last, with no space after
# it, so a grep for "Done " counts none of them and reports an empty sheet
# over a full one.
# Paged to the end, not counted off one screen. The sheet scrolls, only
# rendered rows reach the tree, and a single dump gave 12 while the pill
# said 22 — the fault M-287 withdrew a finding over. That fix went into the
# two scenarios written the same hour and not into this one, which had it
# already.
SHEET=$OUT/sheet-rows.txt
HOW=$(collect_rows "$UDID" ', (Done|Working)$' "$SHEET")
echo "  rows in the sheet: $(wc -l < "$SHEET" | tr -d ' ')  (paging $HOW)"
ui dump | grep -E ', (Done|Working)$' | head -8 | sed 's/^/    /'

echo
echo "== back and reopen =="
ui tap "End call" >/dev/null 2>&1 || idb ui swipe 196 300 196 800 --duration 0.3 --udid "$UDID"
sleep 2
ui tap "Back" >/dev/null 2>&1; sleep 2
ui tap "$ROW" >/dev/null; sleep 4
echo "  pill after reopening: $(pill)"
shot 04-reopened
echo "stills in $OUT"

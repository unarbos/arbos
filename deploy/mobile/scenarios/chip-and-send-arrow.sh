#!/bin/bash
# COVERS: attachments (`+`), photos, files
# The attachment chip, its ×, and the mic that becomes a send arrow.
#
#   chip-and-send-arrow.sh <cycle> [project]
#
# Cycle 72's journey re-exercised the *central* claim of the attachments row
# — a photo reaching the model — and nothing else, so the row was credited
# narrowly and the chip's ×, and the mic→send swap, kept their reading from
# cycle 60. This is that remainder.
#
# Three things, all read off the tree rather than looked at:
#
#   1. with nothing to send, the bar ends in the microphone;
#   2. attaching a photo puts a chip in the bar, and the microphone is
#      replaced by the send arrow;
#   3. the chip's × removes it, and the microphone comes back.
#
# The third is the one that has never been driven. Until this cycle the ×
# had no label and read as "Close" — the SF Symbol's name, and the same word
# the call's end button answered to — so there was nothing dependable to tap.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/chip"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }
# What the right-hand end of the composer offers.
tail_button() { ui dump | grep -E "Button +(Microphone|Send|Stop)$" | tail -1 | awk '{print $4}'; }
chips() { ui dump | grep -cE "Button +Remove "; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 9
reach_the_list "$UDID" || exit 1
ui tap "$ROW" >/dev/null || { echo "no $ROW row"; exit 1; }
sleep 4
shot 01-nothing-to-send
echo "1. empty composer"
echo "   chips: $(chips)   the bar ends in: $(tail_button)"

echo
echo "2. attach a photo"
ui tap "Add" >/dev/null 2>&1 || { echo "   no + button"; exit 1; }
sleep 2
ui tap "Photo Library" >/dev/null 2>&1 || { echo "   no Photo Library in the menu"; exit 1; }
sleep 4
# The picker is another process, so nothing in it is in the app's tree and
# it has to be driven by coordinates — the sequence photo-reaches-the-model
# settled at cycle 56, reused rather than reinvented.
tap_shot 78 470 "$UDID"; sleep 1      # the magenta flowers, top-left of the grid
tap_shot 426 157 "$UDID"; sleep 3     # the tick, which closes the picker
ui field >/dev/null 2>&1 || { echo "   the picker would not close — nothing was attached"; exit 1; }
sleep 2
shot 02-chip-in-the-bar
N=$(chips)
echo "   chips: $N   the bar ends in: $(tail_button)"
ui dump | grep -E "Button +Remove " | sed 's/^/     /'
[ "$N" -gt 0 ] || { echo "   no chip arrived — the rest cannot be tested"; exit 1; }

echo
echo "3. take it off again"
NAME=$(ui dump | grep -E "Button +Remove " | head -1 | awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }')
echo "   tapping: $NAME"
ui tap "$NAME" >/dev/null 2>&1 || echo "   could not tap it"
sleep 3
shot 03-chip-removed
AFTER=$(chips); ENDS=$(tail_button)
echo "   chips: $AFTER   the bar ends in: $ENDS"

echo
if [ "$AFTER" = 0 ] && [ "$ENDS" = "Microphone" ]; then
  echo "VERDICT: the chip goes, and the microphone comes back with it"
elif [ "$AFTER" = 0 ]; then
  echo "VERDICT: the chip goes, but the bar ends in $ENDS rather than the microphone"
else
  echo "VERDICT: $AFTER chip(s) still in the bar after tapping its ×"
fi
echo "stills in $OUT"

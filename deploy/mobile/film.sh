#!/bin/bash
# COVERS: recording
# Run a scenario with the camera on.
#
#   film.sh <cycle> <scenario.sh> [args…]
#
# The standing order asks for a recording every third cycle, and most
# scenarios do not record themselves — only the one written for it does. So
# every filming cycle has wrapped a scenario by hand: start recordVideo in
# the background, run the thing, kill the recorder, hope the kill landed.
# Cycles 178, 181, 187 and 190 each typed that out again.
#
# It is three lines and they are easy to get subtly wrong. An unkilled
# recorder from an interrupted run holds the file open and the next one
# writes nothing — which is how a cycle-46 run produced no video at all and
# nobody noticed until the review asked where it was. So this kills any
# recorder still going before it starts, and says how large the file came out
# rather than leaving that to be discovered later.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; shift
WHAT=${1:?a scenario to run}; shift
[ -f "$HERE/scenarios/$WHAT" ] || [ -f "$HERE/$WHAT" ] || { echo "no scenario called $WHAT"; exit 1; }
RUN=$HERE/scenarios/$WHAT; [ -f "$RUN" ] || RUN=$HERE/$WHAT

OUT="$HOME/mobile-out/$CYCLE/film"; mkdir -p "$OUT"
NAME=$(basename "$WHAT" .sh)
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')

# Any recorder left behind by an interrupted run.
pkill -INT -f "simctl io.*recordVideo" 2>/dev/null && { echo "stopped a recorder left running"; sleep 2; }

xcrun simctl io "$UDID" recordVideo --codec h264 --force "$OUT/$NAME.mp4" >/dev/null 2>&1 &
REC=$!
sleep 2

bash "$RUN" "$CYCLE" "$@"
STATUS=$?

sleep 2
kill -INT $REC 2>/dev/null
wait $REC 2>/dev/null
sleep 3

SIZE=$(wc -c < "$OUT/$NAME.mp4" 2>/dev/null | tr -d ' ')
echo
if [ "${SIZE:-0}" -lt 100000 ]; then
  echo "the film is ${SIZE:-0} bytes, which is not a film. A recorder left over from"
  echo "an earlier run holds the file open and this one writes nothing."
else
  echo "filmed: $OUT/$NAME.mp4  ($SIZE bytes)"
  echo "a review copy: $HERE/review-demo.sh $OUT/$NAME.mp4"
fi
exit $STATUS

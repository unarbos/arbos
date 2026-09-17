#!/bin/bash
# The cycle-1 flow on the simulator: open a project, send a message, watch
# the worker line and the reply. Stills to ~/mobile-out/<cycle>/flow-*.png.
#   mac-flow.sh <cycle> <row-y> "<message>" [record]
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
CYCLE=${1:?cycle}; ROWY=${2:?row y in points}; MSG=${3:?message}; RECORD=${4:-}
OUT="$HOME/mobile-out/$CYCLE"; mkdir -p "$OUT"
UDID=$(cut -d' ' -f2 "$OUT/sim.txt")
BUNDLE=com.unarbos.arbos.ios
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "shot $1"; }
idb connect "$UDID" >/dev/null 2>&1
xcrun simctl terminate "$UDID" $BUNDLE 2>/dev/null || true
xcrun simctl launch --console-pty "$UDID" $BUNDLE > "$OUT/flow-console.log" 2>&1 &
sleep 5
if [ -n "$RECORD" ]; then
  xcrun simctl io "$UDID" recordVideo --codec h264 --force "$OUT/flow.mp4" >/dev/null 2>&1 &
  RECPID=$!
  sleep 1
fi
shot flow-01-projects
idb ui tap 120 "$ROWY" --udid "$UDID"
sleep 5
shot flow-02-chat-open
# The composer sits at the bottom: tap the field, type, send with Enter.
idb ui tap 200 788 --udid "$UDID"
sleep 1.5
idb ui text "$MSG" --udid "$UDID"; sleep 1
sleep 1
idb ui key 40 --udid "$UDID"   # Return (HID keycode 40) → submitLabel .send
sleep 3
shot flow-03-sent
for t in 8 16 26 40 60; do
  sleep $(( t - ${prev:-3} )); prev=$t
  shot "flow-04-t$t"
done
if [ -n "$RECORD" ]; then kill -INT $RECPID 2>/dev/null; sleep 3; fi
grep -v "^$" "$OUT/flow-console.log" | tail -12

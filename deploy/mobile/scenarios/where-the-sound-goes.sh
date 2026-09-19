#!/bin/bash
# COVERS: call — AirPods / speaker route, screen off, CallKit
#
# Does the app know where the sound is going, and does it ever say?
#
#   where-the-sound-goes.sh <cycle>
#
# This row has had nothing covering it for the whole life of the loop —
# coverage-map has printed it under "the genuinely untested ones" every run.
# Part of it cannot be tested here and saying so is the point: AirPods need a
# real device with a real pair, and a simulator has no screen to switch off.
# What *can* be established on this machine is what the app knows and what it
# shows, and those turn out to be different things.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/sound"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
CLIPS="$HOME/mobile-clips"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

[ -f "$CLIPS/ask.wav" ] || { echo "no ask.wav in $CLIPS — run mac-voice.sh once to make the clips"; exit 1; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch --console-pty "$UDID" $B -previewCall 1 -injectWav "$CLIPS/ask.wav" \
  > "$OUT/console.log" 2>&1 &
sleep 14
xcrun simctl io "$UDID" screenshot "$OUT/01-in-a-call.png" >/dev/null 2>&1

echo "== what the app knows =="
ROUTE=$(grep -oE "route=[a-zA-Z]+" "$OUT/console.log" | head -1 | cut -d= -f2)
TRACE=$(grep -oE "route change outputs=\[[^]]*\] volume=[0-9.]+" "$OUT/console.log" | head -1)
echo "  the connect metric reports: route=${ROUTE:-nothing}"
echo "  route-change traces:        ${TRACE:-none this run}"

echo
echo "== what the screen says =="
# Everything on the call screen, labels and values both. If the route were
# shown anywhere it would be here.
SEEN=$(ui values 2>/dev/null)
echo "$SEEN" > "$OUT/call-screen.txt"
SHOWN=$(echo "$SEEN" | grep -icE "speaker|receiver|airpod|bluetooth|headphone|[0-9]+%" | tr -d ' ')
echo "  elements naming a route or a volume: $SHOWN"
echo "$SEEN" | awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }' | grep -vE "^ *$" | head -5 | sed 's/^      /    /'

echo
echo "== CallKit =="
# Asked of the source, because the phone cannot show what was never built.
KIT=$(grep -rl "import CallKit" "$HERE/../../../ios" 2>/dev/null | wc -l | tr -d ' ')
echo "  files importing CallKit: $KIT"

echo
if [ -z "$ROUTE" ]; then
  echo "VERDICT: cannot say — the call never reported a route, so this run"
  echo "         measured neither what the app knows nor what it shows."
elif [ "$SHOWN" = 0 ]; then
  echo "VERDICT: the app knows the route and never says so. The connect metric"
  echo "         carries route=$ROUTE, and nothing on the call screen names a"
  echo "         route or a volume. CallKit is imported by $KIT file(s)."
  echo "         AirPods and a dark screen need a real device; this machine"
  echo "         can establish the first half and does."
else
  echo "VERDICT: the route reaches the screen — $SHOWN element(s) name one,"
  echo "         with the connect metric reporting route=$ROUTE."
fi
echo "still and screen text in $OUT"
xcrun simctl terminate "$UDID" $B 2>/dev/null

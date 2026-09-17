#!/bin/bash
# Cycle-3 call flow: voice-first, then pull down, type, and a recording.
#   mac-call.sh <cycle>
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/call"; mkdir -p "$OUT"
CLIPS="$HOME/mobile-clips"
UDID=$(cut -d' ' -f2 "$HOME/mobile-out/$CYCLE/sim.txt")
BUNDLE=com.unarbos.arbos.ios
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "shot $1"; }
idb connect "$UDID" >/dev/null 2>&1
xcrun simctl terminate "$UDID" $BUNDLE 2>/dev/null || true
xcrun simctl launch --console-pty "$UDID" $BUNDLE -previewCall 1 -injectWav "$CLIPS/ask.wav" > "$OUT/console.log" 2>&1 &
sleep 1
xcrun simctl io "$UDID" recordVideo --codec h264 --force "$OUT/call.mp4" >/dev/null 2>&1 &
RECPID=$!
sleep 2.5; shot voice-first-listening
sleep 5; shot voice-first-speaking
sleep 3
idb ui swipe 196 300 196 560 --duration 0.35 --udid "$UDID"
sleep 1.5; shot pulled-down
idb ui tap 150 772 --udid "$UDID"; sleep 1.2
idb ui text "Say one sentence about the sea." --udid "$UDID"; sleep 0.8
shot pulled-down-typing
idb ui key 40 --udid "$UDID"; sleep 6
shot pulled-down-reply
idb ui tap 284 772 --udid "$UDID"; sleep 1; shot pulled-down-muted
sleep 2
kill -INT $RECPID 2>/dev/null; sleep 3
ffmpeg -v error -y -i "$OUT/call.mp4" -vf "scale=786:-2,fps=30" -c:v libx264 -crf 24 -preset veryfast -pix_fmt yuv420p -an "$OUT/call-pulldown.mp4"
grep -E "^metric|transcript:" "$OUT/console.log" | head -8

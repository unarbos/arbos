#!/bin/bash
# Cycle 6: dictation into the composer (injected clip) and the + menu with
# a photo from the simulator's library, sent with a line. Recorded.
#   mac-attach.sh <cycle>
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE"; mkdir -p "$OUT"
UDID=$(cut -d' ' -f2 "$OUT/sim.txt")
BUNDLE=com.unarbos.arbos.ios
CLIPS="$HOME/mobile-clips"
[ -f "$CLIPS/note.wav" ] || { say -v Samantha -o "$CLIPS/note.aiff" "Please summarise what the workers did today in two sentences." && ffmpeg -loglevel error -y -i "$CLIPS/note.aiff" -ar 24000 -ac 1 -sample_fmt s16 "$CLIPS/note.wav"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
idb connect "$UDID" >/dev/null 2>&1
xcrun simctl privacy "$UDID" grant photos $BUNDLE >/dev/null 2>&1
xcrun simctl terminate "$UDID" $BUNDLE 2>/dev/null || true
xcrun simctl launch --console-pty "$UDID" $BUNDLE -dictateWav "$CLIPS/note.wav" > "$OUT/attach-console.log" 2>&1 &
sleep 5
xcrun simctl io "$UDID" recordVideo --codec h264 --force "$OUT/attach.mp4" >/dev/null 2>&1 &
RECPID=$!
sleep 1
idb ui tap 120 291 --udid "$UDID"; sleep 6; shot a-01-chat
# dictation: tap the mic at the right of the composer
idb ui tap 355 788 --udid "$UDID"; sleep 3; shot a-02-dictating
sleep 6; shot a-03-dictated
# stop (the same spot is now the stop disc)
idb ui tap 355 788 --udid "$UDID"; sleep 1.5; shot a-04-words-in-field
# + → Photo Library
idb ui tap 41 788 --udid "$UDID"; sleep 1.5; shot a-05-plus-menu
idb ui tap 160 767 --udid "$UDID"; sleep 4; shot a-06-photo-picker
# pick the first photo in the grid, then Add
idb ui tap 70 230 --udid "$UDID"; sleep 1; shot a-07-picked
idb ui tap 360 62 --udid "$UDID"; sleep 3; shot a-08-chip
# send
idb ui tap 355 788 --udid "$UDID"; sleep 8; shot a-09-sent
sleep 20; shot a-10-reply
kill -INT $RECPID 2>/dev/null; sleep 3
ffmpeg -v error -y -i "$OUT/attach.mp4" -vf "scale=786:-2,fps=30" -c:v libx264 -crf 24 -preset veryfast -pix_fmt yuv420p -an "$OUT/attach-dictate-photo-send.mp4"
grep -E "error|put|unknown|attachments|transcript" "$OUT/attach-console.log" | head -8

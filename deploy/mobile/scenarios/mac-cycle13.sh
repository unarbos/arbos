#!/bin/bash
# Cycle 13: stream-while-scrolled-up (F12 check, recorded), workers pill → sheet → worker chat, background 8 s, cold start.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
U=B1185668-7488-420F-B12D-4412BAAC7673; O=$HOME/mobile-out/cycle-13; B=com.unarbos.arbos.ios; mkdir -p $O
shot() { xcrun simctl io "$U" screenshot "$O/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
row() { xcrun simctl io "$U" screenshot /tmp/list.png >/dev/null 2>&1; python3 ~/find_row.py /tmp/list.png "$1"; }
T=$(plutil -extract hubToken raw -o - ~/arbos/ios/Arbos/Secrets.plist); H=$(plutil -extract hubURL raw -o - ~/arbos/ios/Arbos/Secrets.plist)
xcrun simctl terminate $U $B 2>/dev/null; sleep 2
xcrun simctl launch --console-pty $U $B -hubURL "$H" -hubToken "$T" > $O/console.log 2>&1 &
sleep 7; shot 01-list
Y=$(row demo); echo "demo row at y=$Y"
xcrun simctl io "$U" recordVideo --codec h264 --force "$O/stream-scroll.mp4" >/dev/null 2>&1 & REC=$!
sleep 1
idb ui tap 120 $Y --udid $U; sleep 5; shot 02-demo
idb ui tap 200 788 --udid $U; sleep 1
idb ui text "Write twenty short numbered lines, one fact about a different planet or moon on each line, no preamble." --udid $U; idb ui key 40 --udid $U
sleep 4; shot 03-streaming-at-tail
# scroll up while it streams: two swipes down (finger moves down = content moves up = older lines)
idb ui swipe 196 400 196 750 --duration 0.25 --udid $U; sleep 0.5; idb ui swipe 196 400 196 750 --duration 0.25 --udid $U
sleep 1; shot 04-scrolled-up-while-streaming
sleep 4; shot 05-still-up-4s-later
sleep 4; shot 06-still-up-8s-later
# back to the tail by hand
for i in 1 2 3 4 5 6; do idb ui swipe 196 650 196 150 --duration 0.15 --udid $U; done; sleep 1.5; shot 07-back-at-tail
sleep 2; kill -INT $REC 2>/dev/null; sleep 3
# workers: pill → sheet → worker chat → back
idb ui tap 70 732 --udid $U; sleep 2; shot 08-workers-sheet
idb ui tap 196 492 --udid $U; sleep 4; shot 09-worker-chat
idb ui tap 42 85 --udid $U; sleep 2; shot 10-back-in-demo
# background 8 s → foreground
xcrun simctl spawn $U launchctl kickstart -k system/com.apple.SpringBoard >/dev/null 2>&1 || true
sleep 1
idb ui button HOME --udid $U 2>/dev/null || xcrun simctl io $U sendkey home 2>/dev/null; sleep 8
xcrun simctl launch $U $B >/dev/null 2>&1; sleep 4; shot 11-after-background-8s
# cold start
xcrun simctl terminate $U $B; sleep 2; xcrun simctl launch --console-pty $U $B -hubURL "$H" -hubToken "$T" > $O/console-cold.log 2>&1 & sleep 6; shot 12-cold-start
ffmpeg -v error -y -i "$O/stream-scroll.mp4" -vf "scale=786:-2,fps=30" -c:v libx264 -crf 24 -preset veryfast -pix_fmt yuv420p -an "$O/stream-while-scrolled-up.mp4"
ls -la $O/*.mp4

#!/bin/bash
# Tools come from the checkout beside this file, never from a copy in
# $HOME. M-238 fixed the journey this way and left every other script
# calling ~/: the two drift, and a fix that lands in the repository
# never reaches the run.
HERE=$(cd "$(dirname "$0")" && pwd)
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
O=~/mobile-out/cycle-37; U=$(cut -d' ' -f2 $O/sim.txt)
ypos() { idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
key = sys.argv[1]
for e in json.load(sys.stdin):
    if key in str(e.get("AXLabel") or ""):
        f = e["frame"]; print(int(f["x"] + f["width"] / 2), int(f["y"] + f["height"] / 2)); break
' "$1"; }
idb ui tap 30 60 --udid $U; sleep 1.5
rm -f $O/recording-long-history-paging*.mp4
xcrun simctl io $U recordVideo --codec h264 $O/recording-long-history-paging.mp4 >/dev/null 2>&1 & REC=$!; sleep 1.5
Y=$(xcrun simctl io $U screenshot /tmp/r.png >/dev/null 2>&1; python3 "$HERE/../find_row.py" /tmp/r.png phone); idb ui tap 120 $Y --udid $U; sleep 2.5
for i in $(seq 1 12); do idb ui swipe 196 250 196 800 --duration 0.08 --udid $U; done; sleep 1.2
E=$(ypos earlier); echo "earlier at $E"; idb ui tap $E --udid $U; sleep 2.5
for i in $(seq 1 12); do idb ui swipe 196 250 196 800 --duration 0.08 --udid $U; done; sleep 1.2
E=$(ypos earlier); echo "earlier again at $E"; idb ui tap $E --udid $U; sleep 2.5
for i in $(seq 1 25); do idb ui swipe 196 800 196 200 --duration 0.08 --udid $U; done; sleep 1.5
kill -INT $REC; sleep 3; ffprobe -v error -show_entries format=duration -of csv=p=0 $O/recording-long-history-paging.mp4
ffmpeg -v error -y -i $O/recording-long-history-paging.mp4 -vf "scale=786:-2,fps=30" -c:v libx264 -crf 24 -preset veryfast -pix_fmt yuv420p -an $O/recording-long-history-paging-small.mp4
ffmpeg -v error -y -i $O/recording-long-history-paging-small.mp4 -vf "fps=1/3,scale=200:-1,tile=9x1" -frames:v 1 /tmp/rec-tiles.png

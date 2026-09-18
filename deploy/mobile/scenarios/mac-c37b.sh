#!/bin/bash
# Cycle 37 b: the agents sheet with the keyboard down, then the long
# history on phone reached by flinging to the top (a status-bar tap does
# not scroll a SwiftUI ScrollView to the top in the simulator). Recorded.
# Tools come from the checkout beside this file, never from a copy in
# $HOME. M-238 fixed the journey this way and left every other script
# calling ~/: the two drift, and a fix that lands in the repository
# never reaches the run.
HERE=$(cd "$(dirname "$0")" && pwd)
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
O="$HOME/mobile-out/cycle-37"; U=$(cut -d' ' -f2 "$O/sim.txt"); B=com.unarbos.arbos.ios
shot() { xcrun simctl io "$U" screenshot "$O/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
row() { xcrun simctl io "$U" screenshot /tmp/r.png >/dev/null 2>&1; python3 "$HERE/../find_row.py" /tmp/r.png "$1"; }
desc() { idb ui describe-all --udid "$U" 2>/dev/null; }
# centre y of the first element whose label contains $1
ypos() { desc | python3 -c '
import sys, json
key = sys.argv[1]
try: els = json.load(sys.stdin)
except Exception: els = []
for e in els:
    if key in str(e.get("AXLabel") or ""):
        f = e.get("frame", {}); print(int(f.get("x", 0) + f.get("width", 0) / 2), int(f.get("y", 0) + f.get("height", 0) / 2)); break
' "$1"; }
idb connect "$U" >/dev/null 2>&1

# ── agents sheet, keyboard down (pod chat is open) ──────────────────────
idb ui tap 196 300 --udid "$U"; sleep 1.2          # a tap on the words puts the keyboard away (F6)
P=$(ypos "Agents"); echo "pill at ${P:-none}"
if [ -n "$P" ]; then idb ui tap $P --udid "$U"; sleep 2; shot 15-pod-agents-sheet-7; desc | grep -o '"AXLabel": "[^"]*"' | grep -iv 'keyboard' | head -20 | tee "$O/sheet-labels.txt"; idb ui swipe 196 500 196 850 --duration 0.3 --udid "$U"; sleep 1; fi
idb ui tap 30 60 --udid "$U"; sleep 1.5

# ── long history on phone: fling to the top, recorded ───────────────────
xcrun simctl io "$U" recordVideo --codec h264 "$O/recording-long-history.mp4" >/dev/null 2>&1 &
REC=$!; sleep 1
Y=$(row phone); idb ui tap 120 "$Y" --udid "$U"; sleep 3
for i in $(seq 1 40); do
  idb ui swipe 196 250 196 800 --duration 0.08 --udid "$U"
  if [ $((i % 5)) -eq 0 ]; then sleep 0.8; E=$(ypos "earlier"); [ -n "$E" ] && { echo "earlier button after $i flings at $E"; break; }; fi
done
sleep 1; shot 16-phone-top-earlier-button
desc | grep -o '"AXLabel": "[^"]*earlier[^"]*"' | head -1 | tee "$O/earlier-label-1.txt"
E=$(ypos "earlier")
if [ -n "$E" ]; then
  # remember the first visible line under the button, tap, and see if it held
  idb ui tap $E --udid "$U"; sleep 3; shot 17-phone-after-load-earlier
  desc | grep -o '"AXLabel": "[^"]*earlier[^"]*"' | head -1 | tee "$O/earlier-label-2.txt"
  for i in $(seq 1 15); do idb ui swipe 196 250 196 800 --duration 0.08 --udid "$U"; done; sleep 1; shot 18-phone-top-after-page
  desc | grep -o '"AXLabel": "[^"]*earlier[^"]*"' | head -1 | tee "$O/earlier-label-3.txt"
fi
for i in $(seq 1 30); do idb ui swipe 196 800 196 200 --duration 0.08 --udid "$U"; done; sleep 2; shot 19-phone-back-at-tail
kill -INT $REC 2>/dev/null; sleep 2
idb ui tap 30 60 --udid "$U"; sleep 1
echo DONE

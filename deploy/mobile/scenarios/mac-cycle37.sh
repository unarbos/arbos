#!/bin/bash
# Cycle 37: the aspects checked longest ago — long history and older-lines
# paging (phone, 1265 lines through the ArbosLife hub), several workers at
# once and archived children kept across a reopen (pod), cold start,
# short background. Recorded (due). Output under ~/mobile-out/cycle-37/.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
O="$HOME/mobile-out/cycle-37"; mkdir -p "$O"
U=$(cut -d' ' -f2 "$O/sim.txt"); B=com.unarbos.arbos.ios
T=$(plutil -extract hubToken raw -o - ~/arbos/ios/Arbos/Secrets.plist); H=$(plutil -extract hubURL raw -o - ~/arbos/ios/Arbos/Secrets.plist)
shot() { xcrun simctl io "$U" screenshot "$O/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
row() { xcrun simctl io "$U" screenshot /tmp/r.png >/dev/null 2>&1; python3 ~/find_row.py /tmp/r.png "$1"; }
desc() { idb ui describe-all --udid "$U" 2>/dev/null; }
idb connect "$U" >/dev/null 2>&1

# ── cold start: terminate, launch, time to rows ─────────────────────────
xcrun simctl terminate "$U" $B 2>/dev/null; sleep 1
T0=$(date +%s.%N)
xcrun simctl launch "$U" $B -hubURL "$H" -hubToken "$T" >/dev/null 2>&1
for i in $(seq 1 30); do
  sleep 0.5
  if desc | grep -q '"AXLabel": "phone'; then echo "cold start → rows at $(python3 -c "print(round($(date +%s.%N)-$T0,1))") s"; break; fi
done
shot 02-cold-start-list

# ── long history: phone (1265 lines), recorded ──────────────────────────
xcrun simctl io "$U" recordVideo --codec h264 "$O/recording-long-history.mp4" >/dev/null 2>&1 &
REC=$!; sleep 1
Y=$(row phone); idb ui tap 120 "$Y" --udid "$U"; T1=$(date +%s.%N)
for i in $(seq 1 40); do sleep 0.25; if desc | grep -q 'earlier lines\|Follow up'; then echo "phone chat open → content at $(python3 -c "print(round($(date +%s.%N)-$T1,2))") s"; break; fi; done
sleep 2; shot 03-phone-tail
# to the top: status-bar tap scrolls to top; then find the earlier-lines button
idb ui tap 196 20 --udid "$U"; sleep 2; shot 04-phone-top
desc | grep -B2 -A6 'earlier' | grep -E 'AXLabel|frame' | head -6 | tee "$O/earlier-button.txt"
EY=$(desc | python3 -c '
import sys, json, re
txt = sys.stdin.read()
try: els = json.loads(txt)
except Exception: els = []
for e in els if isinstance(els, list) else []:
    lab = str(e.get("AXLabel") or "")
    if "earlier" in lab:
        f = e.get("frame", {}); print(int(f.get("y", 0) + f.get("height", 0) / 2)); break
')
echo "earlier button y=${EY:-none}"
if [ -n "${EY:-}" ]; then
  idb ui tap 196 "$EY" --udid "$U"; T2=$(date +%s.%N); sleep 3; shot 05-phone-after-load-earlier
  echo "paged; held place? compare 04 vs 05"
  idb ui tap 196 20 --udid "$U"; sleep 2; shot 06-phone-top-again
  desc | grep -o '"AXLabel": "[^"]*earlier[^"]*"' | head -1
fi
# back to the tail: swipe up several times fast
for i in 1 2 3 4 5 6; do idb ui swipe 196 750 196 150 --duration 0.15 --udid "$U"; done; sleep 2; shot 07-phone-back-at-tail
sleep 1; kill -INT $REC 2>/dev/null; sleep 2
idb ui tap 30 60 --udid "$U"; sleep 1.5   # back

# ── several workers at once: pod ────────────────────────────────────────
Y=$(row pod); idb ui tap 120 "$Y" --udid "$U"; sleep 4; shot 08-pod-open
idb ui tap 200 788 --udid "$U"; sleep 1
idb ui text "C37 $(date -u +%H%M): use four sub-agents with the spawn tool at once, each says one sentence about a season (spring, summer, autumn, winter). No files, no commits. Then list the four sentences." --udid "$U"; sleep 0.5
idb ui key 40 --udid "$U"
for i in $(seq 1 40); do sleep 1; if desc | grep -qi 'Working [2-4]\|Agents [2-4]'; then echo "pill at ${i}s: $(desc | grep -o '"AXLabel": "\(Working\|Agents\) [0-9]*"' | head -1)"; break; fi; done
shot 09-pod-four-workers
idb ui tap 60 779 --udid "$U"; sleep 2; shot 10-pod-agents-sheet
idb ui swipe 196 500 196 850 --duration 0.3 --udid "$U"; sleep 1
sleep 30; shot 11-pod-after-four
desc | grep -o '"AXLabel": "\(Working\|Agents\|Done\)[^"]*"' | sort | uniq -c | head
# archived children kept across a reopen
idb ui tap 30 60 --udid "$U"; sleep 1.5; Y=$(row pod); idb ui tap 120 "$Y" --udid "$U"; sleep 4; shot 12-pod-reopened
idb ui tap 60 779 --udid "$U"; sleep 2; shot 13-pod-sheet-after-reopen
idb ui swipe 196 500 196 850 --duration 0.3 --udid "$U"; sleep 1

# ── short background 8 s → foreground ───────────────────────────────────
idb ui button HOME --udid "$U"; sleep 8
xcrun simctl launch "$U" $B >/dev/null 2>&1; sleep 3; shot 14-after-8s-background
echo DONE

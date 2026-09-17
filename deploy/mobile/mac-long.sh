#!/bin/bash
# Cycle 5: long history, many workers, a network drop, a long background.
#   mac-long.sh <cycle>
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE"; mkdir -p "$OUT"
UDID=$(cut -d' ' -f2 "$OUT/sim.txt")
BUNDLE=com.unarbos.arbos.ios
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
idb connect "$UDID" >/dev/null 2>&1
xcrun simctl terminate "$UDID" $BUNDLE 2>/dev/null || true
xcrun simctl launch --console-pty "$UDID" $BUNDLE > "$OUT/long-console.log" 2>&1 &
sleep 5; shot l-01-list
T0=$(date +%s.%N)
idb ui tap 120 291 --udid "$UDID"           # demo
sleep 6; shot l-02-open-long-history
echo "open→content ~ $(python3 -c "print(round($(date +%s.%N)-$T0-6,1))") s (see console for history_end)"
# scroll up twice, capture, then back down
idb ui swipe 196 300 196 700 --duration 0.3 --udid "$UDID"; sleep 1
idb ui swipe 196 300 196 700 --duration 0.3 --udid "$UDID"; sleep 1.5; shot l-03-scrolled-up
sleep 3; shot l-03b-after-scroll-up-3s
idb ui swipe 196 700 196 200 --duration 0.2 --udid "$UDID"; idb ui swipe 196 700 196 200 --duration 0.2 --udid "$UDID"; sleep 1.5; shot l-04-back-down
# four workers at once
idb ui tap 200 788 --udid "$UDID"; sleep 1.2
idb ui text "Use four sub-agents with the spawn tool at once: each says one sentence about a season (spring, summer, autumn, winter). No files, no commits. Then list the four sentences." --udid "$UDID"; sleep 1
idb ui key 40 --udid "$UDID"; sleep 10; shot l-05-four-workers
idb ui tap 60 779 --udid "$UDID"; sleep 2; shot l-06-agents-sheet   # the Working pill → sheet
idb ui swipe 196 500 196 850 --duration 0.3 --udid "$UDID"; sleep 1  # dismiss sheet
sleep 25; shot l-07-after-four
# network drop: block the hub for 45 s
HUB=$(grep -m1 -o "hubURL</key><string>[^<]*" ~/arbos/ios/Arbos/Secrets.plist | sed 's/.*<string>//' | sed -E 's#wss?://##; s#/.*##')
HUBIP=$(dig +short "$HUB" | head -1)
echo "hub $HUB → $HUBIP"
echo "block drop out quick on en0 to $HUBIP" | sudo pfctl -ef - 2>/dev/null; sudo pfctl -e 2>/dev/null
sleep 8; shot l-08-link-cut
sleep 20; shot l-09-reconnecting
sudo pfctl -d 2>/dev/null; sudo pfctl -F all 2>/dev/null
sleep 25; shot l-10-after-drop
grep -E "dropped|closed|reconnect|frame turn|history_end" "$OUT/long-console.log" | tail -6
# long background: Home for 10 minutes, then back
idb ui button HOME --udid "$UDID"; sleep 600
xcrun simctl launch "$UDID" $BUNDLE >/dev/null 2>&1; sleep 4; shot l-11-resume-after-10min
sleep 8; shot l-12-resume-settled

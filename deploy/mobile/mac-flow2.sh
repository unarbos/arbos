#!/bin/bash
# Cycle-2 flow: list → demo → send → worker lines → tap a worker line →
# worker chat → back → after the turn; then background/foreground the app
# (Home, relaunch) and capture the reconnect.
#   mac-flow2.sh <cycle> "<message>"
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
CYCLE=${1:?cycle}; MSG=${2:?message}
OUT="$HOME/mobile-out/$CYCLE"; mkdir -p "$OUT"
UDID=$(cut -d' ' -f2 "$OUT/sim.txt")
BUNDLE=com.unarbos.arbos.ios
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "shot $1"; }
idb connect "$UDID" >/dev/null 2>&1
xcrun simctl terminate "$UDID" $BUNDLE 2>/dev/null || true
xcrun simctl launch --console-pty "$UDID" $BUNDLE > "$OUT/flow2-console.log" 2>&1 &
sleep 5
shot f2-01-projects
idb ui tap 120 291 --udid "$UDID"; sleep 4
shot f2-02-chat
idb ui tap 200 788 --udid "$UDID"; sleep 1.5
idb ui text "$MSG" --udid "$UDID"; sleep 1
idb ui key 40 --udid "$UDID"; sleep 7
shot f2-03-working
# the first worker line sits just above the pill: ~657 pt
idb ui tap 200 657 --udid "$UDID"; sleep 4
shot f2-04-worker-chat
idb ui tap 50 84 --udid "$UDID"; sleep 2
shot f2-05-back
sleep 30
shot f2-06-after-turn
# background: Home, wait, relaunch (simctl launch foregrounds the running app)
idb ui button HOME --udid "$UDID" 2>/dev/null || xcrun simctl io "$UDID" 2>/dev/null
sleep 8
shot f2-07-home
xcrun simctl launch "$UDID" $BUNDLE >/dev/null 2>&1
sleep 5
shot f2-08-foreground-again
grep -E "^frame|Keychain|dropped|closed" "$OUT/flow2-console.log" | sort | uniq -c | sort -rn | head -8

#!/bin/bash
# Cycle 8: notify → banner while away, badge, the away card, seen.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
OUT="$HOME/mobile-out/cycle-8"; UDID=$(cut -d' ' -f2 "$OUT/sim.txt"); B=com.unarbos.arbos.ios
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
idb connect "$UDID" >/dev/null 2>&1
sleep 6
idb ui tap 120 296 --udid "$UDID"; sleep 7; shot r-00-open
# permission prompt may be up (fresh install): Allow is the right button
idb ui tap 271 506 --udid "$UDID"; sleep 1
idb ui tap 200 788 --udid "$UDID"; sleep 1
idb ui text "Reply with exactly: the kettle is on." --udid "$UDID"; idb ui key 40 --udid "$UDID"; sleep 0.2
xcrun simctl launch "$UDID" com.apple.Preferences >/dev/null 2>&1; sleep 4; shot r-01-banner-and-badge; xcrun simctl terminate "$UDID" com.apple.Preferences 2>/dev/null; sleep 1
sleep 5; shot r-02-badge
xcrun simctl launch "$UDID" $B >/dev/null 2>&1; sleep 3; shot r-03-away-card
idb ui tap 340 0 --udid "$UDID" >/dev/null 2>&1
# Got it sits at the card's right edge; find it by a tap near the card header (y from the still) — try the common spot
idb ui tap 380 660 --udid "$UDID"; sleep 1.5; shot r-04-after-got-it
# replay on attach: send, kill the app before the reply is seen, relaunch
idb ui tap 200 788 --udid "$UDID"; sleep 1
idb ui text "Reply with exactly: replayed while away." --udid "$UDID"; idb ui key 40 --udid "$UDID"; sleep 0.1
xcrun simctl terminate "$UDID" $B; sleep 4; shot r-05-home-after-kill
xcrun simctl launch "$UDID" $B >/dev/null 2>&1; sleep 4; idb ui tap 120 296 --udid "$UDID"; sleep 7; shot r-06-card-from-replay

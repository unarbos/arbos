#!/bin/bash
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
OUT="$HOME/mobile-out/cycle-7"; UDID=$(cut -d' ' -f2 "$OUT/sim.txt"); B=com.unarbos.arbos.ios
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
idb connect "$UDID" >/dev/null 2>&1
sleep 6
idb ui tap 42 85 --udid "$UDID"; sleep 2; shot c-01-settings-build
idb ui swipe 196 200 196 800 --duration 0.4 --udid "$UDID"; sleep 1.5
idb ui tap 120 370 --udid "$UDID"; sleep 7; shot c-02-subnet120-after-replay
idb ui swipe 196 300 196 700 --duration 0.3 --udid "$UDID"; sleep 1.5; shot c-03-scrolled-under-pill
idb ui tap 351 85 --udid "$UDID"; sleep 1.2; idb ui tap 300 136 --udid "$UDID"; sleep 7; shot c-04-after-reconnect
idb ui tap 351 85 --udid "$UDID"; sleep 1.2; idb ui tap 300 94 --udid "$UDID"; sleep 4; shot c-05-call-voice-first
idb ui swipe 196 250 196 600 --duration 0.4 --udid "$UDID"; sleep 1.5; shot c-06-call-pulled-down

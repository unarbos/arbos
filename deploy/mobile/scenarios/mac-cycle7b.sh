#!/bin/bash
# Cycle 7, second pass with pt coordinates (px / 1.2): search, filter,
# the workers sheet, the ⋯ menu and the call.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
OUT="$HOME/mobile-out/cycle-7"; UDID=$(cut -d' ' -f2 "$OUT/sim.txt"); B=com.unarbos.arbos.ios
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
idb connect "$UDID" >/dev/null 2>&1
# search
idb ui tap 298 85 --udid "$UDID"; sleep 1.2; shot b-01-search-open
idb ui text "sub" --udid "$UDID"; sleep 1.5; shot b-02-search-sub
idb ui tap 298 85 --udid "$UDID"; sleep 1; shot b-03-search-closed
# filter
idb ui tap 351 85 --udid "$UDID"; sleep 1.2; shot b-04-filter-menu
idb ui tap 240 136 --udid "$UDID"; sleep 1.5; shot b-05-live-only
idb ui tap 351 85 --udid "$UDID"; sleep 1; idb ui tap 240 94 --udid "$UDID"; sleep 1
# workers sheet on subnet120 (two archived children from the first pass)
idb ui tap 120 370 --udid "$UDID"; sleep 6; shot b-06-subnet120
idb ui tap 69 732 --udid "$UDID"; sleep 2; shot b-07-workers-sheet
idb ui tap 120 420 --udid "$UDID"; sleep 4; shot b-08-worker-chat
idb ui tap 42 85 --udid "$UDID"; sleep 2
# the ⋯ menu → Call
idb ui tap 351 85 --udid "$UDID"; sleep 1.2; shot b-09-menu
idb ui tap 300 136 --udid "$UDID"; sleep 4; shot b-10-call-voice-first
idb ui swipe 196 250 196 600 --duration 0.4 --udid "$UDID"; sleep 1.5; shot b-11-call-pulled-down
idb ui tap 42 85 --udid "$UDID"; sleep 2; shot b-12-after-call

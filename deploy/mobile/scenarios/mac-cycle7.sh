#!/bin/bash
# Cycle 7: list search/filter/refresh, the list's composer, settings, the
# workers sheet (archived children), the call screen, and M-63.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
OUT="$HOME/mobile-out/cycle-7"; mkdir -p "$OUT"
UDID=$(cut -d' ' -f2 "$OUT/sim.txt"); B=com.unarbos.arbos.ios
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
idb connect "$UDID" >/dev/null 2>&1
xcrun simctl terminate "$UDID" $B 2>/dev/null || true
xcrun simctl keychain "$UDID" reset >/dev/null 2>&1      # a fresh phone: the baked token is adopted
xcrun simctl spawn "$UDID" defaults delete $B >/dev/null 2>&1
xcrun simctl privacy "$UDID" grant microphone $B >/dev/null 2>&1
xcrun simctl launch --console-pty "$UDID" $B > "$OUT/console.log" 2>&1 &
sleep 8; shot 01-list-fresh
# search
idb ui tap 358 102 --udid "$UDID"; sleep 1; idb ui text "sub" --udid "$UDID"; sleep 1.5; shot 02-search-sub
idb ui key 40 --udid "$UDID"; sleep 0.5
idb ui tap 358 102 --udid "$UDID"; sleep 1; shot 03-search-closed
# filter (live only)
idb ui tap 421 102 --udid "$UDID"; sleep 1.5; shot 04-filter-on
idb ui tap 421 102 --udid "$UDID"; sleep 1
# pull to refresh
idb ui swipe 196 300 196 700 --duration 0.4 --udid "$UDID"; sleep 0.6; shot 05-refreshing; sleep 3; shot 06-refreshed
# settings
idb ui tap 50 102 --udid "$UDID"; sleep 2; shot 07-settings
idb ui swipe 196 700 196 250 --duration 0.4 --udid "$UDID"; sleep 1.5; shot 08-settings-scrolled
idb ui swipe 196 200 196 800 --duration 0.4 --udid "$UDID"; sleep 1.5   # dismiss the sheet
# the list's composer → last project
idb ui tap 200 788 --udid "$UDID"; sleep 1.2
idb ui text "Reply with exactly: from the list." --udid "$UDID"; sleep 0.5; idb ui key 40 --udid "$UDID"
sleep 2; shot 09-list-composer-opened; sleep 10; shot 10-list-composer-reply
idb ui tap 50 102 --udid "$UDID"; sleep 1.5
# workers sheet with archived children: open demo, ask for two quick workers, wait, open the sheet
idb ui tap 120 380 --udid "$UDID"; sleep 6
idb ui tap 200 788 --udid "$UDID"; sleep 1
idb ui text "Spawn two sub-agents at once with the spawn tool: each names one colour in one word. No files. Then list both words." --udid "$UDID"; sleep 0.5; idb ui key 40 --udid "$UDID"
sleep 40; shot 11-workers-done
idb ui tap 80 850 --udid "$UDID"; sleep 2; shot 12-workers-sheet
idb ui swipe 196 500 196 850 --duration 0.3 --udid "$UDID"; sleep 1
# the call: voice-first, pulled down
idb ui tap 421 102 --udid "$UDID"; sleep 1.2; shot 13-menu
idb ui tap 300 160 --udid "$UDID"; sleep 4; shot 14-call-voice-first
idb ui swipe 196 300 196 650 --duration 0.4 --udid "$UDID"; sleep 1.5; shot 15-call-pulled-down
idb ui tap 50 102 --udid "$UDID"; sleep 2; shot 16-call-closed
grep -a -E "roster|attach" "$OUT/console.log" | head -6 | cut -c1-160

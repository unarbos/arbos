#!/bin/bash
# Cycle 8: a reply while the app is in the background → a notification.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
OUT="$HOME/mobile-out/cycle-8"; mkdir -p "$OUT"; UDID=$(cut -d' ' -f2 "$OUT/sim.txt"); B=com.unarbos.arbos.ios
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
idb connect "$UDID" >/dev/null 2>&1
sleep 6
idb ui tap 120 296 --udid "$UDID"; sleep 6; shot n-01-permission-prompt
idb ui tap 300 520 --udid "$UDID"; sleep 1.5; shot n-02-after-allow      # "Allow" on the system alert (right button)
idb ui tap 200 788 --udid "$UDID"; sleep 1
idb ui text "Reply with exactly: the kettle is on." --udid "$UDID"; idb ui key 40 --udid "$UDID"; sleep 0.3
idb ui button HOME --udid "$UDID"; sleep 1; shot n-03-home
sleep 9; shot n-04-banner
sleep 4; shot n-05-banner-late
# tap the banner → the project's chat
idb ui tap 196 60 --udid "$UDID"; sleep 3; shot n-06-opened-from-banner
# a worker's finish while away
idb ui tap 200 788 --udid "$UDID"; sleep 1
idb ui text "Spawn one sub-agent with the spawn tool that says one word about the sea. No files." --udid "$UDID"; idb ui key 40 --udid "$UDID"; sleep 0.3
idb ui button HOME --udid "$UDID"; sleep 25; shot n-07-worker-banner

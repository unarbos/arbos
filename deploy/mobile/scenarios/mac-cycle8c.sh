#!/bin/bash
# Cycle 8, pass c: a banner while another app is in front (a second client
# starts the turn), the badge, replay on attach, seen from another client.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
OUT="$HOME/mobile-out/cycle-8"; UDID=$(cut -d' ' -f2 "$OUT/sim.txt"); B=com.unarbos.arbos.ios
ARGS="-hubURL ws://127.0.0.1:7780 -hubToken ${LOCAL_HUB_CLIENT_TOKEN:?the loopback hub fixture client token}"
HUB="ws://127.0.0.1:7780/attach/awsmac/longproj"; TOK="Authorization: Bearer ${LOCAL_HUB_CLIENT_TOKEN:?the loopback hub fixture client token}"
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
send_other() { (sleep 1; echo "{\"type\":\"user\",\"agent\":\"root\",\"text\":\"$1\",\"channel\":\"text\",\"device\":\"desktop\"}"; sleep 3) | websocat -t --header="$TOK" "$HUB" >/dev/null 2>&1; }
idb connect "$UDID" >/dev/null 2>&1
# the app on longproj, then Settings in front
xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B $ARGS >/dev/null 2>&1; sleep 6
idb ui tap 120 296 --udid "$UDID"; sleep 6
idb ui tap 380 660 --udid "$UDID"; sleep 1          # Got it, if a card is up
xcrun simctl launch "$UDID" com.apple.Preferences >/dev/null 2>&1; sleep 2
send_other "Reply with exactly: a banner from the desktop."
sleep 3; shot s-01-banner-over-settings
xcrun simctl terminate "$UDID" com.apple.Preferences 2>/dev/null; sleep 2; shot s-02-home-badge
# back to the app: the card, then Got it
xcrun simctl launch "$UDID" $B $ARGS >/dev/null 2>&1; sleep 3; shot s-03-card-on-return
idb ui tap 380 660 --udid "$UDID"; sleep 1.5; shot s-04-after-got-it
# replay on attach: kill the app, a second client starts a turn, relaunch
xcrun simctl terminate "$UDID" $B; sleep 1
send_other "Reply with exactly: replayed on attach."
sleep 2
xcrun simctl launch "$UDID" $B $ARGS >/dev/null 2>&1; sleep 5; idb ui tap 120 296 --udid "$UDID"; sleep 7; shot s-05-card-from-replay
# seen from another client clears the phone
(sleep 1; echo '{"type":"seen","through":999}'; sleep 2) | websocat -t --header="$TOK" "$HUB" >/dev/null 2>&1
sleep 2; shot s-06-cleared-by-other-client

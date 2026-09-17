#!/bin/bash
# Proof A: lines typed into demo while its link is cut must never run in phone.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
U=B1185668-7488-420F-B12D-4412BAAC7673; O=$HOME/mobile-out/cycle-12
shot() { xcrun simctl io "$U" screenshot "$O/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
MARK="ISOLATION-A $(date -u +%H%M%S)"
echo "marker: $MARK" | tee $O/a-marker.txt
idb ui tap 120 308 --udid $U; sleep 5; shot a-01-demo-open
echo "block drop out quick proto tcp from any to any port 443" | sudo pfctl -ef - 2>/dev/null; echo "$(date -u +%H:%M:%S) 443 blocked"
sleep 3
idb ui tap 200 788 --udid $U; sleep 1
idb ui text "$MARK first: reply with exactly LEAKED-A-1" --udid $U; idb ui key 40 --udid $U; sleep 2
idb ui text "$MARK second: reply with exactly LEAKED-A-2" --udid $U; idb ui key 40 --udid $U; sleep 12
shot a-02-demo-two-pending
idb ui tap 42 85 --udid $U; sleep 2; shot a-03-back-to-list
idb ui tap 120 382 --udid $U; sleep 4; shot a-04-phone-open-link-down
sudo pfctl -d 2>/dev/null; sudo pfctl -F all 2>/dev/null; echo "$(date -u +%H:%M:%S) 443 open"
sleep 45; shot a-05-phone-after-link-back
for i in 1 2 3 4 5 6; do idb ui swipe 196 800 196 200 --duration 0.05 --udid $U; done; sleep 1; shot a-06-phone-tail
idb ui tap 42 85 --udid $U; sleep 2; idb ui tap 120 308 --udid $U; sleep 6; shot a-07-demo-after
echo "--- console mentions of the marker:"; grep -c "LEAKED-A" $O/app-console.log || true

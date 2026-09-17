#!/bin/bash
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
O=~/mobile-out/cycle-37; U=$(cut -d' ' -f2 $O/sim.txt); B=com.unarbos.arbos.ios
T=$(plutil -extract hubToken raw -o - ~/arbos/ios/Arbos/Secrets.plist); H=$(plutil -extract hubURL raw -o - ~/arbos/ios/Arbos/Secrets.plist)
shot() { xcrun simctl io $U screenshot $O/$1.png >/dev/null 2>&1; echo shot $1; }
rows() { idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
els=json.load(sys.stdin)
print([e["AXLabel"] for e in els if e.get("type")=="Button" and e.get("AXLabel") and ", " in e["AXLabel"]])
print([ (e.get("AXLabel") or "")[:90] for e in els if e.get("type")=="StaticText" and e.get("AXLabel")][:6])'; }
# hub token field → a wrong token
idb ui tap 196 627 --udid $U; sleep 1; idb ui text "not-a-real-token" --udid $U; sleep 0.5; shot 51-settings-token-typed
idb ui tap 340 97 --udid $U; sleep 6; shot 52-list-after-bad-token; rows
# refresh by pull
idb ui swipe 196 300 196 650 --duration 0.4 --udid $U; sleep 6; shot 53-list-after-refresh-bad-token; rows
# put the right token back through Settings (paste path): the field takes a paste; type it
idb ui tap 42 85 --udid $U; sleep 2; idb ui tap 196 627 --udid $U; sleep 1; idb ui text "$T" --udid $U; sleep 0.5; idb ui tap 340 97 --udid $U; sleep 6; shot 54-list-after-token-restored; rows

#!/bin/bash
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
O=~/mobile-out/cycle-37; U=$(cut -d' ' -f2 $O/sim.txt); B=com.unarbos.arbos.ios
T=$(plutil -extract hubToken raw -o - ~/arbos/ios/Arbos/Secrets.plist); H=$(plutil -extract hubURL raw -o - ~/arbos/ios/Arbos/Secrets.plist)
shot() { xcrun simctl io $U screenshot $O/$1.png >/dev/null 2>&1; echo shot $1; }
rows() { idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
els=json.load(sys.stdin)
print([e["AXLabel"] for e in els if e.get("type")=="Button" and e.get("AXLabel") and ", " in e["AXLabel"]])'; }
idb ui tap 196 600 --udid $U; sleep 1   # close the filter menu
idb ui tap 42 85 --udid $U; sleep 2; shot 55-settings-again
idb ui tap 196 627 --udid $U; sleep 1; idb ui text "$T" --udid $U; sleep 0.5; idb ui tap 340 97 --udid $U; sleep 2; shot 56-after-done
# the system Save Password sheet, if it shows: Not Now
NN=$(idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
for e in json.load(sys.stdin):
    if e.get("AXLabel")=="Not Now": f=e["frame"]; print(int(f["x"]+f["width"]/2), int(f["y"]+f["height"]/2)); break')
echo "not-now at ${NN:-none}"; [ -n "$NN" ] && idb ui tap $NN --udid $U; sleep 6; shot 57-list-after-restore; rows
sleep 12; shot 58-list-after-restore-18s; rows

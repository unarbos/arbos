#!/bin/bash
# Tools come from the checkout beside this file, never from a copy in
# $HOME. M-238 fixed the journey this way and left every other script
# calling ~/: the two drift, and a fix that lands in the repository
# never reaches the run.
HERE=$(cd "$(dirname "$0")" && pwd)
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
O=~/mobile-out/cycle-37; U=$(cut -d' ' -f2 $O/sim.txt); B=com.unarbos.arbos.ios
T=$(plutil -extract hubToken raw -o - ~/arbos/ios/Arbos/Secrets.plist); H=$(plutil -extract hubURL raw -o - ~/arbos/ios/Arbos/Secrets.plist)
shot() { xcrun simctl io $U screenshot $O/$1.png >/dev/null 2>&1; echo shot $1; }
rows() { idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
print([e["AXLabel"] for e in json.load(sys.stdin) if e.get("type")=="Button" and e.get("AXLabel") and ", " in e["AXLabel"]])'; }
idb connect $U >/dev/null 2>&1
xcrun simctl terminate $U $B 2>/dev/null; sleep 1; xcrun simctl launch $U $B -hubURL "$H" -hubToken "$T" >/dev/null 2>&1; sleep 6
echo "before: $(rows)"; shot 40-list-before-fold
Y=$(xcrun simctl io $U screenshot /tmp/r.png >/dev/null 2>&1; python3 "$HERE/../find_row.py" /tmp/r.png pod); idb ui tap 120 $Y --udid $U; sleep 4; shot 41-pod-open
idb ui tap 30 60 --udid $U; sleep 2; echo "after opening pod: $(rows)"; shot 42-list-after-fold
# survives a relaunch and a refresh
xcrun simctl terminate $U $B 2>/dev/null; sleep 1; xcrun simctl launch $U $B -hubURL "$H" -hubToken "$T" >/dev/null 2>&1; sleep 6
echo "after relaunch: $(rows)"; shot 43-list-after-relaunch
grep -E "hello|store" $O/app-console.log | tail -3 | cut -c1-160

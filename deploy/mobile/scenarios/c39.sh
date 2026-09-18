#!/bin/bash
# Tools come from the checkout beside this file, never from a copy in
# $HOME. M-238 fixed the journey this way and left every other script
# calling ~/: the two drift, and a fix that lands in the repository
# never reaches the run.
HERE=$(cd "$(dirname "$0")" && pwd)
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
O=~/mobile-out/cycle-39; U=$(cut -d' ' -f2 $O/sim.txt); B=com.unarbos.arbos.ios
T=$(plutil -extract hubToken raw -o - ~/arbos/ios/Arbos/Secrets.plist); H=$(plutil -extract hubURL raw -o - ~/arbos/ios/Arbos/Secrets.plist)
shot() { xcrun simctl io $U screenshot $O/$1.png >/dev/null 2>&1; echo shot $1; }
cat $O/build-sha.txt; grep -c "BUILD SUCCEEDED" $O/xcodebuild.log
idb connect $U >/dev/null 2>&1
xcrun simctl terminate $U $B 2>/dev/null; sleep 1; xcrun simctl launch $U $B -hubURL "$H" -hubToken "$T" >/dev/null 2>&1; sleep 6
Y=$(xcrun simctl io $U screenshot /tmp/r.png >/dev/null 2>&1; python3 "$HERE/../find_row.py" /tmp/r.png demo); idb ui tap 120 $Y --udid $U; sleep 4
# the refused spawn sits a few screens up: fling up until the line is in the tree
for i in $(seq 1 30); do
  idb ui swipe 196 250 196 700 --duration 0.1 --udid $U; sleep 0.3
  if idb ui describe-all --udid $U 2>/dev/null | grep -q "start arbos-kernel serve"; then echo "line found after $i swipes"; break; fi
done
sleep 1; shot 01-refused-spawn-line
idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
for e in json.load(sys.stdin):
    l=e.get("AXLabel") or ""
    if "start arbos-kernel" in l or "Starting" in l or "Agents" in l or "Working" in l: print(e["type"], repr(l[:160]))' | head -6

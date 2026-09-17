#!/bin/bash
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
O=~/mobile-out/cycle-37; U=$(cut -d' ' -f2 $O/sim.txt); B=com.unarbos.arbos.ios
shot() { xcrun simctl io $U screenshot $O/$1.png >/dev/null 2>&1; echo shot $1; }
els() { idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
for e in json.load(sys.stdin):
    l=e.get("AXLabel") or ""; v=e.get("AXValue"); f=e["frame"]
    if e.get("type") in ("Button","TextField","SecureTextField","StaticText","Switch") and (l or v): print(e["type"], repr(l[:50]), repr(str(v)[:30]) if v else "", int(f["x"]+f["width"]/2), int(f["y"]+f["height"]/2))'; }
idb connect $U >/dev/null 2>&1
idb ui tap 42 85 --udid $U; sleep 2; shot 50-settings; els

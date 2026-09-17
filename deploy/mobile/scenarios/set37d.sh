#!/bin/bash
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
O=~/mobile-out/cycle-37; U=$(cut -d' ' -f2 $O/sim.txt); B=com.unarbos.arbos.ios
T=$(plutil -extract hubToken raw -o - ~/arbos/ios/Arbos/Secrets.plist)
shot() { xcrun simctl io $U screenshot $O/$1.png >/dev/null 2>&1; echo shot $1; }
cat $O/build-sha.txt; grep -E "BUILD" $O/xcodebuild.log | tail -1
idb connect $U >/dev/null 2>&1; sleep 3
idb ui tap 42 85 --udid $U; sleep 2
idb ui tap 196 627 --udid $U; sleep 1; idb ui text "$T" --udid $U; sleep 0.5; shot 60-settings-token-typed-fix; idb ui tap 340 97 --udid $U; sleep 3; shot 61-list-after-done-fix
python3 - <<PY
from PIL import Image
im=Image.open("$O/61-list-after-done-fix.png").convert("RGB"); w,h=im.size
# the system sheet sat around the centre; a light grey card there means it is up
c=im.getpixel((w//2,int(h*0.45))); print("centre pixel", c, "sheet up" if sum(c)>200 else "no sheet")
PY
idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
els=json.load(sys.stdin)
print([e["AXLabel"] for e in els if e.get("type")=="Button" and e.get("AXLabel") and ", " in e["AXLabel"]])'

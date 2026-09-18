#!/bin/bash
# Tools come from the checkout beside this file, never from a copy in
# $HOME. M-238 fixed the journey this way and left every other script
# calling ~/: the two drift, and a fix that lands in the repository
# never reaches the run.
HERE=$(cd "$(dirname "$0")" && pwd)
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
O=~/mobile-out/cycle-37; U=$(cut -d' ' -f2 $O/sim.txt); B=com.unarbos.arbos.ios
shot() { xcrun simctl io $U screenshot $O/$1.png >/dev/null 2>&1; echo shot $1; }
ypos() { idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
key = sys.argv[1]
for e in json.load(sys.stdin):
    if key in str(e.get("AXLabel") or ""):
        f = e["frame"]; print(int(f["x"] + f["width"] / 2), int(f["y"] + f["height"] / 2)); break
' "$1"; }
xcrun simctl privacy $U grant microphone $B >/dev/null 2>&1
idb connect $U >/dev/null 2>&1
idb ui button HOME --udid $U; sleep 1
T=$(plutil -extract hubToken raw -o - ~/arbos/ios/Arbos/Secrets.plist); H=$(plutil -extract hubURL raw -o - ~/arbos/ios/Arbos/Secrets.plist)
xcrun simctl terminate $U $B 2>/dev/null; sleep 1; xcrun simctl launch $U $B -hubURL "$H" -hubToken "$T" >/dev/null 2>&1; sleep 6
Y=$(xcrun simctl io $U screenshot /tmp/r.png >/dev/null 2>&1; python3 "$HERE/../find_row.py" /tmp/r.png phone); idb ui tap 120 $Y --udid $U; sleep 3; shot 70-phone-chat-one-surface
idb ui tap 350 73 --udid $U; sleep 1.5; shot 71-chat-menu
idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
for e in json.load(sys.stdin):
    if e.get("type") in ("Button",) and e.get("AXLabel"): f=e["frame"]; print(repr(e["AXLabel"][:40]), int(f["x"]+f["width"]/2), int(f["y"]+f["height"]/2))' | head -8
C=$(ypos Call); echo "call at $C"; [ -n "$C" ] && idb ui tap $C --udid $U; sleep 4; shot 72-call-voice-first
D=$(ypos disc); [ -z "$D" ] && D="196 426"; idb ui tap $D --udid $U; sleep 4; shot 73-call-connected
idb ui swipe 196 300 196 700 --duration 0.3 --udid $U; sleep 2; shot 74-call-pulled-down
idb ui tap 196 788 --udid $U; sleep 1.5; shot 75-call-pulled-down-keyboard
idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
for e in json.load(sys.stdin):
    if e.get("AXLabel"): f=e["frame"]; print(e["type"], repr(e["AXLabel"][:50]), int(f["x"]+f["width"]/2), int(f["y"]+f["height"]/2))' | head -14

#!/bin/bash
# Cycle 40: Stop on the phone (M-130) against the #374 kernel already
# serving kernel-stall on the local hub (started by mac-stall.sh).
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
U=B1185668-7488-420F-B12D-4412BAAC7673; B=com.unarbos.arbos.ios; O=$HOME/mobile-out/cycle-40
shot() { xcrun simctl io "$U" screenshot "$O/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
els() { idb ui describe-all --udid $U 2>/dev/null; }
pos() { els | python3 -c '
import sys, json
key = sys.argv[1]
for e in json.load(sys.stdin):
    if str(e.get("AXLabel") or "") == key or key in str(e.get("AXLabel") or ""):
        f = e["frame"]; print(int(f["x"] + f["width"] / 2), int(f["y"] + f["height"] / 2)); break
' "$1"; }
idb connect $U >/dev/null 2>&1
xcrun simctl terminate $U $B 2>/dev/null; sleep 1
xcrun simctl launch --console-pty $U $B -hubURL ws://127.0.0.1:7780 -hubToken ${LOCAL_HUB_CLIENT_TOKEN:?the loopback hub fixture client token} > $O/stop-console.log 2>&1 &
sleep 8
R=$(pos "kernel-stall"); echo "row at ${R:-none}"; idb ui tap $R --udid $U; sleep 4
idb ui tap 200 788 --udid $U; sleep 1
idb ui text "Run exactly this with your bash tool and nothing else first: sleep 60. Then reply with one word: done." --udid $U; sleep 0.5; idb ui key 40 --udid $U
sleep 6; idb ui tap 196 300 --udid $U; sleep 1.5   # keyboard away
shot t-01-working-with-stop
S=$(pos "Stop"); echo "stop button at ${S:-none}"
els | python3 -c '
import sys, json
for e in json.load(sys.stdin):
    l=str(e.get("AXLabel") or "")
    if e.get("type")=="Button" and l in ("Stop","Microphone","Send"): f=e["frame"]; print(e["type"], repr(l), int(f["x"]), int(f["y"]))'
[ -n "$S" ] && idb ui tap $S --udid $U
sleep 4; shot t-02-after-stop
els | python3 -c '
import sys, json
for e in json.load(sys.stdin):
    l=str(e.get("AXLabel") or "")
    if any(k in l for k in ("Stopped","Working","interrupt","Worked")): print(e["type"], repr(l[:160]))'
grep -iE "interrupt|stop" ~/kstall.log | tail -3 | cut -c1-160

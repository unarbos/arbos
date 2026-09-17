#!/bin/bash
# Cycle 40: the kernel's stall notice (#374) on the phone. A kernel built
# from #374 serves a scratch place on the Mac's local hub with
# ARBOS_STALL_SECS=20; the app attaches through launch args only (the
# baked build stays on the ArbosLife hub); a `sleep 60` command makes the
# turn silent; the notice must appear as an ordinary muted line, whole.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
source ~/.op-env
U=B1185668-7488-420F-B12D-4412BAAC7673; B=com.unarbos.arbos.ios; O=$HOME/mobile-out/cycle-40; mkdir -p $O
K=$HOME/arbos-k342/target/release/arbos-kernel
export OPENROUTER_API_KEY="$(op read 'op://Arbos/sjcqhq3pt73chkklvzoc4uq23i/credential')"
[ -n "$OPENROUTER_API_KEY" ] || { echo "no openrouter key"; exit 1; }
shot() { xcrun simctl io "$U" screenshot "$O/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
labels() { idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
for e in json.load(sys.stdin):
    l=e.get("AXLabel") or ""
    if any(k in l for k in ("Still working","Waiting on","Working","waiting on","Stop")): print(e["type"], repr(l[:220]))'; }
tmux kill-session -t kstall 2>/dev/null; pkill -f "kernel-stall" 2>/dev/null; rm -rf ~/kernel-stall; mkdir -p ~/kernel-stall; sleep 1
: > ~/kstall.log
tmux new-session -d -s kstall "cd ~ && ARBOS_STALL_SECS=20 ARBOS_HUB_TOKEN=${LOCAL_HUB_MACHINE_TOKEN:?the loopback hub fixture machine token} OPENROUTER_API_KEY='$OPENROUTER_API_KEY' $K serve ~/kernel-stall --bind 127.0.0.1:7781 --hub ws://127.0.0.1:7780 --machine awsmac --project stallproj 2>&1 | tee -a ~/kstall.log"
sleep 6; tail -2 ~/kstall.log | cut -c1-160
idb connect $U >/dev/null 2>&1
xcrun simctl terminate $U $B 2>/dev/null; sleep 1
xcrun simctl launch --console-pty $U $B -hubURL ws://127.0.0.1:7780 -hubToken ${LOCAL_HUB_CLIENT_TOKEN:?the loopback hub fixture client token} > $O/stall-console.log 2>&1 &
sleep 8; shot s-00-list
R=$(idb ui describe-all --udid $U 2>/dev/null | python3 -c '
import sys, json
for e in json.load(sys.stdin):
    if e.get("type")=="Button" and str(e.get("AXLabel") or "").startswith("kernel-stall"):
        f=e["frame"]; print(int(f["x"]+f["width"]/2), int(f["y"]+f["height"]/2)); break')
echo "row at ${R:-none}"; [ -n "$R" ] || { shot s-01-no-row; exit 1; }
idb ui tap $R --udid $U; sleep 4; shot s-01-open
idb ui tap 200 788 --udid $U; sleep 1
idb ui text "Run exactly this with your bash tool and nothing else first: sleep 60. Then reply with one word: done." --udid $U; sleep 0.5; idb ui key 40 --udid $U
T0=$(date +%s)
for i in $(seq 1 24); do
  sleep 5
  if labels | grep -q "Still working"; then echo "stall notice on the phone after $(( $(date +%s) - T0 )) s"; break; fi
done
idb ui tap 196 300 --udid $U; sleep 1   # keyboard away so the line is in view
shot s-02-stall-notice; labels
sleep 45; shot s-03-turn-done; labels
grep -E "turn_stalled|notice" ~/kstall.log | tail -3 | cut -c1-200

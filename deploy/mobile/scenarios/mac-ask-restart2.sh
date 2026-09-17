#!/bin/bash
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
source ~/.op-env
U=B1185668-7488-420F-B12D-4412BAAC7673; O=$HOME/mobile-out/cycle-23
K=$HOME/arbos-k342/target/release/arbos-kernel
export OPENROUTER_API_KEY="$(op read 'op://Arbos/sjcqhq3pt73chkklvzoc4uq23i/credential')"
shot() { xcrun simctl io "$U" screenshot "$O/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
start_k() { tmux new-session -d -s kask "cd ~ && ARBOS_HUB_TOKEN=${LOCAL_HUB_MACHINE_TOKEN:?the loopback hub fixture machine token} OPENROUTER_API_KEY='$OPENROUTER_API_KEY' $K serve ~/kernel-ask --bind 127.0.0.1:7779 --hub ws://127.0.0.1:7780 --machine awsmac --project askproj 2>&1 | tee -a ~/kask.log"; }
xcrun simctl terminate $U com.unarbos.arbos.ios 2>/dev/null; sleep 1; xcrun simctl launch --console-pty $U com.unarbos.arbos.ios -hubURL ws://127.0.0.1:7780 -hubToken ${LOCAL_HUB_CLIENT_TOKEN:?the loopback hub fixture client token} > $O/ask-restart-console.log 2>&1 & sleep 8; idb ui tap 120 308 --udid $U; sleep 5; shot r-01-open
idb ui tap 200 788 --udid $U; sleep 0.8
idb ui text "RESTART-ASK3 $(date -u +%H%M%S): use your ask tool to ask me which animal I like best, with the options Cat, Dog, Owl. Then reply with exactly the animal I chose." --udid $U; idb ui key 40 --udid $U
export KERNEL_HUB=ws://127.0.0.1:7780 KERNEL_TOKEN=${LOCAL_HUB_CLIENT_TOKEN:?the loopback hub fixture client token}
t=0; while [ $t -lt 90 ]; do python3 ~/kernel.py awsmac/askproj history 6 2>/dev/null | grep -qE "^ *[0-9]+ ask " && break; sleep 3; t=$((t+3)); done; sleep 4; shot r-02-question-open
echo "$(date -u +%H:%M:%S) killing the kernel with the question open"; tmux kill-session -t kask 2>/dev/null; pkill -9 -f "kernel-ask" 2>/dev/null; sleep 8; shot r-03-kernel-dead
echo "$(date -u +%H:%M:%S) new kernel on the same place"; start_k; sleep 40; shot r-04-reattached; sleep 15; shot r-05-reattached-later
grep -c "asks_replayed" ~/kask.log; grep "asks_replayed" ~/kask.log | tail -1
# answer from the re-offered card: tap the first chip (found by colour band under the question)
python3 - <<'PY'
from PIL import Image
im=Image.open("/Users/ec2-user/mobile-out/cycle-23/r-05-reattached-later.png").convert("RGB")
W,H=im.size
# chips are the light boxes (~#1a1a1a on #0e0e0e bg); find rows with several chip-coloured runs in the lower half
rows=[]
for y in range(H//3, H-500, 3):
    xs=[x for x in range(60,W-60,4) if 24<=im.getpixel((x,y))[0]<=34 and abs(im.getpixel((x,y))[0]-im.getpixel((x,y))[2])<3]
    if len(xs)>25: rows.append(y)
print("chip rows", rows[:3], rows[-3:] if rows else None)
if rows:
    y=(rows[0]+rows[-1])//2
    print("TAP", 60//3+10, y//3)
PY

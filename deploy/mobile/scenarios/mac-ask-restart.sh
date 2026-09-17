#!/bin/bash
# #342 on the phone: a question open, the kernel killed, a new kernel on the same place — does the card come back answerable?
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
source ~/.op-env
U=B1185668-7488-420F-B12D-4412BAAC7673; B=com.unarbos.arbos.ios; O=$HOME/mobile-out/cycle-23; mkdir -p $O
K=$HOME/arbos-k342/target/release/arbos-kernel
export OPENROUTER_API_KEY="$(op read 'op://Arbos/sjcqhq3pt73chkklvzoc4uq23i/credential')"
[ -n "$OPENROUTER_API_KEY" ] || { echo "no openrouter key"; exit 1; }
shot() { xcrun simctl io "$U" screenshot "$O/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
start_k() { tmux new-session -d -s kask "cd ~ && ARBOS_HUB_TOKEN=${LOCAL_HUB_MACHINE_TOKEN:?the loopback hub fixture machine token} OPENROUTER_API_KEY='$OPENROUTER_API_KEY' $K serve ~/kernel-ask --bind 127.0.0.1:7779 --hub ws://127.0.0.1:7780 --machine awsmac --project askproj 2>&1 | tee -a ~/kask.log"; }
tmux kill-session -t kask 2>/dev/null; pkill -f "kernel-ask" 2>/dev/null; rm -rf ~/kernel-ask; mkdir -p ~/kernel-ask; sleep 1
: > ~/kask.log; start_k; sleep 6; tail -3 ~/kask.log
# the app on the local hub (launch args only; the baked build stays on the pod hub)
xcrun simctl terminate $U $B 2>/dev/null; sleep 1
xcrun simctl launch --console-pty $U $B -hubURL ws://127.0.0.1:7780 -hubToken ${LOCAL_HUB_CLIENT_TOKEN:?the loopback hub fixture client token} > $O/ask-restart-console.log 2>&1 &
sleep 8; shot r-00-list
python3 - <<'PY'
from PIL import Image
im=Image.open("/Users/ec2-user/mobile-out/cycle-23/r-00-list.png"); print("list", im.size)
PY

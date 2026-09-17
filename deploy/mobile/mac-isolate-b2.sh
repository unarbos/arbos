#!/bin/bash
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
U=B1185668-7488-420F-B12D-4412BAAC7673; O=$HOME/mobile-out/cycle-12
K=$HOME/arbos/target/release/arbos-kernel
shot() { xcrun simctl io "$U" screenshot "$O/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
start1() { tmux new-session -d -s kern "cd ~ && ARBOS_HUB_TOKEN=${LOCAL_HUB_MACHINE_TOKEN:?the loopback hub fixture machine token} $K serve ~/kernel-place --bind 127.0.0.1:7777 --provider replay --replies ~/replies.jsonl --hub ws://127.0.0.1:7780 --machine awsmac --project longproj 2>&1 | tee ~/kern.log"; }
MARK="ISOLATION-B2 $(date -u +%H%M%S)"; echo "marker: $MARK" | tee $O/b2-marker.txt
idb ui tap 120 308 --udid $U; sleep 5; shot b-01-longproj-open
tmux kill-session -t kern 2>/dev/null; pkill -f "kernel-place " 2>/dev/null; echo "$(date -u +%H:%M:%S) longproj kernel killed"
sleep 6; shot b-02-longproj-link-lost
idb ui tap 200 788 --udid $U; sleep 1
idb ui text "$MARK: reply with exactly LEAKED-B" --udid $U; idb ui key 40 --udid $U; sleep 12
shot b-03-longproj-pending
idb ui tap 42 85 --udid $U; sleep 2; shot b-04-list
shot b-04b-list-after-kill; idb ui tap 120 308 --udid $U; sleep 4; shot b-05-otherproj-open
start1; echo "$(date -u +%H:%M:%S) longproj kernel back"
sleep 40; shot b-06-otherproj-after
idb ui tap 42 85 --udid $U; sleep 3; shot b-07-list-again
echo "--- otherproj transcript mentions of the marker (must be 0):"; grep -c "ISOLATION-B2" ~/kernel-place2/.arbos/agents/root/transcript.jsonl 2>/dev/null || echo 0
echo "--- longproj transcript mentions:"; grep -c "ISOLATION-B2" ~/kernel-place/.arbos/agents/root/transcript.jsonl 2>/dev/null || echo 0
echo "--- otherproj kernel log mentions:"; grep -c "ISOLATION-B2" ~/kern2.log || true

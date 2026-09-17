#!/bin/bash
# Proof B: the kernel dies (not the network). Lines typed into longproj while
# its kernel is dead must never run in otherproj. Local hub, two kernels.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
U=B1185668-7488-420F-B12D-4412BAAC7673; O=$HOME/mobile-out/cycle-12; B=com.unarbos.arbos.ios
K=$HOME/arbos/target/release/arbos-kernel
shot() { xcrun simctl io "$U" screenshot "$O/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
start1() { tmux new-session -d -s kern "cd ~ && ARBOS_HUB_TOKEN=${LOCAL_HUB_MACHINE_TOKEN:?the loopback hub fixture machine token} $K serve ~/kernel-place --bind 127.0.0.1:7777 --provider replay --replies ~/replies.jsonl --hub ws://127.0.0.1:7780 --machine awsmac --project longproj 2>&1 | tee ~/kern.log"; }
start2() { tmux new-session -d -s kern2 "cd ~ && ARBOS_HUB_TOKEN=${LOCAL_HUB_MACHINE_TOKEN:?the loopback hub fixture machine token} $K serve ~/kernel-place2 --bind 127.0.0.1:7778 --provider replay --replies ~/replies.jsonl --hub ws://127.0.0.1:7780 --machine awsmac --project otherproj 2>&1 | tee ~/kern2.log"; }
mkdir -p ~/kernel-place2
tmux kill-session -t kern2 2>/dev/null; pkill -f "kernel-place2" 2>/dev/null; sleep 1
tmux has-session -t kern 2>/dev/null || start1
start2; sleep 4
MARK="ISOLATION-B $(date -u +%H%M%S)"; echo "marker: $MARK" | tee $O/b-marker.txt
xcrun simctl terminate "$U" $B 2>/dev/null; sleep 2
xcrun simctl launch --console-pty "$U" $B -hubURL ws://127.0.0.1:7780 -hubToken ${LOCAL_HUB_CLIENT_TOKEN:?the loopback hub fixture client token} > "$O/b-console.log" 2>&1 &
sleep 7; shot b-00-list

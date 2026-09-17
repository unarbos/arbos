#!/bin/bash
# Against the Mac's own kernel (#270 + #272) through a local hub: paging
# back, a photo that lands, one calm line on a lost link.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
OUT="$HOME/mobile-out/cycle-6"; UDID=$(cut -d' ' -f2 "$OUT/sim.txt"); B=com.unarbos.arbos.ios
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
idb connect "$UDID" >/dev/null 2>&1
xcrun simctl terminate "$UDID" $B 2>/dev/null || true
sleep 3
xcrun simctl spawn "$UDID" defaults write $B hubURL ws://127.0.0.1:7780
sleep 1
xcrun simctl spawn "$UDID" defaults read $B hubURL
xcrun simctl launch --console-pty "$UDID" $B -hubURL ws://127.0.0.1:7780 -hubToken ${LOCAL_HUB_CLIENT_TOKEN:?the loopback hub fixture client token} -dictateWav "$HOME/mobile-clips/note.wav" > "$OUT/local-console.log" 2>&1 &
sleep 6; shot f-01-list
# longproj row: find it by tapping the first row under the local hub's section; take a still first
idb ui tap 120 291 --udid "$UDID"; sleep 5; shot f-02-open
# to the top: several fast swipes down
for i in 1 2 3 4 5 6 7 8 9 10 11 12; do idb ui swipe 196 250 196 800 --duration 0.05 --udid "$UDID"; done
sleep 1.5; shot f-03-top
idb ui tap 196 150 --udid "$UDID"; sleep 3; shot f-04-earlier-loaded
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14; do idb ui swipe 196 250 196 800 --duration 0.05 --udid "$UDID"; done
sleep 1.5; shot f-05-top-again
idb ui tap 196 150 --udid "$UDID"; sleep 3; shot f-06-top-of-history
# back to the bottom
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28; do idb ui swipe 196 800 196 200 --duration 0.05 --udid "$UDID"; done
sleep 1.5
# photo + line
idb ui tap 41 788 --udid "$UDID"; sleep 1.5
idb ui tap 160 767 --udid "$UDID"; sleep 4
idb ui tap 70 230 --udid "$UDID"; sleep 1
idb ui tap 355 131 --udid "$UDID"; sleep 3; shot f-07-chip
idb ui tap 200 780 --udid "$UDID"; sleep 1
idb ui text "What is in this photo?" --udid "$UDID"; sleep 0.5
idb ui key 40 --udid "$UDID"; sleep 8; shot f-08-photo-sent
ls -la ~/kernel-place/.arbos/attachments/ 2>/dev/null
tail -3 ~/kernel-place/.arbos/agents/root/transcript.jsonl | cut -c1-260
# lost link: kill the kernel; one calm line
tmux kill-session -t kern 2>/dev/null; sleep 5; shot f-09-link-lost
tmux new-session -d -s kern "cd ~ && ARBOS_HUB_TOKEN=${LOCAL_HUB_MACHINE_TOKEN:?the loopback hub fixture machine token} ~/arbos/target/release/arbos-kernel serve ~/kernel-place --bind 127.0.0.1:7777 --provider replay --replies ~/replies.jsonl --hub ws://127.0.0.1:7780 --machine awsmac --project longproj 2>&1 | tee ~/kern.log"
sleep 25; shot f-10-back
grep -E "written|history_end|earlier" "$OUT/local-console.log" | head -6

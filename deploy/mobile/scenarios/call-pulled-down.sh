#!/bin/bash
# The call screen pulled down: type, mute, close — and where close lands.
#
#   call-pulled-down.sh <cycle> [project]
#
# Cycle 38's row says close "returns to the chat". Cycle 56 saw it return to
# the projects list. Both can be true: 56 entered the call by launch
# argument, with no chat behind it. This enters the way a person does —
# from the project's own chat — so the answer is about the real path.
#
# Also checks what the orb says with no microphone at all, which is the
# simulator's normal state and the one a scripted run meets first.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/call-pulled-down"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
hist() { python3 "$HERE/../kernel.py" pod history 30 2>/dev/null; }
where() { ui dump | grep -oE "StaticText +Projects|StaticText +$ROW|Button +Back" | head -1; }

BEFORE=$(hist | tail -1 | awk '{print $1}')

echo "== into the call from the project's own chat =="
xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
# No -injectWav: the simulator has no microphone, which is claim one.
xcrun simctl launch --console-pty "$UDID" $B -noAskNotifications 1 > "$OUT/console.log" 2>&1 &
sleep 8
ui tap "$ROW" >/dev/null || { echo "no $ROW row"; exit 1; }
sleep 4
echo "  before the call, we are on: $(where)"
ui menu >/dev/null || { echo "no chat menu"; exit 1; }
sleep 1.5
ui tap "Call $ROW" >/dev/null || { echo "no Call $ROW"; exit 1; }
sleep 6; shot 01-call-no-microphone
echo "  what the orb says: $(ui dump | grep -E "StaticText" | awk '{$1="";$2="";$3="";print}' | tr '\n' ';')"

echo
echo "== pulled down =="
idb ui swipe 196 300 196 700 --duration 0.4 --udid "$UDID"; sleep 1.5
shot 02-pulled-down
echo "  the row it shows: $(ui dump | awk '$2 > 700' | tr '\n' ';')"

echo
echo "== typing on the call =="
# Plain ASCII only. `idb ui text` maps characters to key codes and throws
# "No keycode found" on anything outside the keyboard — an em dash in this
# very line cost a run.
LINE="typed on the call at $(date -u +%H%M%S), reply with the single word heard"
ui focus >/dev/null 2>&1 || { echo "  no composer on the pulled-down call"; }
sleep 0.7
idb ui text "$LINE" --udid "$UDID"
for _ in $(seq 1 60); do [ "$(ui field plain 2>/dev/null)" = "$LINE" ] && break; sleep 0.25; done
ui tap "Up" >/dev/null 2>&1 || idb ui key 40 --udid "$UDID"
sleep 12; shot 03-typed-and-answered
echo "  did it reach the kernel: $(hist | awk -v a="${BEFORE:-0}" '$1+0 > a+0' | grep -c "typed on the call")"
hist | awk -v a="${BEFORE:-0}" '$1+0 > a+0' | tail -2 | cut -c1-140 | sed 's/^/    /'

echo
echo "== mute, then close =="
ui tap "Microphone" >/dev/null 2>&1 && echo "  muted" || echo "  no mic button to mute"
sleep 1; shot 04-muted
ui tap "Close" >/dev/null || { echo "  no close button"; exit 1; }
sleep 4; shot 05-after-close
echo "  close landed on: $(where)"
echo "stills in $OUT"

#!/bin/bash
# The four rows that had been sitting at cycle 37, measured rather than
# eyeballed:
#
#   1. cold start — launch to a list with rows in it
#   2. away and back — the chat as it was, after eight seconds elsewhere
#   3. long history — the chat opening at the tail, and paging back
#   4. attachments — a photo chip in the field, and the send arrow
#
#   cold-start-and-history.sh <cycle> [project]
#
# Every number here is a poll of the accessibility tree, not a stopwatch on a
# screenshot: the tree is what says the rows exist.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/cold-start"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }
now() { python3 -c 'import time;print(time.time())'; }
since() { python3 -c "import time;print(f'{time.time()-$1:.1f}')"; }

# Wait until `pattern` is in the tree; print the seconds it took, or "never".
wait_for() {
  local pattern=$1 limit=${2:-30} t0=$3
  while [ "$(python3 -c "import time;print(int(time.time()-$t0))")" -lt "$limit" ]; do
    ui dump 2>/dev/null | grep -qE "$pattern" && { since "$t0"; return 0; }
  done
  echo never; return 1
}

echo "== 1. cold start =="
xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 2
T0=$(now)
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
FIRST=$(wait_for "Button +[a-z0-9-]+, (Idle|Working)" 40 "$T0")
echo "  launch to a list with rows: ${FIRST}s   (cycle 37 measured 3.1s)"
shot 01-cold-start
echo "  rows: $(ui dump | grep -cE 'Button +[a-z0-9-]+, (Idle|Working)')"

echo
echo "== 3. long history =="
T0=$(now)
ui tap "$ROW" >/dev/null || { echo "  no $ROW row"; exit 1; }
OPEN=$(wait_for "TextField" 30 "$T0")
echo "  tap to a chat with a composer: ${OPEN}s   (cycle 37 measured 1.1s to the tail)"
shot 02-chat-tail
echo "  transcript lines the kernel holds: $(python3 "$HERE/../kernel.py" pod total 2>/dev/null || echo '?')"
# Fling to the top of the loaded window. A fixed number of flings is not
# enough and does not know it: six reported "pager: not found" on a window
# of two hundred long replies that thirty reached easily. Fling until the
# pager appears or the transcript stops moving.
PAGER=""; LAST=""
for i in $(seq 1 60); do
  idb ui swipe 196 250 196 820 --duration 0.15 --udid "$UDID"
  if [ $((i % 5)) = 0 ]; then
    PAGER=$(ui dump | grep -oE "Show [0-9]+ earlier lines")
    [ -n "$PAGER" ] && { echo "  pager after $i flings: $PAGER"; break; }
    HERE_TOP=$(ui dump | grep -m1 StaticText)
    [ "$HERE_TOP" = "$LAST" ] && { echo "  the transcript stopped moving after $i flings, and no pager"; break; }
    LAST=$HERE_TOP
  fi
done
sleep 1; shot 03-scrolled-back
[ -n "$PAGER" ] || echo "  pager: never appeared"
if [ -n "$PAGER" ]; then
  T0=$(now)
  ui tap "$PAGER" >/dev/null && sleep 3
  echo "  paged in $(since "$T0")s; the tail of the window is now $(ui dump | grep -m1 -oE 'harness probe [0-9-]+|[A-Z][a-z]+ [0-9]{6}' || echo 'older content')"
  shot 04-paged-back
fi

echo
echo "== 2. away and back =="
BEFORE=$(ui dump | grep -cE "StaticText")
idb ui button HOME --udid "$UDID"; sleep 8
xcrun simctl launch "$UDID" $B >/dev/null 2>&1; sleep 3
AFTER=$(ui dump | grep -cE "StaticText")
echo "  text rows before: $BEFORE, after: $AFTER"
shot 05-back-from-away
ui dump | head -5 | sed 's/^/  /'

echo
echo "== 4. attachments =="
ui tap "Add" >/dev/null 2>&1 && sleep 1.5 && ui tap "Photo Library" >/dev/null 2>&1 && sleep 4 || echo "  could not open the picker"
. "$HERE/../sim-lib.sh"
tap_shot 78 470 "$UDID"; sleep 1
tap_shot 426 157 "$UDID"; sleep 3
shot 06-photo-chip
echo "  composer row: $(ui dump | awk '$2 > 700 && $2 < 900' | tr '\n' ';')"
echo "stills in $OUT"

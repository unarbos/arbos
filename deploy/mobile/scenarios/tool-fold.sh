#!/bin/bash
# Tool calls fold into one line, and the line opens.
#
#   tool-fold.sh <cycle> [project]
#
# This came from Jacob, build 956: "I don't want to see these tool calls on
# the phone unless maybe I expand" (F13). Consecutive calls became one dim
# line — "4 tool calls · 12s", failures counted — that opens on a tap.
#
# Nothing has checked it since. `fold37.sh` is cycle-37 scratch: it reads
# `~/mobile-out/cycle-37` and takes its tools from `$HOME`, so it cannot run
# today and would not be measuring this if it did.
#
# Two claims, and the second is the one with a person behind it: the calls
# fold, **and the fold opens**. A fold that cannot be opened is not tidier
# than the old list, it is the same information hidden.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/tool-fold"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
rows() { ui dump | grep -cE "StaticText"; }
foldline() { ui dump | grep -oE "[0-9]+ tool calls?[^\"]*" | tail -1; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
sleep 11
reach_the_list "$UDID" || exit 1
ui tap "$ROW" >/dev/null 2>&1 || { echo "no '$ROW' row"; exit 1; }
sleep 5
ui field >/dev/null 2>&1 || { echo "no composer in '$ROW'"; exit 1; }

MARK="fold $(date -u +%H%M%S)"
echo "== give it several tool calls to fold =="
ui focus >/dev/null; sleep 0.7
idb ui text "$MARK: run these three bash commands one after another and nothing else — echo one, echo two, echo three — then reply done." --udid "$UDID" >/dev/null 2>&1
sleep 3
ui tap "Send" >/dev/null 2>&1

FOLD=""
for _ in $(seq 1 60); do
  FOLD=$(foldline)
  [ -n "$FOLD" ] && break
  sleep 2
done
xcrun simctl io "$UDID" screenshot "$OUT/01-folded.png" >/dev/null 2>&1
if [ -z "$FOLD" ]; then
  echo "  no fold line appeared"
  echo
  echo "VERDICT: none — the turn made no folded tool calls, so there was"
  echo "         nothing to open and this says nothing about folding"
  exit 1
fi
echo "  the fold reads: $FOLD"
CLOSED=$(rows)
echo "  text rows while closed: $CLOSED"

echo
echo "== open it =="
# Tapped by the words it shows, not by a position: the fold moves as the
# transcript grows underneath it.
ui tap "$(echo "$FOLD" | grep -oE "^[0-9]+ tool calls?")" >/dev/null 2>&1
sleep 3
OPEN=$(rows)
xcrun simctl io "$UDID" screenshot "$OUT/02-open.png" >/dev/null 2>&1
echo "  text rows while open:   $OPEN"

echo
echo "== close it again =="
ui tap "$(echo "$FOLD" | grep -oE "^[0-9]+ tool calls?")" >/dev/null 2>&1
sleep 3
AGAIN=$(rows)
echo "  text rows closed again: $AGAIN"

echo
FAULTS=0
[ "$OPEN" -gt "$CLOSED" ] || { echo "  FAULT: opening the fold showed nothing more ($CLOSED → $OPEN)"; FAULTS=$((FAULTS + 1)); }
[ "$AGAIN" -le "$CLOSED" ] || { echo "  FAULT: closing it left $((AGAIN - CLOSED)) row(s) behind"; FAULTS=$((FAULTS + 1)); }
if [ "$FAULTS" = 0 ]; then
  echo "VERDICT: '$FOLD' folds $((OPEN - CLOSED)) row(s) away and gives them back on a tap"
else
  echo "VERDICT: $FAULTS fault(s) in the fold — named above"
fi
echo "stills in $OUT"

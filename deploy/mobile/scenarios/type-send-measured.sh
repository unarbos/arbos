#!/bin/bash
# Why the journey's typed lines go missing (M-162), counted.
#
#   type-send-measured.sh [trials]
#
# In the cycle-48 journey 6 of 8 typed lines never reached the kernel, which
# invalidated most of the run. This measures the harness rather than
# reasoning about it, and stops short of sending — nothing here presses
# send, so no kernel is woken and nothing downstream can be mistaken for
# this fault.
#
# Two questions, because there turned out to be two faults:
#
#   1. does the journey's fixed tap point still find the composer once the
#      keyboard is up?
#   2. how long after `idb ui text` returns do its characters arrive?
#
# The read-back needs the keyboard down: while it is up `describe-all`
# returns the keyboard's own tree and the composer is not in it. Tapping the
# chat body puts it away without sending and without losing what was typed.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
N=${1:-4}
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

# The journey's lines are long. A short probe would pass where they fail, so
# this is the length of the shortest of them.
LINE="probe: fix the failing test, add perimeter(w, h) and diagonal(w, h) with three unit tests each, and commit the work on a branch."
CHARS=${#LINE}

away() { idb ui tap 196 150 --udid "$UDID" >/dev/null 2>&1; sleep 1; }
focus() { ui focus >/dev/null 2>&1; }

clear_field() {
  local n
  away
  n=$(ui field 2>/dev/null | wc -c | tr -d ' ')
  [ "$n" -gt 0 ] || return 0
  focus; sleep 0.6
  # A generous over-count of backspaces: deleting an empty field is a no-op.
  idb ui key-sequence $(for _ in $(seq 1 $((n + 20))); do printf '42 '; done) --udid "$UDID" >/dev/null 2>&1
  sleep 1
  away
}

echo "== 1. where the journey's fixed tap point lands =="
clear_field
echo "keyboard down, the composer sits at:"
ui dump | awk '$3 == "TextField" { print "  " $1 " " $2 }'
focus; sleep 1
echo "keyboard up, what covers 200,788:"
ui dump | awk '$2 > 745 && $2 < 800 && $1 > 150 && $1 < 260 { print "  " $0 }'
away

echo
echo "== 2. how long the characters take to arrive =="
echo "line is $CHARS characters"
for wait in 0.3 1 2 4; do
  whole=0
  for _ in $(seq 1 "$N"); do
    clear_field
    focus; sleep 0.8
    idb ui text "$LINE" --udid "$UDID" >/dev/null 2>&1
    sleep "$wait"
    away
    [ "$(ui field 2>/dev/null)" = "$LINE" ] && whole=$((whole+1))
  done
  echo "  waited ${wait}s after idb returned: $whole/$N whole"
done
clear_field
echo "field left empty; nothing was sent"

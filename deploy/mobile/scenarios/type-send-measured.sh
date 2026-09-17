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
#   1. is the composer still where the journey taps for it?
#   2. how long after `idb ui text` returns do its characters arrive?
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
N=${1:-4}
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

# The journey's lines are long. A short probe would settle where they do not,
# so this is the length of the shortest of them.
LINE="probe: fix the failing test, add perimeter(w, h) and diagonal(w, h) with three unit tests each, and commit the work on a branch."
CHARS=${#LINE}

where() { ui dump | awk '$3 == "TextField" { print $1 " " $2 }'; }
clear_field() {
  local n
  n=$(ui field 2>/dev/null | wc -c | tr -d ' ')
  [ "${n:-0}" -gt 0 ] || return 0
  ui focus >/dev/null 2>&1; sleep 0.6
  idb ui key-sequence $(for _ in $(seq 1 $((n + 20))); do printf '42 '; done) --udid "$UDID" >/dev/null 2>&1
  sleep 1
}

echo "== 1. the composer does not stay put =="
clear_field
echo "  empty, keyboard down: $(where)"
ui focus >/dev/null; sleep 1
echo "  empty, keyboard up:   $(where)"
idb ui text "$LINE" --udid "$UDID" >/dev/null 2>&1; sleep 4
echo "  full, keyboard up:    $(where)"
echo "  the journey taps 200 788 every time"

echo
echo "== 2. the characters arrive after idb returns =="
echo "line is $CHARS characters"
for _ in $(seq 1 "$N"); do
  clear_field
  ui focus >/dev/null; sleep 0.8
  start=$(python3 -c 'import time;print(time.time())')
  idb ui text "$LINE" --udid "$UDID" >/dev/null 2>&1
  at_return=$(ui field 2>/dev/null); at_return=${#at_return}
  settled=""
  for _ in $(seq 1 60); do
    if [ "$(ui field 2>/dev/null)" = "$LINE" ]; then
      settled=$(python3 -c "import time;print(round(time.time()-$start,1))")
      break
    fi
    sleep 0.25
  done
  echo "  $at_return/$CHARS characters when idb returned; whole after ${settled:-never}s"
done
clear_field
echo "field left empty; nothing was sent"

#!/bin/bash
# Why the journey's typed lines went missing (M-162), counted.
#
#   type-send-measured.sh old|new [lines]
#
# In the cycle-48 journey 6 of 8 typed lines never reached the kernel, which
# invalidated most of the run, and the journey scored the app for it. This
# sends the same kind of line the journey sends and counts how many the
# kernel actually received, the old way and the new one.
#
# Run it with the app on a project chat. Each line asks for nothing, but it
# is a real turn, so the kernel does wake.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
WAY=${1:?old or new}; N=${2:-4}
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
len() { local v; v=$(ui field 2>/dev/null); echo ${#v}; }
hist() { python3 "$HERE/../kernel.py" pod history 200 2>/dev/null; }

clear_field() {
  local n
  for _ in 1 2 3 4 5; do
    n=$(len); [ "$n" -gt 0 ] || return 0
    ui focus >/dev/null 2>&1; sleep 0.6
    idb ui key-sequence $(for _ in $(seq 1 $((n + 5))); do printf '42 '; done) --udid "$UDID" >/dev/null 2>&1
    sleep 0.8
  done
}

# What the journey did until cycle 54.
old_send() {
  idb ui tap 200 788 --udid "$UDID" >/dev/null 2>&1
  sleep 0.8
  idb ui text "$1" --udid "$UDID" >/dev/null 2>&1
  sleep 0.3
  idb ui key 40 --udid "$UDID" >/dev/null 2>&1
}

# Find the box, put the caret past everything in it, type, read it back, and
# only then send — by the button rather than by a key code.
new_send() {
  local want=$1 got
  for _ in 1 2 3; do
    clear_field
    ui focus >/dev/null 2>&1 || { sleep 1; continue; }
    sleep 0.7
    idb ui text "$want" --udid "$UDID" >/dev/null 2>&1
    for _ in $(seq 1 40); do [ "$(ui field plain 2>/dev/null)" = "$want" ] && break; sleep 0.25; done
    got=$(ui field plain 2>/dev/null)
    if [ "$got" = "$want" ]; then
      ui tap "Up" >/dev/null 2>&1 || idb ui key 40 --udid "$UDID" >/dev/null 2>&1
      return 0
    fi
    echo "    the box held ${#got} of ${#want} characters; clearing and retrying"
  done
  return 1
}

STAMP=$(date -u +%H%M%S)
echo "way=$WAY lines=$N stamp=$STAMP"
for i in $(seq 1 "$N"); do
  # The journey's lines are long. A short one lands where a long one does not.
  LINE="harness probe $STAMP-$i, no action needed: confirm you saw this line and say nothing else, it is checking whether typed lines arrive whole."
  case $WAY in
    old) old_send "$LINE" ;;
    new) new_send "$LINE" || echo "    line $i never went" ;;
    *) echo "way must be old or new"; exit 1 ;;
  esac
  sleep 12
done
sleep 20
arrived=0
echo "-- what the kernel has --"
for i in $(seq 1 "$N"); do
  if hist | grep -q "harness probe $STAMP-$i,"; then
    echo "  $i arrived"; arrived=$((arrived+1))
  else
    echo "  $i MISSING"
  fi
done
echo "$WAY: $arrived/$N arrived, $(hist | grep -c "harness probe $STAMP-.*arrive whole\.")/$N whole"

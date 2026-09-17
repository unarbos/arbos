#!/bin/bash
# How often does a typed line actually land in the composer?
#
#   type-send-measured.sh <trials>
#
# M-162: in the cycle-48 journey, 6 of 8 typed lines never reached the
# kernel, which invalidated most of the run. The journey blamed the app. This
# counts the harness instead, and it separates the two things `type_send`
# does — putting the caret in the field, and getting the characters in — by
# reading the field back before pressing return.
#
# Nothing here presses return, so no kernel is woken and the count is not
# confused by anything downstream.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
N=${1:-10}
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

# The journey's real lines are long. A short probe would pass where they fail,
# so the probe is the length of the shortest of them.
LINE="probe: in mathlib/ fix the failing test, add perimeter(w, h) and diagonal(w, h) with three unit tests each, and commit on a branch."
CHARS=${#LINE}

clear_field() {
  # Select all and delete, so a trial never inherits the last one's text.
  idb ui key 40 --udid "$UDID" >/dev/null 2>&1   # dismiss anything transient
  for _ in $(seq 1 6); do idb ui key-sequence 42 --udid "$UDID" >/dev/null 2>&1; done
}

trial() {
  local how=$1 landed
  case $how in
    pixel) idb ui tap 200 788 --udid "$UDID" >/dev/null 2>&1 ;;
    label) ui tap "Follow up" >/dev/null 2>&1 || ui tap "Message" >/dev/null 2>&1 ;;
  esac
  sleep 0.8
  idb ui text "$LINE" --udid "$UDID" >/dev/null 2>&1
  sleep 0.3
  landed=$(ui value "Follow up" 2>/dev/null || ui value "Message" 2>/dev/null)
  printf '%s %s %s\n' "$how" "${#landed}" "$([ "$landed" = "$LINE" ] && echo whole || echo SHORT)"
}

echo "line is $CHARS characters; $N trials each way"
for how in pixel label; do
  whole=0
  for i in $(seq 1 "$N"); do
    out=$(trial "$how")
    echo "  $i $out"
    [[ $out == *whole ]] && whole=$((whole+1))
    clear_field
  done
  echo "$how: $whole/$N whole"
done

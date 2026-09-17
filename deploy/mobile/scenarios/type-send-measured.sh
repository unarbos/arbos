#!/bin/bash
# How often does a typed line actually reach the composer?
#
#   type-send-measured.sh [trials]
#
# M-162: in the cycle-48 journey, 6 of 8 typed lines never reached the
# kernel, which invalidated most of the run. This counts the harness rather
# than reasoning about it, and it stops short of sending — nothing here
# presses send, so no kernel is woken and no downstream fault can be
# mistaken for this one.
#
# The read-back needs the keyboard down, because `idb ui describe-all`
# returns the keyboard's own tree while it is up and the composer is not in
# it. Tapping the chat body puts the keyboard away without sending and
# without losing what was typed.
#
# Trials are cumulative instead of cleared between rounds: the field is read
# before and after, and a trial counts as whole when it grew by exactly the
# length of the line and ends with it. Deleting 131 characters between
# trials would cost more than it proves.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
N=${1:-6}
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

# The journey's lines are long. A short probe would pass where they fail, so
# this is the length of the shortest of them.
LINE="probe: fix the failing test, add perimeter(w, h) and diagonal(w, h) with three unit tests each, and commit the work on a branch."
CHARS=${#LINE}

# The composer is named by its placeholder in the chat and on the call
# screen; with text in it the placeholder goes and the typed text is the
# value, so both names are tried.
field() { ui value "Follow up" 2>/dev/null || ui value "Message" 2>/dev/null || ui value "Type to" 2>/dev/null; }
focus() { ui tap "Follow up" >/dev/null 2>&1 || ui tap "Message" >/dev/null 2>&1 || ui tap "Type to" >/dev/null 2>&1; }
keyboard_away() { idb ui tap 196 150 --udid "$UDID" >/dev/null 2>&1; sleep 1; }

trial() {
  local how=$1 before after grew
  keyboard_away
  before=$(field); before=${#before}
  case $how in
    # What the journey does today: one fixed point, every time.
    pixel) idb ui tap 200 788 --udid "$UDID" >/dev/null 2>&1 ;;
    # By name, with the keyboard put away first so the field is in the tree.
    label) focus ;;
  esac
  sleep 0.8
  idb ui text "$LINE" --udid "$UDID" >/dev/null 2>&1
  sleep 0.5
  keyboard_away
  after=$(field)
  grew=$(( ${#after} - before ))
  if [ "$grew" = "$CHARS" ] && [[ $after == *"$LINE" ]]; then
    printf '%s grew %s whole\n' "$how" "$grew"
  else
    printf '%s grew %s LOST\n' "$how" "$grew"
  fi
}

echo "line is $CHARS characters; $N trials each way"
for how in pixel label; do
  whole=0
  for i in $(seq 1 "$N"); do
    out=$(trial "$how")
    echo "  $i $out"
    [[ $out == *whole ]] && whole=$((whole+1))
  done
  echo "$how: $whole/$N whole"
done
echo "the field is left with the probe text in it; nothing was sent"

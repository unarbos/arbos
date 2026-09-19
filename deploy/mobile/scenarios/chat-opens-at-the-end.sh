#!/bin/bash
# COVERS: project chat — long history, scroll, older lines
# Opening a chat: does it land at the end, and is the last line clear?
#
#   chat-opens-at-the-end.sh <cycle> [project...]
#
# Two claims, and they are not the same one. Cycle 110 measured the first
# and reported the second, which is how #633 shipped a fix aimed at the
# wrong mechanism:
#
#   landing   — opening scrolls to the end of the transcript, with no
#               unused scroll left underneath. Measured by comparing where
#               the last element sits on opening against where it sits
#               after paging to the end by hand.
#   clearance — the last line is not underneath the workers pill. A chat can
#               land at its end and still be clipped, because the pill
#               floats over the transcript.
#
# The history: opening a chat with workers once stopped **501 pt** short
# (M-368) because the rows between are not realised when the first scroll
# runs; a second scroll fixes it. #633's earlier guess reserved the pill's
# height instead and left a pill-high gap. Anyone tempted to change this
# again should run this first and put the numbers in the PR.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; shift || true
ROWS=${*:-phone demo}
OUT="$HOME/mobile-out/$CYCLE/opens-at-the-end"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
# The last thing drawn in the transcript, and the pill that floats over it.
last_y() { ui dump | grep -E "StaticText|GenericElement" | tail -1 | awk '{print $2}'; }
pill_y() { ui dump | grep -E "Button +([^ ]+, )?(Agents|Working) [0-9]+" | head -1 | awk '{print $2}'; }

FAULTS=0; CHECKED=0
for ROW in $ROWS; do
  xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
  xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
  sleep 12
  reach_the_list "$UDID" || { echo "$ROW: could not reach the list"; FAULTS=$((FAULTS + 1)); continue; }
  ui tap "$ROW" >/dev/null 2>&1 || { echo "$ROW: not on the list"; continue; }
  sleep 9

  OPENED=$(last_y); PILL=$(pill_y)
  xcrun simctl io "$UDID" screenshot "$OUT/$ROW-as-opened.png" >/dev/null 2>&1
  # Page to the true end by hand. Four pages, because one swipe is not the
  # end of a long transcript and calling it one is how cycle 110 got 93 pt
  # for a shortfall that was really 501.
  for _ in 1 2 3 4; do idb ui swipe 196 700 196 300 --duration 0.5 --udid "$UDID" >/dev/null 2>&1; sleep 2; done
  ENDED=$(last_y)

  CHECKED=$((CHECKED + 1))
  echo "== $ROW =="
  case "$OPENED$ENDED" in
    *[!0-9-]*|"") echo "  could not read both positions — saying nothing about this one"
                  FAULTS=$((FAULTS + 1)); continue;;
  esac
  SHORT=$((OPENED - ENDED))
  echo "  last line on opening:  y=$OPENED"
  echo "  last line at the end:  y=$ENDED"
  echo "  unused scroll:         ${SHORT} pt"
  if [ "${SHORT#-}" -le 4 ]; then
    echo "  landing:   lands at the end"
  else
    echo "  landing:   STOPS $SHORT pt SHORT — the rows between are not realised"
    echo "             when the first scroll runs (M-368); a second scroll fixes it"
    FAULTS=$((FAULTS + 1))
  fi
  if [ -z "$PILL" ]; then
    echo "  clearance: no workers pill in this chat, so nothing floats over the end"
  elif [ "$OPENED" -lt "$PILL" ]; then
    echo "  clearance: the last line sits $((PILL - OPENED)) pt above the pill"
  else
    echo "  clearance: the last line is AT OR UNDER the pill (line $OPENED, pill $PILL)"
    FAULTS=$((FAULTS + 1))
  fi
done

echo
if [ "$CHECKED" = 0 ]; then
  echo "VERDICT: none — no chat was opened, so this says nothing"
elif [ "$FAULTS" = 0 ]; then
  echo "VERDICT: every chat opened at its end with its last line clear of the pill"
  echo "         ($CHECKED checked)"
else
  echo "VERDICT: $FAULTS fault(s) across $CHECKED chat(s) — named above"
fi
echo "stills in $OUT"

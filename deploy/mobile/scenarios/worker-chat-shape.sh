#!/bin/bash
# What a worker's chat is made of, counted rather than looked at.
#
#   worker-chat-shape.sh <cycle> [project]
#
# The style row for this screen has been judged by eye since cycle 37 and
# last re-read at 74: "plain header, collapsed tool rows with chevrons, flat
# surface, no composer, the header reads Back". Every one of those is a
# statement about the accessibility tree, so none of them needs an eye.
#
# `style-pair.py` is not the tool here. It measures row pitch and ground
# colour, which are properties of a *list*; pointed at a chat it compares
# paragraph edges with message bubbles and reports a difference that means
# nothing (M-353). It refuses that pairing now, which left this row
# unmeasured — this is the part of it that can be measured.
#
# What is deliberately absent matters as much as what is there. A worker's
# chat has no composer on purpose (M-154: a dead input is worse than none),
# so a TextField appearing here is a regression, not an improvement.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/worker-chat-shape"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
sleep 10
reach_the_list "$UDID" || exit 1
# Name what was on screen when the row was not. The roster changes — `pod`
# left the list between two runs of this an hour apart — and "no pod row"
# alone reads as a navigation failure when it is a project that is gone.
ui tap "$ROW" >/dev/null || {
  echo "no '$ROW' row. The list holds:"
  ui dump | grep -oE "Button +[a-z0-9.-]+," | sed -E 's/^Button +//; s/,$//' | sed 's/^/  /'
  exit 1
}
sleep 5

# In by the sheet, which is the way that has always worked.
PILL=$(ui dump | grep -E "Button +([^ ]+, )?(Agents|Working) [0-9]+" | head -1)
[ -n "$PILL" ] || { echo "no workers pill in $ROW — this project has no workers to open"; exit 1; }
idb ui tap "$(echo "$PILL" | awk '{print $1}')" "$(echo "$PILL" | awk '{print $2}')" --udid "$UDID"
sleep 3
FIRST=$(ui dump | grep -E "Button +.+, " | head -1 | awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }')
[ -n "$FIRST" ] || { echo "the sheet did not open"; exit 1; }
ui tap "$FIRST" >/dev/null || { echo "could not open '$FIRST'"; exit 1; }
sleep 4
ui dump > "$OUT/tree.txt"
xcrun simctl io "$UDID" screenshot "$OUT/01-worker-chat.png" >/dev/null 2>&1

# Prove we are where we think we are before counting anything about it. A
# worker's chat has a Back button and no composer; the project's chat has
# both. Counting the project's chat and calling it the worker's would pass
# every line below.
if ! grep -qE "Button +Back" "$OUT/tree.txt"; then
  echo "no Back button — this is not a pushed chat. Nothing below would be about"
  echo "a worker's chat, so nothing below is printed."
  exit 1
fi

echo "opened: $FIRST"
echo

FAULTS=0
say() { printf "  %-34s %s\n" "$1" "$2"; }
want() { # want <what> <found> <expected-when-good> <fault message>
  if [ "$2" = "$3" ]; then say "$1" "$2"; else say "$1" "$2   ← $4"; FAULTS=$((FAULTS + 1)); fi
}

# The whole name, not its first word. A worker is named by its goal, so
# `$4` printed "count" for "count slowly one to forty, Done" — a header
# claim that reads as a header of one word.
TITLE=$(awk '$3 == "StaticText" && $2 < 120 { $1=""; $2=""; $3=""; sub(/^ +/, ""); print; exit }' "$OUT/tree.txt")
say "the header names" "${TITLE:-nothing}"
[ -n "$TITLE" ] || { FAULTS=$((FAULTS + 1)); }

want "back control reads" "$(grep -cE "Button +Back" "$OUT/tree.txt")" 1 "the header should carry exactly one Back"
want "composers (M-154: none)" "$(grep -cE " TextField " "$OUT/tree.txt")" 0 "a worker's chat has no composer on purpose"
want "send buttons" "$(grep -cE "Button +Send" "$OUT/tree.txt")" 0 "nothing to send with, so nothing to send"
want "microphone buttons" "$(grep -cE "Button +Microphone" "$OUT/tree.txt")" 0 "no composer means no dictation"

# A plain header: the back control, the name, and nothing else up there.
CHROME=$(awk '$2 < 120 && ($3 == "Button" || $3 == "PopUpButton") { n++ } END { print n+0 }' "$OUT/tree.txt")
want "controls in the header" "$CHROME" 1 "plain means Back and the name, nothing more"

ROWS=$(grep -cE "StaticText" "$OUT/tree.txt")
say "text rows on screen" "$ROWS"
FOLDS=$(grep -cE "tool call|tool calls" "$OUT/tree.txt")
say "collapsed tool rows" "$FOLDS"

echo
if [ "$FAULTS" = 0 ]; then
  echo "VERDICT: the shape holds — one Back, a name, no composer, and nothing"
  echo "         else in the header"
else
  echo "VERDICT: $FAULTS of the shape's claims no longer hold — marked above"
fi
echo "tree and still in $OUT"

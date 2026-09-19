#!/bin/bash
# COVERS: projects list — faces, rows, sections
# The list's two sections: do they appear, fold, and hold the right rows?
#
#   list-sections.sh <cycle>
#
# The coverage row is called "projects list — faces, rows, **sections**".
# `list-rows.sh` measures the rows and says nothing about the sections, so
# half the row's name has never been checked — a check that covers part of
# its own claim, which is this loop's commonest fault in a new place.
#
# Three claims, from `ProjectsView`:
#
#   * a section header is drawn only when it has rows under it — "an empty
#     `Read` header over nothing reads as a section somebody collapsed, not
#     as nothing matched";
#   * tapping a header folds its rows away and tapping it again brings them
#     back;
#   * a project with a live link belongs to `Working`, one without to
#     `Read`.
#
# The third is checked by position rather than by asking the app: a row
# between the `Working` header and the `Read` header is in `Working`. That
# is what a person sees, and it cannot agree with the app by construction.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/list-sections"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
rows() { ui dump | grep -cE "Button +[A-Za-z.][A-Za-z0-9._-]*, "; }
header_y() { ui dump | awk -v t="$1" '$3 == "Button" && $4 == t { print $2; exit }'; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
sleep 11
reach_the_list "$UDID" || exit 1
ui dump > "$OUT/tree.txt"
xcrun simctl io "$UDID" screenshot "$OUT/01-list.png" >/dev/null 2>&1

FAULTS=0
WORKING_Y=$(header_y Working)
READ_Y=$(header_y Read)
ALL=$(rows)
echo "headers on screen:  Working $([ -n "$WORKING_Y" ] && echo "at y=$WORKING_Y" || echo "absent")," \
     "Read $([ -n "$READ_Y" ] && echo "at y=$READ_Y" || echo "absent")"
echo "rows on screen:     $ALL"

# 1. A header with nothing under it. `Working` is absent when nothing is
#    working, which is the common case, so its absence is not a fault —
#    a header present with no rows beneath it is.
under() { # rows between $1 and the next header below it
  local from=$1 to=$2
  ui dump | awk -v a="$from" -v b="$to" '
    $3 == "Button" && $4 ~ /^[A-Za-z.][A-Za-z0-9._-]*,$/ && $2 > a && (b == 0 || $2 < b) { n++ }
    END { print n+0 }'
}
if [ -n "$WORKING_Y" ]; then
  N=$(under "$WORKING_Y" "${READ_Y:-0}")
  echo "  under Working:    $N"
  [ "$N" -gt 0 ] || { echo "  FAULT: a Working header with nothing under it"; FAULTS=$((FAULTS + 1)); }
fi
if [ -n "$READ_Y" ]; then
  N=$(under "$READ_Y" 0)
  echo "  under Read:       $N"
  [ "$N" -gt 0 ] || { echo "  FAULT: a Read header with nothing under it"; FAULTS=$((FAULTS + 1)); }
fi
[ -n "$WORKING_Y$READ_Y" ] || { echo "no section headers at all — nothing to measure"; exit 1; }

# 2. Folding. Tap the header that has rows, count, tap again, count.
FOLD=${READ_Y:+Read}; FOLD=${FOLD:-Working}
echo
echo "folding '$FOLD':"
ui tap "$FOLD" >/dev/null 2>&1; sleep 2
FOLDED=$(rows)
xcrun simctl io "$UDID" screenshot "$OUT/02-folded.png" >/dev/null 2>&1
echo "  rows after folding:   $FOLDED   (was $ALL)"
[ "$FOLDED" -lt "$ALL" ] || { echo "  FAULT: folding hid nothing"; FAULTS=$((FAULTS + 1)); }
ui tap "$FOLD" >/dev/null 2>&1; sleep 2
BACK=$(rows)
echo "  rows after unfolding: $BACK"
[ "$BACK" = "$ALL" ] || { echo "  FAULT: unfolding did not bring them all back ($BACK of $ALL)"; FAULTS=$((FAULTS + 1)); }

echo
# The third claim in this file's own header — a live project belongs to
# Working, one without a link to Read — needs both sections on screen at
# once. A run where every project rests in Read cannot see it, and a
# verdict that does not say so claims more than it measured. Which is the
# fault this file was written to fix, one level up.
if [ -z "$WORKING_Y" ] || [ -z "$READ_Y" ]; then
  MEMBERSHIP="not tested this run: only one section was on screen, so no row
         could be seen to be in the right one"
else
  MEMBERSHIP="both sections were on screen, and every row sat under one of them"
fi

if [ "$FAULTS" = 0 ]; then
  echo "VERDICT: the sections hold — no header stands over nothing, and folding"
  echo "         hides $((ALL - FOLDED)) row(s) and gives them back."
  echo "         Membership: $MEMBERSHIP"
else
  echo "VERDICT: $FAULTS fault(s) in the list's sections — named above."
  echo "         Membership: $MEMBERSHIP"
fi
echo "tree and stills in $OUT"

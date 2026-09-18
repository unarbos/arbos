#!/bin/bash
# Two counts of the same thing, from the same app, at the same moment.
#
#   pill-count-vs-sheet.sh <cycle> [project]
#
# The chat's pill says how many agents a project has. The sheet it opens
# lists them. At cycle 74 the pill read `Agents 19` and the sheet, paged to
# its end, held 12 rows — so one of the two is wrong, and a worker that has
# only just started is a plausible one to be missing (M-272, M-275).
#
# This exists so the question can be re-asked in one command rather than
# reconstructed. It diagnoses nothing; it puts the two numbers side by side.
#
# Two traps it avoids, both of which this loop has already fallen into:
#
#   * the sheet scrolls, and only rendered rows reach the tree, so the rows
#     are collected by paging to the end rather than read off one screen;
#   * two workers whose goal text truncates to the same string would collapse
#     in a naive unique count, so duplicate labels are reported rather than
#     silently folded away.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"   # page_up: scroll in points, not screenshot pixels
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/pill-vs-sheet"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 9
reach_the_list "$UDID" || exit 1
ui tap "$ROW" >/dev/null || { echo "no $ROW row"; exit 1; }
sleep 5

# The pill reads `Working N` while anything runs and `Agents N` when nothing
# does, and only the second is the same quantity the sheet lists. Declining
# on the first was right, and it made this a permanent decline: run in the
# sweep, after the scenarios that start workers, there is always something
# working. So wait for the project to settle before reading it.
WAITED=0
while [ "$WAITED" -lt 180 ]; do
  PILL=$(ui dump | grep -oE "(Agents|Working) [0-9]+" | head -1)
  case "$PILL" in
    Agents*) break;;
    "")      break;;
  esac
  [ "$WAITED" = 0 ] && echo "the pill says:  $PILL — waiting for the project to settle"
  sleep 10
  WAITED=$((WAITED + 10))
done
COUNT=$(echo "$PILL" | grep -oE "[0-9]+")
[ -n "$COUNT" ] || { echo "no pill in this chat — nothing to compare"; exit 1; }
echo "the pill says:  $PILL$([ "$WAITED" -gt 0 ] && echo "   (after $WAITED s)")"
# The pill counts two different things: `Agents N` is every agent, `Working
# N` is only the ones running. The sheet always lists them all, so comparing
# against the Working form is comparing a subset with a whole — it reported
# "pill 2, sheet 3, and no shared labels to explain it" on a project behaving
# exactly as designed.
case "$PILL" in
  Working*) echo "VERDICT: cannot say. Still 'Working' after ${WAITED}s, so the pill is"
            echo "         counting only what runs while the sheet lists every agent —"
            echo "         a subset against a whole. Not a fault, and not a comparison."
            exit 0;;
esac

# Tapped by frame: the pill's label is one the sheet's rows carry too.
AT=$(ui dump | grep -E "Button +(Agents|Working) [0-9]+" | head -1 | awk '{print $1, $2}')
idb ui tap $AT --udid "$UDID"; sleep 3
xcrun simctl io "$UDID" screenshot "$OUT/01-sheet.png" >/dev/null 2>&1

LABELS=$OUT/rows.txt
HOW=$(collect_rows "$UDID" 'Button +.+, ' "$LABELS")
ROWS=$(wc -l < "$LABELS" | tr -d ' ')
echo "the sheet lists: $ROWS rows (paging $HOW)"

# Two workers could share a label only if both are on screen together, so
# the check must be within one dump. Across the ten pages every row repeats
# by construction, and counting that way reported "12 duplicates" for a list
# with none.
DUPES=$(ui dump | grep -E "Button +.+, " \
        | awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }' \
        | sort | uniq -d | wc -l | tr -d ' ')
echo "labels sharing a name on one screen (each would hide a row): $DUPES"

# Rows are counted by their label, so two workers with the same goal text
# count once. That is fine while labels are unique and worthless the moment
# they are not — and with enough runs behind it this project has repeats.
# A tool that cannot measure should say so rather than produce a number and
# a verdict, which is how cycle 74 filed a disagreement that did not exist.
if [ "$DUPES" -gt 0 ]; then
  echo "VERDICT: cannot say. $DUPES label(s) are shared on a single screen, and rows are"
  echo "         counted by label, so the sheet's $ROWS is a floor and not a count."
  echo "         Pill $COUNT. Compare these two only on a project whose goals are distinct —"
  echo "         qa-cycle-11-demo was one at cycle 92, where the two agreed on 12."
elif [ "$ROWS" = "$COUNT" ]; then
  echo "VERDICT: the two agree on $COUNT."
else
  echo "VERDICT: they disagree — pill $COUNT, sheet $ROWS, and no shared labels to explain it."
  echo "         Rows are in $LABELS if you want to see which are present."
fi

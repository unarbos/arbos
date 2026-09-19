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
# Side by side is all this ever was, and cycle 154 read its own comment and
# saw the problem: the pill is `chat.workers.count` and the sheet is a
# `ForEach` over that same array. Two drawings of one number cannot disagree
# about the app. Every "cannot say" it has ever printed was about the rig
# failing to reach the end of a scrolling sheet, or failing to tell two
# workers apart when their goals truncate alike — and the one disagreement it
# did file, at cycle 74, turned out to be exactly that (M-272, M-275).
#
# So the numbers stay, as context, and the verdict moves to a question that
# can come out either way: **workers this run put there itself**. Three goals
# carrying a tag no earlier run used. The pill must rise by three and the
# sheet must show all three. A row dropped in the drawing fails it; a rig that
# cannot reach the end of the sheet fails it too, and says which.
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
BEFORE=$(echo "$PILL" | grep -oE "[0-9]+")
[ -n "$BEFORE" ] || { echo "no pill in this chat — nothing to compare"; exit 1; }
echo "the pill says:  $PILL$([ "$WAITED" -gt 0 ] && echo "   (after $WAITED s)")"

# Three workers of this run's own, tagged so no earlier run's goal can be
# mistaken for one of them. The tag leads each goal because the sheet
# truncates a long name at the end (M-305).
TAG=p$(date -u +%H%M%S)
echo
echo "asking for three workers tagged $TAG"
type_line "$UDID" "Start three workers at once, each waiting for none of the others. Their goals are exactly $TAG one, $TAG two and $TAG three. Each says one short sentence about its number." || exit 1
ui tap "Send" >/dev/null || { echo "  no send button"; exit 1; }

# Wait for the pill to account for them. It reads `Working N` while they run,
# which is the same N.
SEEN=0
for _ in $(seq 1 60); do
  NOW=$(ui dump | grep -oE "(Agents|Working) [0-9]+" | head -1 | grep -oE "[0-9]+")
  [ -n "$NOW" ] && [ "$NOW" -ge $((BEFORE + 3)) ] && { SEEN=$NOW; break; }
  sleep 5
done
if [ "$SEEN" = 0 ]; then
  echo "  the pill never reached $((BEFORE + 3)) — it says $(ui dump | grep -oE '(Agents|Working) [0-9]+' | head -1)."
  echo
  echo "VERDICT: cannot say. The three workers this run asked for never showed in"
  echo "         the pill, so there is nothing of this run's own to look for in"
  echo "         the sheet. That is the kernel or the ask, not the drawing."
  exit 1
fi
echo "  the pill now says: $SEEN   (was $BEFORE)"
COUNT=$SEEN
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

# The part that can come out either way.
MINE=$(grep -c "$TAG" "$LABELS")
echo "rows carrying this run's tag $TAG: $MINE of 3"
grep "$TAG" "$LABELS" | cut -c1-70 | sed 's/^/    /' 

# The verdict. It rests on the three workers this run put there, because
# those are the only rows whose number is known independently of what the app
# says about itself. The totals follow as context.
TOTALS="the totals read: pill $COUNT, sheet $ROWS"
if [ "$ROWS" -lt "$COUNT" ]; then
  PAGES=$(echo "$HOW" | grep -oE "[0-9]+" | head -1)
  TOTALS="$TOTALS — the sheet's paging fell short over $PAGES page(s), which is"
  TOTALS="$TOTALS this rig, not the app: both come from one array"
fi

if [ "$MINE" = 3 ]; then
  echo "VERDICT: the sheet shows all three workers this run started. $TOTALS."
elif [ "$MINE" -gt 3 ]; then
  echo "VERDICT: cannot say. $MINE rows carry a tag only three workers were given,"
  echo "         so the tag is not doing its job. $TOTALS."
else
  echo "VERDICT: the pill counted three new workers and the sheet shows $MINE of them."
  echo "         $TOTALS."
  if [ "${DUPES:-0}" -gt 0 ]; then
    echo "         $DUPES label(s) are shared on one screen, so a tagged row could be"
    echo "         hidden behind an identical one — read the rows above before filing."
  fi
fi

# The old comparison, kept below the verdict because it cannot answer anything
# about the app: the pill is `chat.workers.count` and the sheet is a `ForEach`
# over that same array. It is printed as a reading on this rig.
echo
if [ "$DUPES" -gt 0 ]; then
  echo "on the totals: $DUPES label(s) are shared on a single screen, and rows are"
  echo "         counted by label, so the sheet's $ROWS is a floor and not a count."
elif [ "$ROWS" = "$COUNT" ]; then
  echo "on the totals: the two agree on $COUNT, so the rig reached the end."
elif [ "$ROWS" -lt "$COUNT" ]; then
  # The app cannot disagree with itself here. The pill is `chat.workers.count`
  # and the sheet is a `ForEach` over that same array, and `workers` is a
  # dictionary keyed by the agent's id, so no row can be lost to a collision.
  # A shortfall is this scenario failing to reach the end of the sheet, and
  # `collect_rows` cannot tell "I reached the end" from "my swipe did
  # nothing" — both look like a page that added no labels.
  # Both numbers come from one array in the app — the pill counts it, the
  # sheet draws it, and `workers` is keyed by the agent's id — so the app
  # cannot disagree with itself. Which of the two rig faults it is depends
  # on whether the sheet scrolled at all.
  PAGES=$(echo "$HOW" | grep -oE "[0-9]+" | head -1)
  echo "on the totals: the sheet's paging found $ROWS of the pill's $COUNT."
  echo "         The app draws both from one array, so the two cannot disagree;"
  if [ "${PAGES:-1}" -le 1 ]; then
    echo "         and the sheet converged on its first page, so it never scrolled."
    echo "         Fix the swipe — it is landing outside the sheet."
  else
    echo "         and it scrolled $PAGES pages before converging, so the swipe works."
    echo "         Rows are counted by their label and this project has $((COUNT - ROWS))"
    echo "         more agents than distinct goals: twins on different pages count"
    echo "         once, and the per-screen duplicate check above cannot see them."
    echo "         Use a project whose goals are distinct — qa-cycle-11-demo at"
    echo "         cycle 92, where pill and sheet agreed on 12."
  fi
else
  echo "VERDICT: they disagree — pill $COUNT, sheet $ROWS, and no shared labels to explain it."
  echo "         Rows are in $LABELS if you want to see which are present."
fi

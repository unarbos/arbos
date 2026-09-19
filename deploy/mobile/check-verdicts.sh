#!/bin/bash
# Which verdicts can fire when nothing was measured?
#
#   check-verdicts.sh
#
# Four cycles in six found the same fault in four different scenarios: a
# check that cannot tell "the thing is right" from "the thing is not there".
#
#   M-455  "and the composer cleared" was printed, never tested
#   M-493  the refusal check passed a helper with its guard removed
#   M-521  a row count could not see a transcript grow, because the view
#          scrolled as much as it gained
#   M-552  zero occurrences of the answer read as "one row, worded
#          differently" — on a screen with no answer on it
#
# Finding them one at a time, by running a row and reading hard, costs a
# cycle each. This looks for the shape instead.
#
# The shape is narrow, and saying so is the whole difficulty. A pass gated on
# a zero is usually right: zero faults, zero duplicates, zero rows left
# behind. What is suspect is a pass gated on a count of things *observed*
# being zero, because nothing observed is also what a broken rig produces.
# So the two families are told apart by what the variable counts, which is
# what its name says:
#
#   zero is good      FAULT(S), WRONG, BAD, DUPE(S), MISSED, STRAY, LOST,
#                     EXTRA, LEFT, SPLIT, ORPHAN
#   zero is nothing   ROWS, TIMES, COUNT, SEEN, LINES, FOUND, HELD, AFTER,
#                     REVEALED, ANSWERS, TOTAL
#
# A hit is not a bug. It is a verdict worth reading with the question "what
# would this say if the app never drew anything at all?"
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
GOOD='FAULT|FAULTS|WRONG|BAD|DUPE|DUPES|MISSED|STRAY|LOST|EXTRA|LEFT|SPLIT|ORPHAN|NOVERDICT|SILENT'
NOTHING='ROWS|TIMES|COUNT|SEEN|LINES|FOUND|HELD|AFTER|REVEALED|ANSWERS|TOTAL|SPOKEN|MINE|OPEN|CLOSED'

# Read once, at cycle 180, and sound. Listed so a later run shows only what
# is new — a tool that reprints the same five known-good lines every time
# gets skimmed, and then the sixth line is skimmed too.
#
#   call-text-in-chat  SPOKEN=0  declines: "nothing is marked Spoken"
#   call-text-in-chat  AFTER=0   declines: no answer row under the marker,
#                                which is the guard cycle 179 added after the
#                                check passed on a screen with no answer
#   chip-and-send-arrow AFTER=0  the desired outcome, and a chip arriving is
#                                checked separately: "[ $N -gt 0 ] || exit 1"
#   chip-and-send-arrow AFTER=0  the same guard, second branch
#   pill-count-vs-sheet SEEN=0   declines: the pill never counted the workers
#   worker-while-it-works ROWS=0 declines: "the sheet did not open"
# Keyed on the file and the variable, not the line. The first version used
# line numbers and cycle 179's edit to call-text-in-chat moved its guard from
# 105 to 111, so a known-good entry reported as new the very next run. A list
# that cries wolf after any edit above it is a list nobody reads.
READ_ALREADY="call-text-in-chat.sh:AFTER call-text-in-chat.sh:SPOKEN chip-and-send-arrow.sh:AFTER pill-count-vs-sheet.sh:SEEN worker-while-it-works.sh:ROWS"

echo "verdicts that can fire on a count of nothing:"
HITS=0
KNOWN=0
for f in "$HERE"/scenarios/*.sh "$HERE"/mac-*.sh; do
  [ -f "$f" ] || continue
  # A branch guarding a zero on a variable that counts what was observed,
  # with a verdict inside it. Read the file, not this list, before acting.
  while IFS= read -r line; do
    no=${line%%:*}
    var=$(echo "$line" | grep -oE '\$\{?('"$NOTHING"')\b' | head -1 | tr -d '${')
    [ -n "$var" ] || continue
    echo "$var" | grep -qE "^($GOOD)$" && continue
    # Is there a verdict in the next few lines of that branch?
    sed -n "${no},$((no + 6))p" "$f" | grep -qiE "VERDICT|holds|the rule" || continue
    case " $READ_ALREADY " in
      *" $(basename "$f"):$var "*) KNOWN=$((KNOWN + 1)); continue;;
    esac
    printf '  %-34s line %-5s %s\n' "$(basename "$f")" "$no" "$(echo "${line#*:}" | sed 's/^ *//' | cut -c1-58)"
    HITS=$((HITS + 1))
  done < <(grep -nE '^[[:space:]]*(if|elif)[[:space:]]+\[[^]]*(= *0|-eq 0|-le 0|-lt 1)' "$f")
done
[ "$HITS" = 0 ] && echo "  none new — $KNOWN known one(s), read at cycle 180 and sound"
[ "$HITS" = 0 ] || echo "  ($KNOWN other(s) were read at cycle 180 and are sound)"

echo
echo "Read each as: what would this print if the app drew nothing at all?"
echo "A pass on zero faults is right. A pass on zero rows, zero lines or zero"
echo "occurrences is the shape that cost four cycles."

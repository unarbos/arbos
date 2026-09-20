#!/bin/bash
# Which coverage rows a cycle actually exercised, read from what it left behind.
#
#   what-ran.sh <cycle> [remote]
#
# Closing a cycle means ageing the rows it covered, and that has been done
# from memory. Memory keeps missing the second row: a scenario that declares
# two `# COVERS:` lines gets one of them updated and the other quietly grows
# old on paper while being run every few cycles. It happened at 195, at 200,
# and again at 201 — `what-the-caller-sees.sh` ran at 196 and covers both
# "call — voice first, orb, colours" and "call — first word and
# transcription"; only the first was written down, so the queue kept offering
# the second as the oldest thing in the loop.
#
# Nothing needs to be added to the scenarios for this. Each one writes into
# `~/mobile-out/<cycle>/<its own subdirectory>`, and that name is a literal
# in the file, so the directories a cycle left behind name the scenarios that
# ran — and each scenario already declares the rows it covers.
#
# One thing it cannot see: `mac-journey.sh` writes to
# `~/mobile-out/journey/<run id>`, not under the cycle, so a journey run has
# to be aged by hand. `journey-ledger.sh` is the record that it happened.
set -uo pipefail
CYCLE=${1:?which cycle}
REMOTE=${2:-mac}
HERE=$(cd "$(dirname "$0")" && pwd)

DIRS=$(ssh -o LogLevel=ERROR "$REMOTE" "ls ~/mobile-out/$CYCLE/ 2>/dev/null" 2>/dev/null)
if [ -z "$DIRS" ]; then
  echo "nothing under ~/mobile-out/$CYCLE on $REMOTE — no runs to read"
  exit 1
fi

echo "cycle $CYCLE left these behind:"
echo "$DIRS" | sed 's/^/  /' | head -12

echo
echo "scenarios that wrote them:"
MATCHED=""
for f in "$HERE"/scenarios/*.sh "$HERE"/mac-*.sh; do
  [ -f "$f" ] || continue
  # mac-voice.sh writes `$OUT/voice` from an OUT of its own, so the
  # subdirectory is worth looking for on the line after as well.
  sub=$(grep -oE 'mobile-out/\$CYCLE/[a-z0-9-]+' "$f" 2>/dev/null \
    | sed 's|.*\$CYCLE/||' | head -1)
  [ -n "$sub" ] || sub=$(grep -oE 'OUT/[a-z0-9-]+"' "$f" 2>/dev/null \
    | head -1 | tr -d '"' | sed 's|OUT/||')
  [ -n "$sub" ] || continue
  echo "$DIRS" | grep -qxF "$sub" || continue
  echo "  $(basename "$f")  → $sub"
  MATCHED="$MATCHED $f"
done
[ -n "$MATCHED" ] || echo "  none — the directories match no scenario's own output path"

echo
echo "rows to age to $CYCLE:"
# Every row every one of them declares, which is the whole point: the second
# declaration is the one that gets forgotten.
for f in $MATCHED; do
  grep -h "^# COVERS:" "$f" 2>/dev/null
done | sed 's/^# COVERS: */  /' | sort -u

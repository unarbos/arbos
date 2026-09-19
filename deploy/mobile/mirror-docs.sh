#!/bin/bash
# Mirror the loop's ledgers to the Mac, refusing to lose anything.
#
#   mirror-docs.sh <store-internal-dir> [remote]
#
# The store has dropped these three files three times and this mirror was the
# only surviving copy each time, so the copy must never be blind: a truncated
# source overwriting a complete mirror would destroy the thing the mirror is
# for.
#
# The rule used to be "copy only when the source is the longer file". That is
# unsound in a way cycle 192 met head-on. Updating the coverage table means
# editing rows in place — a cycle's entry is prepended to the row it belongs
# to — so the file grows by hundreds of bytes and not by one line. The line
# count matched exactly, the rule refused, and the decision fell to reading
# both files by hand, which is the thing a rule is supposed to prevent.
#
# What actually matters is not length. It is that nothing in the mirror is
# absent from the source. So that is what this asks, line by line, with one
# allowance: a mirror line that appears *inside* a source line was edited in
# place rather than lost, which is exactly what a coverage row looks like
# after a cycle prepends to it.
set -uo pipefail
DIR=${1:?the internal/ directory inside the store}
REMOTE=${2:-mac}
FILES="mobile-cycle-reports.md mobile-findings.md mobile-coverage.md"
COPIED=0
REFUSED=0

for f in $FILES; do
  SRC="$DIR/$f"
  if [ ! -f "$SRC" ]; then
    echo "  $f — not in the store. Nothing to copy, and nothing is overwritten."
    continue
  fi
  TMP=$(mktemp)
  if ! scp -q -o LogLevel=ERROR "$REMOTE:~/mobile-docs/$f" "$TMP" 2>/dev/null; then
    scp -q -o LogLevel=ERROR "$SRC" "$REMOTE:~/mobile-docs/$f" \
      && echo "  $f — no mirror yet, so the first copy is made ($(wc -c < "$SRC") bytes)"
    rm -f "$TMP"; COPIED=$((COPIED + 1)); continue
  fi

  # Every mirror line the source does not have verbatim. Each one is then
  # asked the softer question: does it survive inside a longer source line?
  LOST=0
  while IFS= read -r line; do
    [ -z "$line" ] && continue
    grep -qxF -- "$line" "$SRC" && continue
    grep -qF -- "$line" "$SRC" && continue   # edited in place, still carried
    LOST=$((LOST + 1))
    [ "$LOST" -le 3 ] && echo "      only in the mirror: $(echo "$line" | cut -c1-90)"
  done < "$TMP"

  SB=$(wc -c < "$SRC"); MB=$(wc -c < "$TMP")
  if [ "$LOST" -gt 0 ]; then
    echo "  $f — REFUSED. $LOST line(s) live only in the mirror; copying would lose them."
    echo "      source $SB bytes, mirror $MB bytes. Reconcile by hand, then run again."
    REFUSED=$((REFUSED + 1))
  elif [ "$SB" -le "$MB" ]; then
    echo "  $f — nothing to do: the mirror already holds everything ($MB bytes)."
  else
    scp -q -o LogLevel=ERROR "$SRC" "$REMOTE:~/mobile-docs/$f" \
      && echo "  $f — mirrored, $MB → $SB bytes, nothing in the mirror lost." \
      || { echo "  $f — the copy failed. The mirror is untouched."; REFUSED=$((REFUSED + 1)); }
    COPIED=$((COPIED + 1))
  fi
  rm -f "$TMP"
done

echo
echo "$COPIED copied, $REFUSED refused"
[ "$REFUSED" = 0 ]

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

  # Every mirror line the source does not carry. "Carry" is the whole
  # question, and the first version of this got it wrong in a way that would
  # have made the tool useless: it asked whether the mirror line survived as
  # a substring, which covers text appended to the end of a line and nothing
  # else. A coverage row is updated by *prepending* the new cycle into the
  # middle — `| row | 179 (…)` becomes `| row | 193 (…), 179 (…)` — so the
  # old line is no longer contiguous anywhere, and the tool refused every
  # coverage update it was written to wave through.
  #
  # So a table row is read as a table row: same first cell, and everything
  # after it still present. Anything else is compared whole.
  LOST=$(python3 - "$SRC" "$TMP" <<'PY'
import sys
src = open(sys.argv[1]).read().splitlines()
mirror = open(sys.argv[2]).read().splitlines()
verbatim = set(src)


def key_and_rest(line):
    if not line.startswith("|") or line.count("|") < 3:
        return None, None
    cells = line.split("|")
    return cells[1].strip(), "|".join(cells[2:])


by_key = {}
for line in src:
    k, rest = key_and_rest(line)
    if k is not None:
        by_key.setdefault(k, []).append(rest)

lost = []
for line in mirror:
    if not line.strip() or line in verbatim:
        continue
    if line in "\n".join(src):          # appended to, still whole
        continue
    k, rest = key_and_rest(line)
    if k is not None and any(rest.strip() in r for r in by_key.get(k, [])):
        continue                        # same row, added to in the middle
    lost.append(line)

for line in lost[:3]:
    print(f"      only in the mirror: {line[:90]}", file=sys.stderr)
print(len(lost))
PY
)

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

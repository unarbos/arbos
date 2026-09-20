#!/bin/bash
# Which coverage rows does the harness actually exercise?
#
#   coverage-map.sh [path to mobile-coverage.md]
#
# The rotation rule says take the oldest row first, and a row's age is the
# last cycle that *named* it. Cycle 156 found the rule pointing at rows the
# sweep had run days earlier: `list composer` read as cycle 81 and `cold
# start` as 86, and both had gone through the cycle-151 sweep. A row covered
# by the sweep ages on paper while being tested every time, so the loop's own
# scheduler sends it to the wrong place (M-484).
#
# Each scenario now declares the row it serves, as `# COVERS: <row>` near the
# top. This reads those declarations against the table and answers three
# questions:
#
#   * which rows nothing claims — the genuinely untested ones;
#   * which declarations name a row the table does not have — a typo, which
#     would otherwise make a row look covered when nothing covers it;
#   * which rows the sweep in particular reaches, since that is the set the
#     rotation keeps mis-ageing.
#
# It is deliberately a reader, not a writer. Nothing here edits the ledger:
# a tool that silently marks rows as checked would age them correctly and
# say nothing true about whether anyone looked.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
TABLE=${1:-$HOME/mobile-docs/mobile-coverage.md}
[ -f "$TABLE" ] || { echo "no coverage table at $TABLE"; echo "pass its path as the argument"; exit 1; }

ROWS=$(mktemp); CLAIMS=$(mktemp); SWEEPERS=$(mktemp)
trap 'rm -f "$ROWS" "$CLAIMS" "$SWEEPERS"' EXIT

# The table's first column, minus the header and the rule line.
grep "^|" "$TABLE" | awk -F'|' '{ gsub(/^ +| +$/, "", $2); print $2 }' \
  | grep -vE "^(aspect|-+)?$" | sort -u > "$ROWS"

# Scenarios are not the only thing that covers a row. The journey, the style
# pair and the name check are entry points of their own, and reading only
# `scenarios/` reported five rows as untested that this harness tests every
# time it runs them — including the two journey rows, on the morning after a
# journey run.
grep -h "^# COVERS:" "$HERE"/scenarios/*.sh "$HERE"/*.sh "$HERE"/*.py 2>/dev/null \
  | sed 's/^# COVERS: *//' | sed 's/ *$//' | sort -u > "$CLAIMS"

# Which scenarios the sweep runs, read from the sweep rather than restated.
# Its default list is DEFAULT=( ... ), not SCENARIOS=( ... ); SCENARIOS is the
# argument list. Reading the wrong name printed an empty section and a verdict
# that did not notice, which is the fault this whole file exists to catch.
sed -n '/^DEFAULT=(/,/^)/p' "$HERE/sweep.sh" | grep -oE "[a-z0-9-]+\.sh" | sort -u > "$SWEEPERS"
[ -s "$SWEEPERS" ] || { echo "read no scenarios out of sweep.sh — its list is not where this"
                        echo "expects it, and every line below about the sweep would be empty"
                        echo "rather than false. Fix the reader before trusting the report."
                        exit 1; }

echo "coverage rows:            $(wc -l < "$ROWS" | tr -d ' ')"
echo "rows something claims:     $(comm -12 "$ROWS" "$CLAIMS" | wc -l | tr -d ' ')"
echo

echo "rows nothing in the harness claims — the genuinely untested ones:"
UNCLAIMED=$(comm -23 "$ROWS" "$CLAIMS")
if [ -z "$UNCLAIMED" ]; then echo "  none"; else echo "$UNCLAIMED" | sed 's/^/  /'; fi

echo
echo "declarations naming a row the table does not have:"
# A typo here is worse than a missing declaration: the row it meant to claim
# still reads as unclaimed, and the claim itself points at nothing.
STRAY=$(comm -13 "$ROWS" "$CLAIMS")
if [ -z "$STRAY" ]; then echo "  none — every declaration matches a row"; else echo "$STRAY" | sed 's/^/  /'; fi

echo
echo "rows the sweep reaches every run (these age wrongly in the ledger):"
while read -r s; do
  grep -h "^# COVERS:" "$HERE/scenarios/$s" 2>/dev/null | sed 's/^# COVERS: *//'
done < "$SWEEPERS" | sort -u | sed 's/^/  /'

echo
echo "the work queue — rows the sweep does not reach, oldest first:"
# The rotation rule says take the oldest row. Read plainly it keeps naming
# rows the sweep ran an hour earlier, because a row's age is the last cycle
# that *named* it and the sweep names nothing. The rows worth a cycle are the
# old ones nothing runs automatically, and that is a different list.
python3 - "$TABLE" "$SWEEPERS" "$HERE" <<'PY'
import re, subprocess, sys
table, sweepers, here = sys.argv[1], sys.argv[2], sys.argv[3]
swept = set()
for name in open(sweepers).read().split():
    try:
        for line in open(f"{here}/scenarios/{name}"):
            if line.startswith("# COVERS:"):
                swept.add(line[len("# COVERS:"):].strip())
    except OSError:
        pass
out = []
for line in open(table, encoding="utf-8"):
    if not line.startswith("| ") or line.startswith("|---"):
        continue
    cells = line.split("|")
    if len(cells) < 3:
        continue
    name = cells[1].strip()
    if not name or name in swept:
        continue
    # An entry is written `<cycle> (what happened)`, and entries are joined
    # with commas: `201 (…), 196 (…), 188 (…)`. So the cycle is the number
    # that opens an entry, never a number inside the prose.
    #
    # Reading every number in the cell needed two guards and both went bad.
    # Finding ids look like cycle numbers once "M-" is gone, which once put
    # "call — the microphone path" at 285. The fix for that was a ceiling —
    # "anything past the cycle we could plausibly be in" — hardcoded at 200.
    # At cycle 202 that ceiling started discarding the real cycle numbers and
    # falling back to whatever digits the prose held: the row updated an hour
    # earlier reported "last named at 7", because `Worked 1m 44s` and a stray
    # 7 were all that survived the filter. A map of what is stale went stale.
    #
    # Matching the shape needs no ceiling and no id-stripping, so it cannot
    # rot with the passage of time.
    seen = [int(n) for n in re.findall(r"(?:^|,)\s*(\d{1,4})\s*\(", cells[2])]
    if name and seen:
        out.append((max(seen), name))

# Not every row is aged in cycles. "TestFlight build on Jacob's phone" is
# written in build numbers — `956 (13 reports), 994 (fixes for F1–F6)` — and
# the old parser read the 13 out of "13 reports" and called it cycle 13, so
# the row sat at the top of the queue for months meaning nothing. Reading the
# entries properly puts it at 994, which sorts to the newest end and drops it
# off a list of the oldest: silently, which is worse.
#
# The yardstick is the table's own: most rows are within a few cycles of each
# other, so the median of their newest entries is near the current cycle, and
# anything several times that is not a cycle number at all. No constant to go
# stale.
ages = sorted(c for c, _ in out)
median = ages[len(ages) // 2] if ages else 0
not_cycles = [(c, n) for c, n in out if median and c > median * 2]
out = [(c, n) for c, n in out if not (median and c > median * 2)]

for cycle, name in sorted(out)[:6]:
    print(f"  last named at {cycle:>4}   {name}")
if not out:
    print("  none — the sweep reaches every row the table has")
if not_cycles:
    print()
    print("  rows not aged in cycles, so not ranked above:")
    for cycle, name in sorted(not_cycles):
        print(f"    {name} — its entries read {cycle}, which is not a cycle number")
PY

echo
if [ -n "$STRAY" ]; then
  echo "VERDICT: $(echo "$STRAY" | wc -l | tr -d ' ') declaration(s) name no row. Fix those first — each one"
  echo "         hides a row that still has nothing covering it."
  exit 1
fi
echo "VERDICT: every declaration matches a row, and $(echo "$UNCLAIMED" | grep -c .) row(s) have nothing covering them."

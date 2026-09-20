#!/bin/bash
# Carry finished journey runs into the store's ledgers.
#
#   journey-ledger.sh <store-internal-dir> [remote]
#
# `mac-journey.sh` ends by calling `journey-record.py`, which writes
# `<run-dir>/record.json` on the Mac and prints the line. Getting that line
# into `internal/mobile-journey-runs.md` and its machine twin
# `mobile-journey-history.jsonl` was a step done by hand — and by cycle 200
# the prose ledger had stopped at cycle 187 and the JSONL two days before
# that, with three completed journeys missing from both. The runs happened,
# the evidence is on the Mac, and the ledger QA reads did not know.
#
# So it is a tool. Every record on the Mac is fetched, and any whose run id
# is not already in the JSONL is appended to both files. Idempotent on the
# run id, so running it twice adds nothing the second time and it is safe to
# call at the end of every cycle whether a journey ran or not.
set -uo pipefail
DIR=${1:?the internal/ directory inside the store}
REMOTE=${2:-mac}
JSONL="$DIR/mobile-journey-history.jsonl"
PROSE="$DIR/mobile-journey-runs.md"
[ -f "$JSONL" ] || { echo "no $JSONL — refusing to create a ledger from nothing"; exit 1; }
[ -f "$PROSE" ] || { echo "no $PROSE — refusing to create a ledger from nothing"; exit 1; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
# The run id is the directory name — `0920-001944` — which is what makes
# this idempotent. The record itself carries an evidence path containing it.
scp -q -o LogLevel=ERROR "$REMOTE:~/mobile-out/journey/*/record.json" "$TMP/" 2>/dev/null
# scp flattens, so identity has to come from inside the file. Fetch them one
# directory at a time instead.
rm -f "$TMP"/*.json
for id in $(ssh -o LogLevel=ERROR "$REMOTE" 'ls -d ~/mobile-out/journey/*/ 2>/dev/null | xargs -n1 basename' 2>/dev/null); do
  scp -q -o LogLevel=ERROR "$REMOTE:~/mobile-out/journey/$id/record.json" "$TMP/$id.json" 2>/dev/null
done

# Each file is asked separately, because they are not equally far behind and
# assuming they are is how the first run of this tool duplicated fourteen
# rows. At cycle 200 the prose ledger held every run up to 09-19 19:37 while
# its machine twin stopped two days earlier: one file was missing three runs
# and the other fourteen. A single "have I seen this?" check against one of
# them appends the difference to both.
ADDED=0
SKIPPED=0
for f in "$TMP"/*.json; do
  [ -f "$f" ] || continue
  id=$(basename "$f" .json)
  # The prose rows are keyed by the date and time the id spells out, because
  # they were written by hand long before this tool existed.
  when="${id:0:2}-${id:2:2} ${id:5:2}:${id:7:2}"
  grep -qF "$id" "$JSONL" && IN_JSONL=yes || IN_JSONL=no
  grep -qF "| $when " "$PROSE" && IN_PROSE=yes || IN_PROSE=no
  if [ "$IN_JSONL" = yes ] && [ "$IN_PROSE" = yes ]; then
    SKIPPED=$((SKIPPED + 1)); continue
  fi
  python3 - "$f" "$id" "$JSONL" "$PROSE" "$IN_JSONL" "$IN_PROSE" <<'PY'
import json, sys

path, run_id, jsonl, prose, in_jsonl, in_prose = sys.argv[1:7]
rec = json.load(open(path))

if in_jsonl == "no":
    with open(jsonl, "a") as f:
        f.write(json.dumps(rec, sort_keys=True) + "\n")

steps = rec.get("steps", {})
passes = sum(1 for v in steps.values() if v == "pass")
unver = sum(1 for v in steps.values() if v == "unverified")
fails = sum(1 for v in steps.values() if v == "fail")
# `0920-001944` reads as a date and a time, which is how every other row in
# this file is written.
when = f"{run_id[0:2]}-{run_id[2:4]} {run_id[5:7]}:{run_id[7:9]}"
tail = f", **{fails} FAIL**" if fails else ""
row = (f"| {when} | `{rec.get('target','?')}` — kernel "
       f"**`{rec.get('kernel_git_sha','?')}`** (attach socket, both ends) | "
       f"app **`{rec.get('branch','?')}`** | **{passes} pass, {unver} "
       f"unverified{tail}** | `{rec.get('evidence','')}` |\n")
if in_prose == "no":
    with open(prose, "a") as f:
        f.write(row)
where = " + ".join([n for n, v in (("jsonl", in_jsonl), ("prose", in_prose)) if v == "no"])
print(f"  {when}  {passes} pass, {unver} unverified{tail}  "
      f"{rec.get('branch','?')}  → {where}")
PY
  ADDED=$((ADDED + 1))
done

# Appending puts a row at the end whatever its date, and a backfill is
# mostly older runs — cycle 200 added six from two days earlier and the
# ledger then read forwards, backwards, then forwards again. Sorted in
# place, rows only: the header and the prose around them do not move, and
# the row count is checked so a sort can never quietly drop one.
if [ "$ADDED" -gt 0 ]; then
  python3 - "$PROSE" <<'PY'
import re, sys

path = sys.argv[1]
lines = open(path).read().split("\n")
key = re.compile(r"^\| (\d{2})-(\d{2}) (\d{2}):(\d{2}) ")
idx = [i for i, l in enumerate(lines) if key.match(l)]
rows = [lines[i] for i in idx]
before = len(rows)
rows.sort(key=lambda l: key.match(l).groups())
for slot, i in enumerate(idx):
    lines[i] = rows[slot]
after = len([l for l in lines if key.match(l)])
if before != after:
    sys.exit(f"  refusing to write: {before} rows became {after}")
open(path, "w").write("\n".join(lines))
print(f"  {before} rows back in date order")
PY
fi

echo
echo "$ADDED added, $SKIPPED already in the ledger"

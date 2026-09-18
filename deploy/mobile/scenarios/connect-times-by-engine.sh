#!/bin/bash
# Is the call's connect time drifting, or is it two different things?
#
#   connect-times-by-engine.sh [output root]
#
# "Connect-time drift" has been an open look since cycle 66, when a 2633 ms
# connect was found dropping the first words of a sentence. It was read as
# drift because the numbers grew over the cycles: a few hundred milliseconds
# early on, then one and two seconds, then 2.6.
#
# This reads no new data. Every call run the loop has ever done printed
# `metric connect <ms> <engine> route=<route>` into its console log, and the
# logs are still on the Mac, so the question can be asked of the record
# rather than of a new experiment.
#
# The engine is the thing that changed. The phone moved to GPT Live as its
# gateway partway through, and `openai` connects cost seconds where the
# self-hosted `duplex` ones cost hundreds of milliseconds. Grouped that way
# the two distributions do not overlap at all, which is not what drift looks
# like.
set -uo pipefail
# Every other scenario here takes a cycle number first, and this one takes a
# directory. Given `127` it said "no logs under 127", which reads as "there
# are no logs" rather than "that is not what I take" — I fell into it on the
# first run of this cycle.
ROOT=${1:-$HOME/mobile-out}
case "$ROOT" in
  [0-9]*) if [ ! -d "$ROOT" ]; then
            echo "'$ROOT' looks like a cycle number. This one takes an output root,"
            echo "not a cycle: it reads every call log the loop has ever written."
            echo "Run it with no argument to read them all."
            exit 1
          fi;;
esac
[ -d "$ROOT" ] || { echo "no directory at $ROOT"; exit 1; }

grep -rhoE "metric connect [0-9]+ms [a-z]+" "$ROOT" 2>/dev/null \
  | awk '{ gsub(/ms/, "", $3); print $4, $3 }' | sort > /tmp/connects-by-engine.txt

python3 - <<'PY'
import statistics as st

rows = {}
for line in open("/tmp/connects-by-engine.txt"):
    engine, value = line.split()
    rows.setdefault(engine, []).append(int(value))

if not rows:
    raise SystemExit("no connect metrics in those logs")

print(f"{'engine':8} {'n':>4}  {'min':>6} {'median':>6} {'p90':>6} {'max':>6} {'mean':>6}")
for engine, values in sorted(rows.items()):
    values.sort()
    p90 = values[min(int(len(values) * 0.9), len(values) - 1)]
    print(f"{engine:8} {len(values):>4}  {values[0]:>6} {int(st.median(values)):>6} "
          f"{p90:>6} {values[-1]:>6} {int(st.mean(values)):>6}")

# Overlap is the whole question. If the slowest of one engine is faster than
# the fastest of the other, these are two populations and not one that
# wandered.
if len(rows) == 2:
    faster, slower = sorted(rows.items(), key=lambda kv: max(kv[1]))
    print()
    if max(faster[1]) < min(slower[1]):
        print(f"No overlap: the slowest {faster[0]} connect ({max(faster[1])} ms) is faster")
        print(f"than the fastest {slower[0]} one ({min(slower[1])} ms). Two populations, not")
        print("one that drifted.")
    else:
        print(f"The two overlap between {min(slower[1])} and {max(faster[1])} ms, so the")
        print("engine does not account for the spread on its own.")

# The pre-socket hold has to outlast the connect or the opening words are
# lost (M-246, M-248). Six seconds was chosen against the worst connect then
# known, so the margin is worth printing every time this is run.
HOLD_MS = 6000
worst = max(max(v) for v in rows.values())
print()
print(f"The hold is {HOLD_MS} ms. The worst connect on record is {worst} ms, "
      f"a margin of {HOLD_MS - worst} ms.")
if HOLD_MS - worst < 500:
    print("That is thin. A connect only slightly worse than the worst seen would")
    print("cost the caller their first words again.")
PY

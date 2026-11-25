#!/bin/bash
# swiftcount.sh — how many Swift tests actually ran, from an xcodebuild log, or no answer at all.
#
#   swiftcount.sh counts <log>              passed/failed/skipped, or exit 1 naming every line it could not read
#   swiftcount.sh recover <log> <declared>  name the tests behind any truncated lines, against a declared list
#
# Used by gate 4 on a Swift leg. Gate 4 compares test NAMES across a rebase, and a name that goes
# missing is adjudicated as a dropped test. So a counting error here does not produce a wrong number,
# it produces a phantom dropped test — or hides a real one by filling its slot.
#
# WHY NOT `Executed N tests`. That summary line is the obvious thing to reconcile against and it
# covers only part of the run. Measured on run 42277644010: the summary says `Executed 68 tests, with
# 13 tests skipped` at the `All tests` level, and all 68 are the UI bundle. The unit bundle runs
# afterwards as a second `Testing started` phase in parallel simulator clones and emits NO summary
# line at all — 551 of that log's 606 results have nothing to reconcile against. A check built on the
# summary would have read like it covered the run while covering a tenth of it.
#
# WHY TWO FORMATS. The same log reports the same kind of result two ways, and a grep written for
# either one silently undercounts by the whole of the other:
#
#   UI bundle, serial:          Test Case '-[Suite testName]' passed (61.705 seconds).
#   unit bundle, clone:         Test case 'Suite.testName()' passed on 'Clone 1 of iPhone 17' (0.04 seconds)
#
# Capital C takes a bracketed selector and no destination; lower-case c takes a dotted name and names
# the clone it ran on. Nothing in the log announces which phase you are in.
#
# WHY IT FAILS CLOSED. xcodebuild interleaves its own status writes into the same stream, and in all
# three logs examined on 2026-11-06 exactly one result line was cut in half by one — a DIFFERENT test
# in each log, with no marker that it happened. A per-case count is therefore short by an arbitrary
# one. Reporting a count with a warning beside it is no use: the warning sits in a log nobody reads
# to the end, and the count gets quoted. So an unreadable line yields NO COUNT.
set -u

# A result line, either format. Anchored at both ends: the closing quote and the verdict must both be
# there, which is exactly what a line cut by an interleaved write has lost.
UI_RESULT="^Test Case '-\[[^]]*\]' (passed|failed|skipped) \([0-9.]+ seconds?\)\.$"
UI_START="^Test Case '-\[[^]]*\]' started\.$"
CLONE_RESULT="^Test case '[^']*' (passed|failed|skipped) on '[^']*' \([0-9.]+ seconds?\)$"
CLONE_START="^Test (case|suite) '[^']*' started on '[^']*'$"
# A performance test emits one of these per metric, file:line prefixed and carrying no verdict. They
# are known output, not truncations, and they must be named as a shape rather than left to the
# unreadable set: a check that cries wolf on four legitimate lines in every log is a check someone
# turns off, and then the one line that matters goes with it.
UI_MEASURE="^/.*:[0-9]+: Test Case '-\[[^]]*\]' measured \[.*"

# Every line that claims to be about a test case, so that "unreadable" is decided over the same set
# the counts are taken from rather than over a narrower one.
mentions_a_case() { grep -nE "Test [Cc]ase '" "$1"; }

readable() { grep -nE "$UI_RESULT|$UI_START|$CLONE_RESULT|$CLONE_START|$UI_MEASURE" "$1"; }

verdicts() { grep -cE "^Test Case '-\[[^]]*\]' $1 \([0-9.]+ seconds?\)\.$|^Test case '[^']*' $1 on '[^']*' \([0-9.]+ seconds?\)$" "$2"; }

# The lines that mention a case and are not any known shape. Compared by line NUMBER, because two
# truncations can leave identical text and a text-only comparison would collapse them to one.
unreadable_lines() {
  local log=$1
  comm -23 <(mentions_a_case "$log" | cut -d: -f1 | sort -u) \
           <(readable "$log" | cut -d: -f1 | sort -u)
}

swift_counts() {
  local log=$1
  [ -f "$log" ] || { echo "swiftcount: $log does not exist — nothing judged." >&2; return 2; }

  local bad
  bad=$(unreadable_lines "$log")
  if [ -n "$bad" ]; then
    echo "swiftcount: REFUSED — $(printf '%s\n' "$bad" | grep -c .) line(s) mention a test case and are not readable." >&2
    echo "  A count taken over them would be short by that many, with nothing in it saying so." >&2
    printf '%s\n' "$bad" | while read -r n; do
      [ -n "$n" ] && echo "  line $n: $(awk -v n="$n" 'NR==n{print substr($0,1,120)}' "$log")" >&2
    done
    echo "  Run 'swiftcount.sh recover $log <declared-list>' to name them, then count again." >&2
    return 1
  fi

  local p f s
  p=$(verdicts passed "$log"); f=$(verdicts failed "$log"); s=$(verdicts skipped "$log")
  echo "passed=$p failed=$f skipped=$s total=$((p + f + s))"
}

# The fragment a truncation leaves is the text from the opening quote to wherever the interleaved
# write began, and the junk is butted straight onto the name with no separator — the observed case
# left `testEveryListedSettingsPa2026-11-06 20:22:…`, where `2026` reads as part of the name. So the
# fragment cannot be used as a prefix directly, and nothing in the line says where the name stopped.
#
# What is reliable is the COMMON PREFIX: compare the fragment against every declared name and keep
# the ones sharing the longest. The right name agrees for as far as it got; a wrong one diverges
# early. A tie is refused rather than broken, because `testEveryListedSettingsPa` matching exactly
# one declared name was luck and not a property — two tests whose names differ late would tie here,
# and picking one would invent a gate-4 verdict out of a coin toss.
RECOVER_MIN_PREFIX=12   # the limit, written beside the guard: shorter than this names too little.

swift_recover() {
  local log=$1 declared=$2
  [ -f "$declared" ] || { echo "swiftcount: declared list $declared does not exist." >&2; return 2; }

  local rc=0 n frag
  for n in $(unreadable_lines "$log"); do
    frag=$(awk -v n="$n" 'NR==n{
      i = index($0, "\x27"); if (!i) next
      rest = substr($0, i + 1)
      j = index(rest, "."); if (j) rest = substr(rest, j + 1)   # declared lists hold bare test names
      print rest
    }' "$log" | grep -oE "^test[A-Za-z0-9_]*")
    if [ -z "$frag" ]; then
      echo "line $n: UNRECOVERABLE — no test-name fragment survived the truncation." >&2; rc=1; continue
    fi

    local best
    best=$(awk -v frag="$frag" -v min="$RECOVER_MIN_PREFIX" '
      { n = 0
        while (n < length($0) && n < length(frag) && substr($0, n + 1, 1) == substr(frag, n + 1, 1)) n++
        if (n >= min) { if (n > top) { top = n; names = $0; c = 1 } else if (n == top) { names = names "\n" $0; c++ } }
      }
      END { if (c) { print top " " c; print names } }' "$declared")

    if [ -z "$best" ]; then
      echo "line $n: NO MATCH — fragment '$frag' shares no $RECOVER_MIN_PREFIX-character prefix with any declared name." >&2
      rc=1; continue
    fi
    # First line carries the two numbers, every line after it is a candidate name — names can be
    # many, so they cannot share a line with the counts.
    local len cnt names
    len=$(printf '%s\n' "$best" | awk 'NR==1{print $1}')
    cnt=$(printf '%s\n' "$best" | awk 'NR==1{print $2}')
    names=$(printf '%s\n' "$best" | awk 'NR>1')
    if [ "$cnt" = 1 ]; then
      echo "line $n: '$frag' -> $names (agrees for $len characters)"
    else
      echo "line $n: AMBIGUOUS — '$frag' agrees for $len characters with $cnt declared names, so it names none:" >&2
      printf '%s\n' "$names" | while read -r m; do echo "    $m" >&2; done
      rc=1
    fi
  done
  return $rc
}

case ${1:-} in
  counts) shift; swift_counts "$@" ;;
  recover) shift; swift_recover "$@" ;;
  "") : ;;  # sourced
  *) echo "usage: swiftcount.sh counts <log> | recover <log> <declared-list>" >&2; exit 2 ;;
esac

#!/bin/bash
# selftest.sh — the merge kit's checks on itself. No network, no git, no credentials.
# Run it directly: scripts/merge/selftest.sh
#
# WHAT THIS IS FOR, because it reads like a test of logging and is not.
#
# The functions below decide which verdicts refuse a merge. A verdict word that
# no caller recognises falls into a default arm, and one of those default arms
# — after()'s — prints WAIT. WAIT is not a refusal: it tells the operator to
# sit and wait for a run that has already finished. So a new word added to
# describe a *failure* can quietly start permitting one, and it would do it
# while every visible symptom looked like better diagnostics.
#
# These cases therefore assert the REFUSAL and not the wording. Asserting that
# "UNTESTED" appears somewhere would pass just as happily in the broken state.
set -u
D=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=scripts/merge/checks.sh
source "$D/checks.sh"

fail=0
ok() { printf '  ok    %s\n' "$1"; }
bad() { printf '  FAIL  %s\n' "$1"; fail=1; }

# Source with whole-line comments stripped. Every case below that reads the
# source must go through these, and the reason is not tidiness: the comments in
# merge-window.sh necessarily name the identifiers these cases grep for, since
# they exist to explain those very calls. A grep over raw text therefore matches
# the explanation of a call that has been deleted. One case did exactly that
# before this helper existed — it passed against a mutant that removed the call
# and left the comment, which is a case that cannot fail.
nocomment() { grep -vE '^[[:space:]]*#'; }
code=$(nocomment <"$D/merge-window.sh")
body() { sed -n "/^$1() {/,/^}/p" "$D/merge-window.sh" | nocomment; }

# 1. Every line verdict() can produce, and the class it has to take. UNKNOWN is
#    a real row: an unregistered word must land somewhere that refuses.
echo "verdict_class — every verdict line the kit can emit:"
while IFS='|' read -r want line; do
  [ -n "$want" ] || continue
  got=$(verdict_class "$line")
  if [ "$got" = "$want" ]; then ok "$want"
  else bad "expected $want, got $got, for: $line"; fi
done <<'CASES'
TAG|TAG v0.6.380
NOMAC|NOMAC v0.6.380 (run 7) no Mac change since v0.6.380 newest green commit e445f70a
NOPATH|NOPATH apps/ios/** only — ci.yml runs no push CI for it, so no Release plan and no Mac tag, by design
COVERED|COVERED v0.6.380 (run 7)
REDGATE|RED GATE required check 'Swift core check' on e445f70ad: NO VERDICT — no run exists
RED|RED CI completed failure
UNTESTED|UNTESTED CI completed failure — run 41761774906 has 20 step(s) that never started and none that failed
WAIT|WAIT no CI run yet
WAIT|WAIT CI in_progress null
BLOCKED|BLOCKED reading CI runs for e445f70a: HTTP 403
UNKNOWN|PURPLE a word nobody has written yet
CASES

# 2. Which classes may clear a GO. This is the gate, stated once.
echo "verdict_clears_go — the refusal set is exactly NOMAC and NOPATH:"
for c in TAG NOMAC NOPATH COVERED REDGATE RED UNTESTED WAIT BLOCKED UNKNOWN; do
  if verdict_clears_go "$c"; then got=clears; else got=refuses; fi
  case "$c" in NOMAC | NOPATH) want=clears ;; *) want=refuses ;; esac
  if [ "$got" = "$want" ]; then ok "$c $got"
  else bad "$c $got, expected $want"; fi
done

# 3. The one that this change could break, named on its own so a reader of a
#    failure sees the reason and not just a row of a table.
echo "the point of the change:"
if verdict_clears_go UNTESTED; then
  bad "UNTESTED clears a GO. A run that aborted tested nothing, which is the definition of ungated, so it is exactly the state a gate must refuse. Diagnosing it better must not start permitting it."
else
  ok "UNTESTED still refuses a GO"
fi

# 4. Both branching callers must recognise every refusing class. after() is the
#    dangerous one: its default arm prints WAIT, so a missing branch there is a
#    refusal turning into a shrug. This reads the source because that is where
#    the arms are; the functions themselves need a network to run.
echo "both callers recognise every refusing verdict:"
for fn in go after; do
  src=$(body "$fn")
  if [ -z "$src" ]; then bad "cannot find $fn() in merge-window.sh"; continue; fi
  for w in RED UNTESTED BLOCKED; do
    if printf '%s\n' "$src" | grep -qE "(^|[|( ])$w(\*|\))"; then ok "$fn() handles $w"
    else bad "$fn() has no branch for $w, so it falls to that function's default arm"; fi
  done
done

# 5. after()'s default arm. Every verdict() string has an arm of its own, so
#    this arm is reached only by a word somebody added without reading the
#    case — which is precisely when it must not read as progress. It printed
#    WAIT until 2026-11-04.
echo "after()'s default arm refuses rather than shrugs:"
after_src=$(body after)
default_arm=$(printf '%s\n' "$after_src" | grep -E '^\s+\*\)')
if [ -z "$default_arm" ]; then
  bad "after() has no default arm at all, so an unrecognised verdict prints nothing"
elif printf '%s\n' "$default_arm" | grep -q 'WAIT'; then
  bad "after()'s default arm says WAIT. An unrecognised verdict is a state nobody has reasoned about; telling the operator to wait for it is treating an unknown as progress."
else
  ok "the default arm does not say WAIT"
fi
if printf '%s\n' "$after_src" | grep -qE '^\s+WAIT\*\)'; then
  ok "WAIT has an explicit arm, so moving the default did not silence it"
else
  bad "WAIT has no explicit arm in after(), so a genuinely-still-running verdict now falls to the default"
fi

# 6. go() must make its real clearing decision THROUGH verdict_clears_go().
#    Until 2026-11-04 it did not: it kept its own literal `case`, and this file
#    asserted the refusal set against a function no production path consulted.
#    Every case above passed while the decision that matters sat elsewhere and
#    was free to drift — this PR's own subject, one layer up. These two cases
#    are what make block 2 an assertion about the kit rather than about itself.
echo "go() decides through verdict_clears_go() and not a private copy:"
go_src=$(body go)
if printf '%s\n' "$go_src" | grep -q 'verdict_clears_go'; then
  ok "go() consults verdict_clears_go"
else
  bad "go() no longer calls verdict_clears_go, so the refusal set asserted above is a replica and the real decision is unpinned"
fi
# The tag check is the call site's, by design: verdict_clears_go() answers for
# the class, not for this release. Drop it and a NOMAC naming an OLDER tag
# starts clearing — a stale verdict about a different release passing as
# judgement on this one, which is fail-open.
if printf '%s\n' "$go_src" | grep -q '"NOMAC \$tag"\*'; then
  ok "NOMAC is still gated on the tag being released"
else
  bad "go() no longer checks that a NOMAC names the current tag, so a NOMAC about an older release would clear this one"
fi

# 7. verdict() must actually route a non-success run through the classifier;
#    without this the two functions above are correct and unreached.
echo "verdict() routes a terminal non-success run through ci_abort_state:"
if printf '%s\n' "$code" | grep -q 'completed\*) ci_abort_state'; then
  ok "the completed* arm calls ci_abort_state"
else
  bad "verdict()'s completed* arm no longer calls ci_abort_state"
fi
# ...and the exact-match arm must stay AHEAD of the glob. Behind it, every
# green run reaches ci_abort_state, which finds no step concluded failure and
# answers UNTESTED — every passing commit refused. Fail-closed and loud, but
# nothing pinned the order until now.
sucline=$(printf '%s\n' "$code" | grep -n '"completed success")' | head -1 | cut -d: -f1)
globline=$(printf '%s\n' "$code" | grep -n 'completed\*) ci_abort_state' | head -1 | cut -d: -f1)
if [ -z "$sucline" ]; then
  bad "verdict() has no exact-match arm for 'completed success', so a green run falls to the glob and is read as an abort"
elif [ -z "$globline" ]; then
  : # the case above already reported the missing glob arm; nothing to order
elif [ "$sucline" -lt "$globline" ]; then
  ok "'completed success' is matched before the completed* glob"
else
  bad "the completed* glob now precedes 'completed success' (lines $globline then $sucline), so every green run is classified UNTESTED"
fi

# 8. A gate run that died before judging must be BLOCKED, and one that judged
#    and refused must be RED. Both are asserted, and the pairing is the case:
#    a change that answered BLOCKED for everything would satisfy the 127 half
#    on its own while quietly letting a real refusal clear as "the gate's
#    fault, not the pull request's". That is the fail-open direction, so the
#    RED half is the one load-bearing against this change's own worst outcome.
#
#    gate_state is network code, so `rd` is replaced here with the two runs
#    this was measured on. The bodies are GitHub's, trimmed to the fields the
#    classifier reads: 41846184562 is #2713's head, which exits 127 at
#    `Can this run decide anything?`, and 41845315633 is #2717's head, refused
#    on main's real outstanding TestFlight build.
echo "gate_state — an instrument failure is BLOCKED and a real refusal is RED:"
jobs_127='{"jobs":[
  {"name":"Release gate","conclusion":"failure","steps":[
    {"name":"Set up job","conclusion":"success"},
    {"name":"Run actions/checkout@v4","conclusion":"success"},
    {"name":"Can this run decide anything?","conclusion":"failure"},
    {"name":"Run actions/setup-python@v5","conclusion":"skipped"},
    {"name":"Does merging this collapse two fixes into one release?","conclusion":"skipped"},
    {"name":"Post Run actions/checkout@v4","conclusion":"success"},
    {"name":"Complete job","conclusion":"success"}]},
  {"name":"Release gate refuses when it should","conclusion":"success","steps":[
    {"name":"Run scripts/release/release-gate-selftest.sh","conclusion":"success"}]}]}'
jobs_refused='{"jobs":[
  {"name":"Release gate","conclusion":"failure","steps":[
    {"name":"Set up job","conclusion":"success"},
    {"name":"Run actions/checkout@v4","conclusion":"success"},
    {"name":"Can this run decide anything?","conclusion":"success"},
    {"name":"Run actions/setup-python@v5","conclusion":"success"},
    {"name":"Does merging this collapse two fixes into one release?","conclusion":"failure"},
    {"name":"Post Run actions/setup-python@v5","conclusion":"skipped"},
    {"name":"Complete job","conclusion":"success"}]},
  {"name":"Release gate refuses when it should","conclusion":"success","steps":[
    {"name":"Run scripts/release/release-gate-selftest.sh","conclusion":"success"}]}]}'
jobs_startup='{"jobs":[{"name":"Release gate","conclusion":"failure","steps":[]}]}'

R=unarbos/arbos
gate_state_with() {  # <jobs-body> — one red run on the head, whose jobs are <jobs-body>
  local body=$1
  # Dispatch on the field each call asks for, never on per_page: `per_page=1`
  # is a prefix of `per_page=100`, so a glob on it answers the run listing
  # with the count and every case below reads PENDING instead of its verdict.
  rd() {
    case "$*" in
      *total_count*)   printf '1' ;;
      *workflow_runs*) printf '%s' "41846184562 pull_request completed failure" ;;
      */jobs*)         printf '%s' "$body" ;;
      *)               printf '' ;;
    esac
  }
  gate_state .github/workflows/release-gate.yml 0123456789012345678901234567890123456789
  unset -f rd
}
for case_name in 127:jobs_127:BLOCKED refusal:jobs_refused:RED startup:jobs_startup:BLOCKED; do
  label=${case_name%%:*}; rest=${case_name#*:}; var=${rest%%:*}; want=${rest##*:}
  got=$(gate_state_with "${!var}")
  if [ "${got%% *}" = "$want" ]; then ok "$label reads $want"
  else bad "$label reads '${got%% *}', expected $want — full line: $got"; fi
done
# Named on its own, because it is the case this change could break and the
# table above would still look healthy if the classifier collapsed to one
# answer: the two live runs must not land on the same word.
a=$(gate_state_with "$jobs_127"); b=$(gate_state_with "$jobs_refused")
if [ "${a%% *}" != "${b%% *}" ]; then
  ok "the 127 run and the refused run are distinguishable"
else
  bad "both runs now read '${a%% *}', so gate_state has collapsed back to one answer for three different things"
fi

# 9. The summary line go() adds for a pull request must keep require_gates'
#    three answers apart. A BLOCKED gate summarised as "no passing run" sends
#    the reader to change the branch, which is the same mistake as case 8 and
#    is the one the operator actually reads.
echo "go() summarises a pull request's gates by which of the three answers it got:"
if printf '%s\n' "$code" | grep -q 'require_gates "\$phead" "\$pfiles"; local prc=\$?'; then
  ok "the return code is captured rather than collapsed with ||"
else
  bad "go() no longer captures require_gates' return code, so BLOCKED, PENDING and RED share one summary again"
fi
if printf '%s\n' "$code" | grep -q 'could not be read on \$phead'; then
  ok "a gate that could not be read has its own summary"
else
  bad "go() has no distinct summary for an unreadable gate, so an instrument failure reads as the pull request's failure"
fi

echo
if [ "$fail" -eq 0 ]; then echo "selftest: PASS"; else echo "selftest: FAIL"; fi
exit "$fail"

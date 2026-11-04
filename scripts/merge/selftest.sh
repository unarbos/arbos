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

# 8. nomac_covers(): when a commit judged against an OLDER tag may be cleared by
#    a newer commit judged against the current one.
#
#    The danger in this change is not that it refuses too much, it is that it
#    clears too much, so the cases are written around the two ways it could:
#    clearing a commit that is NOT contained by the covering one, and clearing
#    anything at all when no current-tag verdict was ever recorded. The second is
#    the real-debt path — genuine unshipped Mac work never produces a current-tag
#    NOMAC, so nothing sets `covering` — and it is asserted on its own below.
#
#    Real commits in a scratch repository, because the function's whole job is an
#    ancestry question and a stub would be asserting my idea of git's answer.
echo "nomac_covers — an ancestor is covered, anything else is not:"
# The SHIPPED function, lifted out and evaluated — not retyped. merge-window.sh
# ends in a top-level `case` dispatch so it cannot be sourced, which is the same
# bind auto-stable-selftest.sh is in with is_hotfix, and this is its answer.
#
# A copy here would be worse than no case at all: every mutation of the real
# function would leave the copy untouched, so the whole of case 8 would pass
# against a broken merge-window.sh. That is not hypothetical — this file carried
# such a copy while it was being written, and three of six mutations survived it.
nomac_covers_src=$(sed -n '/^nomac_covers()/,/^}/p' "$D/merge-window.sh")
if [ -z "$nomac_covers_src" ]; then
  bad "no nomac_covers() in merge-window.sh — it was renamed or removed, and case 8 is testing nothing"
else
  eval "$nomac_covers_src"
fi
repo=$(mktemp -d) || exit 1
trap 'rm -rf "$repo"' EXIT
cd "$repo" || exit 1
git init -q .; git config user.email t@example.com; git config user.name test
git commit -q --allow-empty -m older
git commit -q --allow-empty -m middle
git commit -q --allow-empty -m tip
# `git init` names the first branch master or main depending on the host's
# config, and a failed `rev-parse main~2` prints its own argument on stdout
# before exiting non-zero — so a `main || master` fallback yields two lines,
# not a sha, and every ancestry answer below would be about a bad revision.
git branch -M trunk
git checkout -q -b sibling trunk~2
git commit -q --allow-empty -m "off the first-parent line"
old=$(git rev-parse trunk~2); mid=$(git rev-parse trunk~1)
tip=$(git rev-parse trunk);   sib=$(git rev-parse sibling)
for r in old mid tip sib; do
  case "${!r}" in *[!0-9a-f]* | "") bad "scratch repo: $r is not a sha ('${!r}'), so case 8 would prove nothing"; esac
done

# Values, not variable names. Driving this through ${!var} is what shellcheck
# cannot follow, and a fixture it reads as unused is the shape of a case that
# proves nothing.
covers_is() {  # <clears|refuses> <sha> <covering-sha> <what>
  local got=refuses; nomac_covers "$2" "$3" && got=clears
  if [ "$got" = "$1" ]; then ok "$4"; else bad "$4 — got '$got', wanted '$1'"; fi
}
covers_is clears  "$old" "$tip" "an ancestor of the covering commit is cleared"
covers_is clears  "$mid" "$tip" "the commit just below the covering one is cleared"
covers_is clears  "$tip" "$tip" "the covering commit covers itself"
covers_is refuses "$sib" "$tip" "a commit that is NOT an ancestor is refused"
covers_is refuses "$tip" "$old" "a commit NEWER than the covering one is refused"
# The real-debt path, named on its own because it is the one that must never
# clear: no current-tag NOMAC anywhere means `covering` is empty.
empty=""
if nomac_covers "$old" "$empty"; then
  bad "with no current-tag verdict recorded at all, a commit was still cleared — this is the real-debt path and it must refuse"
else
  ok "with no current-tag verdict recorded, nothing is cleared"
fi
cd "$D" || exit 1

# 9. The guards in go() that keep case 8 honest. nomac_covers is only safe
#    because of WHERE it is called and WHAT sets its second argument; tested
#    alone it would pass just as well in a go() that had lost either.
echo "go() reaches nomac_covers only from a current-tag NOMAC:"
gobody=$(body go)
if printf '%s\n' "$gobody" | grep -q 'nomac_covers "\$sha" "\$covering"'; then
  ok "the clearing branch calls nomac_covers"
else
  bad "go() no longer calls nomac_covers, so an older-tag NOMAC either refuses always or clears unchecked"
fi
# `covering` must be assigned on the exact-match arm and nowhere else. An
# assignment anywhere looser is the whole defect back again, wearing the new
# function as cover.
assigns=$(printf '%s\n' "$gobody" | grep -c 'covering=')
exact=$(printf '%s\n' "$gobody" | grep -c '"NOMAC \$tag"\*) covering=')
if [ "$exact" -eq 1 ] && [ "$assigns" -eq 2 ]; then
  ok "covering is set on the exact \"NOMAC \$tag\" match and nowhere else (one init, one assignment)"
else
  bad "covering is assigned $assigns time(s), $exact of them on the exact tag match — any other source lets a weaker verdict clear the window"
fi
if printf '%s\n' "$gobody" | grep -q 'NOMAC).*not since \$tag\|NOT since \$tag'; then
  ok "the refusal names the tag the commit WAS judged against as well as the one being released"
else
  bad "the NOMAC refusal does not name both tags, so the reader must infer the mismatch from the parenthetical"
fi

# 10. The two states behind "no run exists", which need different words because
#     they need different actions.
#
#     A gate that is required by path and has no run is one sentence in the old
#     kit and two situations in reality. If the workflow was not in the tree a
#     run would have started from, GitHub created nothing and nothing will ever
#     appear while the head sits still — the remedy is to move the head. If the
#     workflow WAS in that tree, a run should exist, and its absence is more
#     likely to be a bad query or a deleted run than a fact about the head.
#
#     The trap this block is written around is a branch that can only ever reach
#     the first answer. Such a branch turns every broken query into a soothing
#     "just rebase", which is the same defect as the sentence it replaces,
#     wearing better prose. So the MISSING case is asserted on its own, and
#     every case also asserts the phrases of its neighbour are ABSENT from it.
#
#     Driven through the REAL require_gates, gate_applies, gate_state and
#     no_run_state. Only `rd` — the one function in checks.sh that touches the
#     network — is replaced, and it answers per URL so each case is driven by
#     the world it describes rather than by one blanket reply. It also records
#     every URL it was asked for, which is how the last two cases can assert
#     that a read was never made at all.
echo "require_gates separates an absent workflow from a genuinely missing run:"

R=unarbos/arbos
GATE_BASE=main
HEAD40=1111111111111111111111111111111111111111
MB40=2222222222222222222222222222222222222222
FIXFILES="apps/ios/Arbos/App.swift"
# A workflow whose filter the fixture diff touches, so gate_applies says "applies"
# and every case below reaches the part being tested.
FIXWF=$(printf 'name: X\non:\n  pull_request:\n    paths:\n      - "apps/ios/**"\n' | base64 | tr -d '\n')

# FIX_TREES   refs whose tree contains the workflow; any other ref 404s.
# FIX_RUNS    the runs listing for the head; empty means total_count 0.
# FIX_NOCMP   non-empty makes the compare read fail, as a 403 would.
# FIX_LOG     every URL asked for, one per line.
rd() {
  local url="" a
  for a in "$@"; do case "$a" in repos/*) url=$a ;; esac; done
  printf '%s\n' "$url" >> "$FIX_LOG"
  case "$url" in
    */compare/*)
      [ -z "$FIX_NOCMP" ] || { echo "BLOCKED:HTTP 403: Forbidden"; return 1; }
      printf '%s' "$MB40" ;;
    */contents/*)
      local ref=${url##*ref=}
      case " $FIX_TREES " in
        *" $ref "*) printf '%s' "$FIXWF" ;;
        *) echo "BLOCKED:HTTP 404: Not Found"; return 1 ;;
      esac ;;
    # Longest first: `per_page=1` is a prefix of `per_page=100`, so the count arm
    # would otherwise answer the listing call with a count. It did, once.
    *per_page=100*) printf '%s' "$FIX_RUNS" ;;
    *per_page=1*)   printf '%s' "$(printf '%s\n' "$FIX_RUNS" | grep -c .)" ;;
    *) echo "BLOCKED:the fixture was asked for an URL no case describes: $url"; return 1 ;;
  esac
}

# gates_is <name> <want rc> <phrases that must appear> <phrases that must NOT>
#
# The must-not list is the half that stops this block testing one branch twice.
# Each "no run" case names its neighbour's wording, so a no_run_state collapsed
# back to a single sentence fails whichever case it does not happen to match —
# and a require_gates that printed both would fail both.
gates_is() {
  local name=$1 want=$2 why=$3 whynot=$4 out got=0 p miss=""
  : > "$FIX_LOG"
  out=$(require_gates "$HEAD40" "$FIXFILES") || got=$?
  CASE_OUT=$out
  while IFS= read -r p; do
    [ -n "$p" ] || continue
    case "$out" in *"$p"*) ;; *) miss="$miss
      wanted and did not find: $p" ;; esac
  done <<< "$why"
  while IFS= read -r p; do
    [ -n "$p" ] || continue
    case "$out" in *"$p"*) miss="$miss
      found and must not have: $p" ;; esac
  done <<< "$whynot"
  if [ "$got" = "$want" ] && [ -z "$miss" ]; then ok "$name (rc $got)"
  else bad "$name: wanted rc $want, got $got$miss
      output: $out"; fi
}

FIX_LOG=$(mktemp); FIX_NOCMP=""; FIX_RUNS=""
RECORD=$(mktemp)
trap 'rm -f "$FIX_LOG" "$RECORD"' EXIT
# One gate at a time, so each case's output is about the gate it names. GATES is
# restored at the end of the block.
REAL_GATES=$GATES
GATES='.github/workflows/release-gate.yml|Release gate'

# #2695's state, in the shape the kit sees it. main carries the workflow; the
# head and its merge base forked before it landed; no run exists and none can.
#
# What could contradict this: if GitHub did start runs from main's current tree
# rather than the base's, #2695 would have a run and this whole state would be
# unreachable. The control is the next case, where the only thing that changes
# is whether the merge base carries the file — and the answer changes with it.
FIX_TREES="main"
gates_is "a head whose base predates the workflow says so, and says to move the head" 1 \
"NO VERDICT — required by path, and the workflow did not exist in this head's base
merge base 222222222
Move the head
unrun, not failing" \
"did exist in this head's base
BLOCKED"
printf '%s\n' "$CASE_OUT" >> "$RECORD"

# The control, and the case the user asked for by name: same head, same empty
# run list, the ONE difference being that the merge base does carry the
# workflow. A branch that could only say "absent" passes the case above and
# fails here, which is the only reason that case is worth running.
FIX_TREES="main $MB40 $HEAD40"
gates_is "a head whose base DID carry the workflow blocks on the missing run instead" 2 \
"BLOCKED — required by path, the workflow did exist in this head's base, and no run exists for this head
Rule the query out before reading it as a missing verdict
merge base 222222222" \
"the workflow did not exist in this head's base
Move the head
NO VERDICT"
printf '%s\n' "$CASE_OUT" >> "$RECORD"

# The merge base itself unreadable. Neither answer is available, so neither may
# be given: a compare that 403s must not be rounded to "absent" just because
# absent is the common case.
FIX_TREES="main"; FIX_NOCMP=1
gates_is "an unreadable merge base is blocked, not rounded to absent" 2 \
"BLOCKED
cannot read 11111111's merge base with main
HTTP 403" \
"the workflow did not exist in this head's base
did exist in this head's base
Move the head"
printf '%s\n' "$CASE_OUT" >> "$RECORD"
FIX_NOCMP=""

# A head with a passing run must not reach any of this. Asserted on the fixture
# log rather than on the words: a case that only checked for the absence of a
# phrase would pass against a no_run_state that ran and then said nothing.
FIX_TREES="main $MB40 $HEAD40"; FIX_RUNS="99 pull_request completed success"
gates_is "a passing run is still a pass" 0 "PASS run 99" \
"no run exists
merge base"
printf '%s\n' "$CASE_OUT" >> "$RECORD"
if grep -q '/compare/' "$FIX_LOG"; then
  bad "a head with a passing run still paid for a merge-base compare, so every commit verdict() walks now costs one"
else
  ok "a passing run asks for no merge base at all"
fi

# A gate the diff does not touch must not be investigated either. The fixture
# diff is replaced with a file outside the filter, so gate_applies answers
# first and nothing downstream runs.
FIXFILES="README.md"
gates_is "a gate the diff does not touch stays not-required" 0 "not required" \
"no run exists
merge base
Move the head"
if grep -qE '/compare/|/runs' "$FIX_LOG"; then
  bad "a gate that does not apply still read runs or a merge base"
else
  ok "a gate that does not apply reads neither runs nor a merge base"
fi
FIXFILES="apps/ios/Arbos/App.swift"; FIX_RUNS=""

# The same audit the release gate's cases get: a phrase that also matches a
# neighbour's output cannot tell its own case from that neighbour, so the case
# is evidence about the pair and not about either one.
echo "each 'no run' case's output is distinguishable from the others:"
dupes=$(sort "$RECORD" | uniq -d | grep -c . )
if [ "$dupes" -eq 0 ]; then
  ok "all $(grep -c . "$RECORD") recorded outputs differ from one another"
else
  bad "$dupes recorded case output(s) are identical to another case's, so those cases cannot be told apart"
fi

GATES=$REAL_GATES
unset -f rd

# 11. The second caller. range_gate() in merge-window.sh had its own copy of the
#     old one-sentence NONE message, and this file's header is about two case
#     statements that must agree and did not. A distinction kept in two places
#     is one that drifts, so the arm must route through no_run_state rather
#     than grow a second pair of words.
#
#     Read from the source with comments stripped, for the reason nocomment()
#     exists: the comment beside the call names the function, so a raw grep
#     would match the explanation of a call somebody deleted.
echo "range_gate() splits the two states through checks.sh and not a second copy:"
rg_src=$(body range_gate)
if [ -z "$rg_src" ]; then
  bad "cannot find range_gate() in merge-window.sh, so case 11 is testing nothing"
else
  if printf '%s\n' "$rg_src" | grep -q 'no_run_state'; then
    ok "range_gate() calls no_run_state"
  else
    bad "range_gate() no longer calls no_run_state, so its NONE arm has gone back to one sentence for two states"
  fi
  if printf '%s\n' "$rg_src" | grep -q 'ABSENT\*)' && printf '%s\n' "$rg_src" | grep -q 'MISSING\*)'; then
    ok "range_gate() has an arm for each state"
  else
    bad "range_gate() is missing an arm for ABSENT or MISSING, so one of them falls through to the other's words"
  fi
  if printf '%s\n' "$rg_src" | grep -q 'no run of \$wf exists for this sha at all'; then
    bad "range_gate() still carries the old single sentence, which is the wording this change exists to split"
  else
    ok "the old single sentence is gone from range_gate()"
  fi
fi

echo
if [ "$fail" -eq 0 ]; then echo "selftest: PASS"; else echo "selftest: FAIL"; fi
exit "$fail"

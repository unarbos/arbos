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
# opread.sh defines functions and does nothing else. Block 12 drives them against a FAKE op on a sandbox
# PATH with a placeholder token, so sourcing it here still reads no credential and reaches no network.
# shellcheck source=scripts/merge/opread.sh
source "$D/opread.sh"

# R is the repository every API path in checks.sh is built from. Nothing here calls the API —
# each case stubs the fetch — but a stubbed function is still CALLED with its arguments built
# first, and under `set -u` an unset R makes building that string a fatal error before the stub
# runs. The case then reads the error as the classifier's own answer.
#
# This is not hypothetical and it is why the default is here: case 10 passed on a developer
# machine and failed in CI, because a shell that has run merge-window.sh has R exported and a
# fresh runner does not. A case whose result depends on who is watching is worse than no case.
R=${R:-unarbos/arbos}

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
NOPLAN|NOPLAN no Release plan has judged dd0d8db1, and none ever will: the newest green commit they name is ddb5ec06
UNKNOWN|PURPLE a word nobody has written yet
CASES

# 2. Which classes may clear a GO. This is the gate, stated once.
#    "May clear" is not "clears": NOMAC and NOPLAN both still have to find a
#    covering descendant in go() before anything is cleared. Case 11 pins that.
echo "verdict_clears_go — the refusal set is exactly NOMAC, NOPATH and NOPLAN:"
for c in TAG NOMAC NOPATH NOPLAN COVERED REDGATE RED UNTESTED WAIT BLOCKED UNKNOWN; do
  if verdict_clears_go "$c"; then got=clears; else got=refuses; fi
  case "$c" in NOMAC | NOPATH | NOPLAN) want=clears ;; *) want=refuses ;; esac
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
git commit -q --allow-empty -m "the tag"
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
# `anchor` stands in for the released tag. It must be BELOW every commit under
# test, because the range nomac_covers measures is exclusive of its left end —
# a fixture that used the oldest commit as both the anchor and a subject would
# assert that a commit is not covered by itself and call that a pass.
anchor=$(git rev-parse trunk~3)
old=$(git rev-parse trunk~2); mid=$(git rev-parse trunk~1)
tip=$(git rev-parse trunk);   sib=$(git rev-parse sibling)
for r in anchor old mid tip sib; do
  case "${!r}" in *[!0-9a-f]* | "") bad "scratch repo: $r is not a sha ('${!r}'), so case 8 would prove nothing"; esac
done

# Values, not variable names. Driving this through ${!var} is what shellcheck
# cannot follow, and a fixture it reads as unused is the shape of a case that
# proves nothing.
covers_is() {  # <clears|refuses> <sha> <covering-sha> <anchor> <what>
  local got=refuses; nomac_covers "$2" "$3" "$4" && got=clears
  if [ "$got" = "$1" ]; then ok "$5"; else bad "$5 — got '$got', wanted '$1'"; fi
}
covers_is clears  "$old" "$tip" "$anchor" "an ancestor of the covering commit is cleared"
covers_is clears  "$mid" "$tip" "$anchor" "the commit just below the covering one is cleared"
covers_is clears  "$tip" "$tip" "$anchor" "the covering commit covers itself"
covers_is refuses "$sib" "$tip" "$anchor" "a commit that is NOT an ancestor is refused"
covers_is refuses "$tip" "$old" "$anchor" "a commit NEWER than the covering one is refused"
# The anchor is load-bearing, not decoration. A commit at or below the anchor is
# outside the range the covering verdict was recorded against, so it must refuse
# however plainly it is an ancestor — this is the older-tag trap pointed the
# other way, and the reason the tag is an argument rather than an assumption.
covers_is refuses "$anchor" "$tip" "$anchor" "the anchor itself is not covered by a verdict anchored at it"
covers_is refuses "$old"    "$tip" "$mid"    "an anchor ABOVE the commit leaves it uncovered"
# Off the first-parent line. `sib` is a genuine ancestor of nothing here, but the
# stronger statement is that first-parent is measured: a merge's second parent is
# reachable and must still not clear.
git checkout -q trunk
git merge -q --no-ff -m "merge the sibling" sibling
merged=$(git rev-parse trunk)
covers_is refuses "$sib"    "$merged" "$anchor" "a commit reachable only through a merge's SECOND parent is refused"
covers_is clears  "$tip"    "$merged" "$anchor" "the merge's first parent is still cleared"
# The real-debt path, named on its own because it is the one that must never
# clear: no current-tag NOMAC anywhere means `covering` is empty.
empty=""
if nomac_covers "$old" "$empty" "$anchor"; then
  bad "with no current-tag verdict recorded at all, a commit was still cleared — this is the real-debt path and it must refuse"
else
  ok "with no current-tag verdict recorded, nothing is cleared"
fi
# With HEAD parked ON the anchor, because that is where an absent anchor is
# dangerous rather than merely wrong: `git rev-list --first-parent "..$2"` reads
# an empty left end as HEAD, so a dropped guard would silently answer a question
# about wherever the checkout happens to be and clear exactly as if the tag had
# been supplied. Asserted from the position that makes the two indistinguishable.
git checkout -q "$anchor"
if nomac_covers "$old" "$tip" ""; then
  bad "with no anchor, a commit was cleared — an empty left end means HEAD, so the tag must be checked and not merely interpolated"
else
  ok "with no anchor, nothing is cleared"
fi
git checkout -q trunk
cd "$D" || exit 1

# 9. The guards in go() that keep case 8 honest. nomac_covers is only safe
#    because of WHERE it is called and WHAT sets its second argument; tested
#    alone it would pass just as well in a go() that had lost either.
echo "go() reaches nomac_covers only from a current-tag NOMAC:"
gobody=$(body go)
if printf '%s\n' "$gobody" | grep -q 'nomac_covers "\$sha" "\$covering" "\$tag"'; then
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

# 10. gate_push_triggered() reads the REAL workflows in this tree, not a fixture.
#     A fixture here would be the defect it guards against: the whole claim is
#     "this workflow cannot run on a push", and a hand-written `on:` block proves
#     that about the fixture. Add `push:` to release-gate.yml and case 10a must
#     go red, because the skip it authorises stops being true that moment.
echo "gate_push_triggered reads each gate's own \`on:\` block out of this tree:"
push_says() {  # <workflow path> <expected: PUSH|NOPUSH> <label>
  local f=$1 want=$2 got
  [ -f "$D/../../$f" ] || { bad "$3 — $f is not in this tree, so this case is testing nothing"; return; }
  # The only stub is the fetch. The classifier under test reads the real bytes.
  rd() { base64 -w0 < "$D/../../$f"; }
  if gate_push_triggered "$f" main >/dev/null 2>&1; then got=PUSH; else
    case $? in 1) got=NOPUSH ;; *) got=UNKNOWN ;; esac
  fi
  unset -f rd
  if [ "$got" = "$want" ]; then ok "$3 reads $got"
  else bad "$3 reads $got, expected $want"; fi
}
push_says .github/workflows/release-gate.yml     NOPUSH "release-gate.yml (pull_request only)"
push_says .github/workflows/swift-core-check.yml PUSH   "swift-core-check.yml (has push:)"
push_says .github/workflows/ci.yml               PUSH   "ci.yml (has push:)"

# 11. The fail-open direction, which is the one this change could create. The
#     skip is authorised ONLY by "no run exists". A run that exists and failed is
#     a verdict about this commit and must still refuse, or a pull_request-only
#     gate's real refusal would be skipped along with its absences.
echo "landed_gate_state skips only an ABSENT run, never a verdict:"
landed_with() {  # <gate_state answer> <has second parent: yes|no>
  local answer=$1 p2=$2
  gate_state() { printf '%s' "$answer"; }
  git() { case "$*" in *"rev-parse -q --verify"*) [ "$p2" = yes ] && printf '%s' 0123456789012345678901234567890123456789 ;; *) command git "$@" ;; esac; }
  gate_push_triggered() { echo "it has no push: trigger"; return 1; }
  landed_gate_state .github/workflows/release-gate.yml deadbeefdeadbeefdeadbeefdeadbeefdeadbeef
  unset -f gate_state git gate_push_triggered
}
for pair in 'NONE|NOTASKED|an absent run on a pull_request-only gate is skipped' \
            'RED 1 run(s) on this head, none successful|RED|a RED verdict still refuses' \
            'PENDING 1 run(s)|PENDING|a PENDING run still waits' \
            'BLOCKED could not read|BLOCKED|an unreadable gate still blocks'; do
  IFS='|' read -r answer want label <<< "$pair"
  got=$(landed_with "$answer" no)
  if [ "${got%% *}" = "$want" ]; then ok "$label"
  else bad "$label — got '${got%% *}', expected $want (full line: $got)"; fi
done

# 12. The second-parent route, which is what keeps the audit alive for a merge
#     commit instead of switching it off. Without it, case 11's skip would be the
#     only answer and a verdict that genuinely exists would go unread.
echo "landed_gate_state reads the pull request head a merge commit merged:"
got=$(gate_state() { case "$2" in deadbeef*) printf 'NONE' ;; *) printf 'PASS run 1 (pull_request)' ;; esac; }
      git() { case "$*" in *"rev-parse -q --verify"*) printf '%s' 0123456789012345678901234567890123456789 ;; *) command git "$@" ;; esac; }
      gate_push_triggered() { echo "no push trigger"; return 1; }
      landed_gate_state .github/workflows/release-gate.yml deadbeefdeadbeefdeadbeefdeadbeefdeadbeef
      unset -f gate_state git gate_push_triggered)
case "$got" in
  PASS*"the pull request head this commit merged"*) ok "a merge commit's second parent supplies the verdict" ;;
  *) bad "the second-parent route is gone: got '$got' — the audit is switched off for merge commits instead of narrowed" ;;
esac
# ...and it must be reached BEFORE the skip, or the skip answers first and the
# route above is dead code that still passes its own case.
if printf '%s\n' "$(sed -n '/^landed_gate_state()/,/^}/p' "$D/checks.sh")" \
   | awk '/rev-parse -q --verify/{p=NR} /gate_push_triggered/{g=NR} END{exit !(p && g && p < g)}'; then
  ok "the second-parent lookup is ordered ahead of the not-asked skip"
else
  bad "the not-asked skip now precedes the second-parent lookup, so a merge commit whose head has a real verdict is skipped instead of read"
fi

# 13. The wiring, which cases 10-12 cannot see. They test landed_gate_state in
#     isolation, and it is only worth anything if range_gate actually calls it
#     and actually treats NOTASKED as a finding. Both of those were SURVIVING
#     mutants until this case existed: the suite stayed green with range_gate
#     calling gate_state directly, and green again with the NOTASKED arm
#     unreachable. Same shape as case 9 for nomac_covers.
echo "range_gate reaches the landed-commit logic and lets NOTASKED through:"
rgbody=$(body range_gate)
if printf '%s\n' "$rgbody" | grep -q 'state=\$(landed_gate_state "\$wf" "\$sha")'; then
  ok "range_gate asks landed_gate_state, not gate_state"
else
  bad "range_gate no longer calls landed_gate_state, so a commit already on main is judged as if it were a pull request head again"
fi
if printf '%s\n' "$rgbody" | grep -q 'gate_state "\$wf" "\$sha")' && \
   ! printf '%s\n' "$rgbody" | grep -q 'landed_gate_state "\$wf" "\$sha")'; then
  bad "range_gate calls gate_state directly, which skips both the second-parent route and the not-asked rule"
else
  ok "no bare gate_state call remains in range_gate"
fi
if printf '%s\n' "$rgbody" | grep -qE 'NOTASKED\*\).*continue'; then
  ok "NOTASKED is a finding that continues, not a verdict that decides the gate"
else
  bad "range_gate has no NOTASKED arm that continues, so a gate that cannot be asked refuses the merge again — the defect this change removes"
fi

# 14. stack_class — every chain shape stack_chain can emit, and the class it has
#     to take. UNKNOWN is a real row here for the same reason it is in case 1.
echo "stack_class — every chain shape stack_chain can emit:"
while IFS='|' read -r want pr line; do
  [ -n "$want" ] || continue
  got=$(stack_class "$line" "$pr")
  if [ "$got" = "$want" ]; then ok "$want"
  else bad "expected $want, got $got, for pr=$pr chain: $line"; fi
done <<'CASES'
SELF|2739|2739 
ROOT|2733|2733 2731 
ROOT|2606|2606 2602 2597 2554 
UNRESOLVED|2678|2678 UNRESOLVED:cursor/agentd-live-instructions-and-tree-mode-6461
CYCLE|2001|2001 2002 CYCLE
DEPTH|2001|2001 2002 DEPTH
BLOCKED|2733|BLOCKED reading the base of #2733: HTTP 403
UNKNOWN|2733|
CASES

# 15. Which stack classes refuse on their own. The pass set is the small one,
#     and it is the one to state: only a pull request based on main, or one
#     whose whole chain was walked, has a reading that covers what reaches main.
echo "stack_refuses — only a walked chain or a main base may pass:"
for c in SELF ROOT UNRESOLVED CYCLE DEPTH UNKNOWN; do
  if stack_refuses "$c"; then got=refuses; else got=passes; fi
  case "$c" in SELF | ROOT) want=passes ;; *) want=refuses ;; esac
  if [ "$got" = "$want" ]; then ok "$c $got"
  else bad "$c $got, expected $want"; fi
done
# BLOCKED is deliberately outside that set: it must not be a plain refusal,
# because its caller reports it as BLOCKED and sends the reader to the failed
# read. If it ever joins the refusing set, an unreadable base starts looking
# like a stack problem.
if stack_refuses BLOCKED; then
  bad "BLOCKED refuses via stack_refuses, so an unreadable base would be reported as a stack verdict instead of as a failed read"
else
  ok "BLOCKED is not a plain refusal — the caller reports it as BLOCKED"
fi

# 16. The three wording defects this file now guards, asserted as behaviour of
#     checks() rather than as prose. Each one let a reading overstate itself.
echo "checks() must say what it did not read:"
cbody=$(body checks)

# (a) The one-argument form must REFUSE on an open pull request's head, and the
#     discriminator needs BOTH halves. `state == open` alone matches a closed
#     pull request that once contained the commit; `head.sha` alone matches a
#     sha that is merely in a branch.
dbody=$(body open_pr_headed_by)
if printf '%s\n' "$cbody" | grep -q 'open_pr_headed_by'; then
  ok "the one-argument path asks whether the sha heads an open pull request"
else
  bad "checks() no longer calls open_pr_headed_by, so the one-argument form reads the first-parent diff of a pull request head again"
fi
if printf '%s\n' "$dbody" | grep -q 'state==\\"open\\"' && printf '%s\n' "$dbody" | grep -q 'head.sha'; then
  ok "the discriminator tests both .state == open and .head.sha == sha"
else
  bad "open_pr_headed_by has lost half its discriminator — one half alone matches pull requests whose one-argument reading is fine"
fi

# (b) The 300-file cap. verdict() has guarded it since it was written; this path
#     did not, which made the operator subcommand the laxer of the two readers.
#
#     Tested by calling it, not by grepping for it. The first version of this
#     case asserted that "300" appeared in checks(), and a mutation that
#     deleted the comparison outright survived it — the digits live on in the
#     BLOCKED message. A case that cannot fail is worse than no case, because
#     it is counted.
for n in 299 300 301; do
  gen=$(seq 1 "$n" | sed 's/$/.rs/')
  if files_capped "$gen"; then got=capped; else got=readable; fi
  case "$n" in 299) want=readable ;; *) want=capped ;; esac
  if [ "$got" = "$want" ]; then ok "$n files read as $got"
  else bad "$n files read as $got, expected $want — the cap is off by one or gone"; fi
done
if printf '%s\n' "$cbody" | grep -q 'files_capped'; then
  ok "checks() guards GitHub's 300-file listing cap"
else
  bad "checks() no longer guards the 300-file cap, so a commit at the cap can read as 'no gate applies'"
fi

# (c) SATISFIED must distinguish "every gate passed" from "no gate applied".
#     The old single sentence claimed a successful run in both cases, which is
#     how a head with all of CI queued read as satisfied.
vac=$(result_line 0 0)
real=$(result_line 0 2)
case "$vac" in
  *"successful run"*) bad "the vacuous pass still claims a successful run, which is the sentence that let a head with all of CI queued read as satisfied" ;;
  *VACUOUSLY*)        ok "a reading where no gate applied says so instead of claiming a successful run" ;;
  *)                  bad "rc=0 with no gate applied does not distinguish itself from a real pass: $vac" ;;
esac
case "$real" in
  *"2 of them"*) ok "a real pass names how many gates it actually read" ;;
  *)             bad "a real pass does not say how many gates applied, so it cannot be told from a vacuous one: $real" ;;
esac
for r in "$vac" "$real"; do
  case "$r" in
    *ci.yml*) ;;
    *) bad "a rc=0 result line does not mention that ci.yml is not among the gates read: $r"; continue ;;
  esac
done
case "$vac$real" in *ci.yml*ci.yml*) ok "both rc=0 result lines say CI is not among the gates they read" ;; esac
# An unrecognised code must not become a pass. This is the same UNKNOWN row as
# case 1 and case 14, in the one place a reader actually acts on.
case "$(result_line 9 0)" in
  *SATISFIED*) bad "an unrecognised result code reads as SATISFIED" ;;
  BLOCKED*)    ok "an unrecognised result code is BLOCKED, not a pass" ;;
  *)           bad "an unrecognised result code produces something other than BLOCKED: $(result_line 9 0)" ;;
esac
for pair in "1 REFUSE" "2 BLOCKED" "3 WAIT"; do
  set -- $pair
  case "$(result_line "$1" 0)" in
    "$2"*) ok "rc=$1 reads as $2" ;;
    *)     bad "rc=$1 does not read as $2: $(result_line "$1" 0)" ;;
  esac
done
if printf '%s\n' "$cbody" | grep -q 'result_line'; then
  ok "checks() prints its result through result_line"
else
  bad "checks() builds its own result text, so the cases above test a function nothing calls"
fi
# GATES_APPLIED is what makes (c) possible, and it must be reset per call or a
# second reading inherits the first one's count.
rbody=$(sed -n '/^require_gates() {/,/^}/p' "$D/checks.sh" | nocomment)
if printf '%s\n' "$rbody" | grep -q 'GATES_APPLIED=0'; then
  ok "require_gates resets GATES_APPLIED, so one reading cannot inherit another's count"
else
  bad "GATES_APPLIED is never reset, so a vacuous reading after a real one reports the wrong count"
fi
if [ "$(printf '%s\n' "$rbody" | grep -c 'GATES_APPLIED=\$((GATES_APPLIED + 1))')" -eq 1 ]; then
  ok "GATES_APPLIED is incremented in exactly one place, on the path where a gate applies"
else
  bad "GATES_APPLIED is incremented in $(printf '%s\n' "$rbody" | grep -c 'GATES_APPLIED=\$((GATES_APPLIED + 1))') places — any second one counts a gate that did not apply"
fi

# (d) The stack reading must go THROUGH the classifier. A caller-side `case` on
#     the chain text is the drift this kit has already paid for once, in the two
#     verdict() callers that had to agree and did not.
if printf '%s\n' "$cbody" | grep -q 'stack_refuses' && printf '%s\n' "$cbody" | grep -q 'stack_class'; then
  ok "checks() classifies the chain through stack_class/stack_refuses"
else
  bad "checks() branches on the chain text itself, so the classifier and the caller can disagree about which shapes refuse"
fi

# The suite must pass with an EMPTY environment, because CI has one and a
# developer does not. This is the only case that tests the suite itself, and it
# exists because case 10 passed here and failed in CI: a shell that has run
# merge-window.sh has R exported, a fresh runner does not, and under `set -u`
# an unset R turned a stubbed read into the classifier's own answer. Every case
# above read UNKNOWN and said so as though it were a verdict about the workflow.
#
# A re-exec rather than an assertion about one variable, because the next such
# dependency will not be R. SELFTEST_CLEAN_RUN stops the recursion.
if [ "${SELFTEST_CLEAN_RUN:-}" != 1 ]; then
  echo "the suite itself does not read anything out of the operator's shell:"
  if env -i PATH="$PATH" HOME=/tmp SELFTEST_CLEAN_RUN=1 bash "$D/selftest.sh" >/dev/null 2>&1; then
    ok "it passes with an empty environment, so CI sees what you see"
  else
    bad "it FAILS with an empty environment — a case is reading something from your shell that a runner will not have. Reproduce with: env -i PATH=\$PATH HOME=/tmp bash $D/selftest.sh"
  fi
fi

echo
# 10. publish_job(): a skipped Release run and an unreadable job listing are
# different answers and must never collapse into one. The skipped run is a FACT
# — it planned nothing — and the scan has to walk past it to the run that holds
# the answer. An unreadable listing is a failed read and must still refuse.
#
# The live failure this pins: a skipped run still lists its jobs, so the guard
# that asks "is there a job?" does not catch it, and the 404 on the skipped
# job's log read as BLOCKED. Because BLOCKED returns out of a newest-first scan,
# one skipped run hid every older run behind it and shut the merge queue on
# dd0d8db10 with no expiry.
echo "publish_job — a skipped run is walked past, an unreadable listing still refuses:"
# shellcheck disable=SC2034  # consumed by the eval'd publish_job below, which builds its URL from it
R=unarbos/arbos
pj_src=$(sed -n '/^publish_job()/,/^}/p' "$D/merge-window.sh")
if [ -z "$pj_src" ]; then
  bad "no publish_job() in merge-window.sh — it was renamed or removed, and case 10 is testing nothing"
else
  eval "$pj_src"
fi
pj() {  # <what rd returns, ';' for newline> <rd exit code> -> "<rc>|<stdout>"
  local out rc
  PJ_OUT=$1 PJ_RC=$2
  # shellcheck disable=SC2317  # reached only through the eval'd publish_job
  rd() { printf '%s' "$PJ_OUT" | tr ';' '\n'; return "$PJ_RC"; }
  out=$(publish_job 99); rc=$?
  unset -f rd
  echo "$rc|$out"
}
pj_case() {  # <label> <rd output> <rd rc> <expected "rc|stdout">
  local got; got=$(pj "$2" "$3")
  if [ "$got" = "$4" ]; then ok "$1"; else bad "$1 — got '$got', expected '$4'"; fi
}
pj_case "a job that ran yields its id"                '126698636706 success' 0 '0|126698636706'
pj_case "a skipped job yields nothing, rc 0 (walk on)" '126698636706 skipped' 0 '0|'
pj_case "a run with no such job yields nothing, rc 0"  ''                     0 '0|'
pj_case "an unreadable listing is rc 1 (refuse)"       'BLOCKED: HTTP 403'    1 '1|BLOCKED: HTTP 403'
pj_case "a cancelled job is not a skipped one"         '126698636706 cancelled' 0 '0|126698636706'
# Two jobs, so that the FIRST is the one taken. With one row every fixture above
# passes just as well against `tail -1`, and the run's own job order is the only
# thing distinguishing the job that planned from a later retry of it.
pj_case "the first matching job is the one used"       '111 success;222 skipped' 0 '0|111'
# The stub makes the jq invisible, so the field it selects has to be asserted on
# the source. `.status` is `completed` on a skipped job too, so a mutant reading
# it would never match ` skipped` and would restore the defect in full.
if printf '%s\n' "$pj_src" | nocomment | grep -q '\.conclusion'; then
  ok "publish_job asks for the job's .conclusion, not its .status"
else
  bad "publish_job no longer reads .conclusion — a skipped job's .status is 'completed', so the skip test cannot fire"
fi
# The pair, asserted on its own. A mutation that returned empty on ANY failure
# would satisfy every row above that mentions `skipped` and would silently turn
# an unreadable listing into "nothing to see here" — the fail-open direction.
skipped=$(pj '126698636706 skipped' 0); unreadable=$(pj 'BLOCKED: HTTP 403' 1)
if [ "$skipped" != "$unreadable" ]; then
  ok "a skipped run and an unreadable listing give different answers ('$skipped' vs '$unreadable')"
else
  bad "a skipped run and an unreadable listing both give '$skipped' — a failed read is being reported as a fact"
fi
# And the call site must honour the empty return by CONTINUING, not by blocking.
vbody=$(body verdict)
if printf '%s\n' "$vbody" | grep -q 'j=$(publish_job "$r")' && printf '%s\n' "$vbody" | grep -q '\[ -n "$j" \] || continue'; then
  ok "verdict() calls publish_job and continues on the empty answer"
else
  bad "verdict() no longer routes through publish_job and continues on empty — the distinction above is unwired"
fi

echo
# 11. NOPLAN: the verdict for a commit no Release plan will ever name, and the
# clearance that is allowed to settle it.
#
# A plan names exactly one commit — the newest green one when it runs. A commit
# buried by newer green commits before any plan named it is therefore never
# named by a later plan either, and telling its operator that "the 15-min
# backstop judges it next" buys a wait that cannot end. dd0d8db10 sat in that
# state and shut the queue.
#
# The danger in the fix is the opposite one: NOPLAN is in verdict_clears_go's
# may-clear set, and go()'s default arm clears unconditionally, so a NOPLAN that
# lost its own arm would clear every unjudged commit on main without looking for
# a covering descendant at all. That is asserted here and not left to review.
echo "passed_over — whether a plan can still reach this commit:"
po_src=$(sed -n '/^passed_over()/,/^}/p' "$D/merge-window.sh")
if [ -z "$po_src" ]; then
  bad "no passed_over() in merge-window.sh — it was renamed or removed, and case 11 is testing nothing"
else
  eval "$po_src"
fi
cd "$repo" || exit 1
po_is() {  # <expected> <sha> <named list> <what>
  local got; got=$(passed_over "$2" "$3")
  if [ "$got" = "$1" ]; then ok "$4"; else bad "$4 — got '$got', wanted '$1'"; fi
}
po_is "$tip"   "$old" "$tip"          "a plan naming a descendant means this commit was passed over"
po_is ""       "$tip" "$old"          "a plan naming an ANCESTOR does not pass this commit over"
po_is ""       "$old" "${old:0:8}"    "a plan naming this very commit, abbreviated, is not passing it over"
po_is ""       "$old" ""              "no plan named anything, so nothing is concluded"
po_is ""       "$sib" "$tip"          "a named commit that does not contain this one yields nothing"
po_is "$sib"   "$old" "$sib"          "containment is the test, not which line the named commit sits on"
po_is ""       "$old" "deadbeef"      "a name that does not resolve yields nothing, leaving the caller at WAIT"
po_is "${tip:0:8}" "$old" "${tip:0:8}" "an abbreviated descendant still counts"
cd "$D" || exit 1
# The emission: the backstop promise must be reachable ONLY when no newer commit
# has been named. A single unconditional string is how this was wrong before.
vb=$(body verdict)
if printf '%s\n' "$vb" | grep -q 'newer=$(passed_over "$sha" "$namedlist")'; then
  ok "verdict() asks passed_over before promising anything"
else
  bad "verdict() no longer calls passed_over with the names it collected — the backstop promise is unconditional again"
fi
# passed_over can only answer from what the loop fed it, so the collection is as
# load-bearing as the call. One init, one append: without the append the list is
# always empty, every commit reads WAIT, and nothing above would notice.
nl_assigns=$(printf '%s\n' "$vb" | grep -c 'namedlist=')
if [ "$nl_assigns" -eq 2 ]; then
  ok "namedlist is initialised once and appended to once"
else
  bad "namedlist is assigned $nl_assigns time(s), expected 2 — with no append it is always empty and NOPLAN can never fire"
fi
if [ "$(printf '%s\n' "$vb" | grep -c 'backstop judges it next')" -eq 1 ] &&
   printf '%s\n' "$vb" | grep -q 'NOPLAN no Release plan has judged'; then
  ok "the backstop promise appears once, beside a NOPLAN alternative"
else
  bad "the backstop promise is not paired with a NOPLAN alternative — a commit no plan will name is being told to wait"
fi
# Both strings can be present while the NOPLAN one is unreachable. The guard
# that chooses between them is the thing under test, so it is named exactly.
if printf '%s\n' "$vb" | grep -qF 'if [ -n "$newer" ]; then'; then
  ok "the NOPLAN string is reached through the passed_over answer"
else
  bad "nothing branches on \$newer — the NOPLAN string may be present and dead, which reads as fixed and behaves as broken"
fi
# The clearance. NOPLAN must have its OWN arm in go(): falling to the default
# would clear it with no covering commit at all.
if printf '%s\n' "$gobody" | grep -qE '^\s+NOPLAN\)'; then
  ok "go() gives NOPLAN an explicit arm rather than its default"
else
  bad "go() has no NOPLAN arm, so NOPLAN reaches the default and clears with no covering commit — every unjudged commit on main would clear"
fi
noplan_arm=$(printf '%s\n' "$gobody" | sed -n '/^\s*NOPLAN)/,/;;/p')
if printf '%s\n' "$noplan_arm" | grep -q 'nomac_covers "\$sha" "\$covering" "\$tag"'; then
  ok "the NOPLAN arm clears only through nomac_covers, anchored at the tag"
else
  bad "the NOPLAN arm does not go through nomac_covers with the tag — it is clearing on something weaker"
fi
# And after(), whose default arm is the one that used to shrug.
if printf '%s\n' "$after_src" | grep -qE '^\s+NOPLAN\*\)'; then
  ok "after() handles NOPLAN explicitly"
else
  bad "after() has no NOPLAN arm; it reaches the default, which is a refusal but an unreasoned one"
fi
if printf '%s\n' "$after_src" | sed -n '/^\s*NOPLAN\*)/,/;;/p' | grep -q 'WAIT'; then
  bad "after()'s NOPLAN arm says WAIT — that is the exact instruction this verdict exists to stop giving"
else
  ok "after()'s NOPLAN arm does not tell the operator to wait"
fi

echo
# 12. Setup, policy and credential are three different answers, and the exit code is which one you got.
#
# The three defects of 2026-11-05 were each a setup gap wearing one of the other two faces, and each one had
# a message that looked right:
#
#   - No /tmp/k on the host: "REFUSED: no merge list at /tmp/k/merge-allow", which reads exactly like a
#     coordinator who has not cleared you. A lane read it that way, fell back to `gh pr merge`, and landed a
#     merge inside a closed release window. That is the failure this block exists to prevent recurring.
#   - No `op` binary, token present: every gate read, a clean GO, and then "PAT read failed" with exit 4 —
#     the code for a credential refusal — from `exec: op: not found`.
#   - opread's error file: opread's own scrub unsets MERGE_KIT before the redirect expands it, so the path
#     was /tmp/k/op-last.err for every kit however MERGE_KIT was set. With no /tmp/k the REDIRECT failed, op
#     never ran, and merge.sh reported the empty file as a 1Password refusal.
#
# These cases assert the EXIT CODE and the presence or absence of each other class's sentence, not the new
# wording. Asserting that "SETUP" appears somewhere would pass just as happily in the broken state, which is
# the point made at the top of this file.
echo "merge.sh tells a setup gap from a policy refusal:"

# The sandbox PATH carries bash and the text tools merge.sh needs to read an allow-list, and NOTHING that can
# reach the network. That absence is load-bearing twice over. It keeps these cases hermetic, and it is how
# "the op check happens up front" is pinned without a stopwatch — if the check moved back to where it used to
# be, below the GO, these runs would die before printing a setup refusal and every case here would go red.
#
# gh and git are STUBS rather than absent, which is a change from when this block was written. They used to be
# simply missing, and case (e) below used their absence as its proof that a correctly set up host gets PAST the
# preflight: the run died further down on a missing gh. Then tools.sh started checking gh and git by version,
# so their absence became a setup gap in its own right and (e) could no longer tell "passed the preflight"
# from "refused by it". The stubs answer --version and refuse everything else, so the host now looks set up,
# the run still dies at the first real gh call, and no case can reach the network.
sandbox=$(mktemp -d) || exit 1
trap 'rm -rf "$repo" "$sandbox"' EXIT
mkdir -p "$sandbox/bin" "$sandbox/fake" "$sandbox/kit" "$sandbox/kit-noallow"
for t in bash dirname sed tr grep cat cut head awk sort date mktemp; do
  p=$(command -v "$t") && ln -sf "$p" "$sandbox/bin/$t" || bad "no $t on PATH, so the sandbox cannot be built"
done
# The gh and git stubs. They report the versions tools.sh treats as its floors, so a clean sandbox produces no
# version NOTE, and they exit non-zero for every other argument so the first genuine call still fails. They sit
# in bin/ rather than fake/ so that NOOP_PATH isolates exactly one missing tool — op — and case (d) stays a
# statement about op and not about three tools at once.
printf '%s\n' '#!/bin/bash' \
  '[ "${1:-}" = --version ] && { echo "gh version 2.103.0 (2026-10-21)"; exit 0; }' \
  'echo "FAKE-GH refuses: $*" >&2; exit 1' > "$sandbox/bin/gh"
printf '%s\n' '#!/bin/bash' \
  '[ "${1:-}" = --version ] && { echo "git version 2.43.0"; exit 0; }' \
  'echo "FAKE-GIT refuses: $*" >&2; exit 1' > "$sandbox/bin/git"
chmod +x "$sandbox/bin/gh" "$sandbox/bin/git"
# `op` is simulated and never the real binary, so these cases hold on a runner that has never installed it —
# and so no case here can spend a request out of the account's shared 1,000/day budget.
# It writes a stdout sentinel and two stderr lines; block 12c reads all three.
printf '%s\n' '#!/bin/bash' \
  'printf "FAKE-OP argv:%s\n" "$*" >&2' \
  'printf "FAKE-OP exported:%s\n" "$(compgen -e | sort | tr "\n" " ")" >&2' \
  'printf "SECRET-SENTINEL-ON-STDOUT\n"' \
  'exit 1' > "$sandbox/fake/op"
chmod +x "$sandbox/fake/op"
NOOP_PATH=$sandbox/bin            # op absent
WITHOP_PATH=$sandbox/bin:$sandbox/fake   # op present
# Never the session's real token. Every run below is handed a placeholder through `env -i`, so no case can
# read the live credential out of the ambient environment even by accident, and nothing any case writes to
# disk could contain one.
FAKE_TOKEN=placeholder-not-a-real-token
SHA40=deadbeefdeadbeefdeadbeefdeadbeefdeadbeef
printf '9999 1234  # the coordinator cleared these\n' > "$sandbox/kit/merge-allow"

MRC=0; MOUT=""
mrun() {  # <path> <merge-kit> <pr> — runs merge.sh with nothing from this shell, sets MRC and MOUT
  MOUT=$(env -i PATH="$1" HOME=/tmp MERGE_KIT="$2" OP_SERVICE_ACCOUNT_TOKEN="$FAKE_TOKEN" \
           bash "$D/merge.sh" "$3" "$SHA40" 2>&1); MRC=$?
}
says()  { printf '%s\n' "$MOUT" | grep -qF "$1"; }

# (a) The kit directory is not there. Setup: nothing was read, so nothing was cleared or declined.
mrun "$WITHOP_PATH" "$sandbox/absent-kit" 9999
a_rc=$MRC; a_out=$MOUT
if [ "$a_rc" -eq 5 ]; then ok "a missing kit directory exits 5 (setup)"
else bad "a missing kit directory exits $a_rc, not 5 — it is being reported as one of the other classes: $MOUT"; fi
if says "$sandbox/absent-kit does not exist"; then ok "the refusal names the directory that is missing"
else bad "the refusal does not name the missing directory, so the reader cannot tell what to create: $MOUT"; fi
for sentence in "not in the coordinator's merge list" "PAT read failed"; do
  if says "$sentence"; then bad "a missing kit directory says '$sentence' — that is another class's answer"
  else ok "a missing kit directory does not say '$sentence'"; fi
done

# (b) The directory is there and the allow-list is not. Still setup, and a different thing to fix.
mrun "$WITHOP_PATH" "$sandbox/kit-noallow" 9999
if [ "$MRC" -eq 5 ]; then ok "a missing allow-list file exits 5 (setup)"
else bad "a missing allow-list file exits $MRC, not 5: $MOUT"; fi
if says "$sandbox/kit-noallow/merge-allow"; then ok "the refusal names the file to write"
else bad "the refusal does not name $sandbox/kit-noallow/merge-allow: $MOUT"; fi
if [ "$MOUT" != "$a_out" ]; then ok "a missing directory and a missing file are not the same message"
else bad "a missing directory and a missing allow-list print the same thing, so one fix is attempted for the other"; fi

# (c) The allow-list is present and does not name this pull request. POLICY, and it must not be code 5.
#     This pair is the defect: (a) and (c) used to be the same sentence and the same code.
mrun "$WITHOP_PATH" "$sandbox/kit" 8888
c_rc=$MRC
if [ "$c_rc" -eq 2 ]; then ok "an allow-list that does not name the PR exits 2 (policy)"
else bad "an allow-list that does not name the PR exits $c_rc, not 2: $MOUT"; fi
if says "#8888 is not in the coordinator's merge list"; then ok "the refusal says the list does not name this PR"
else bad "the policy refusal does not say the list fails to name #8888: $MOUT"; fi
if says "1234"; then ok "the refusal shows the list it did read"
else bad "the refusal does not show the allow-list it read, so it cannot be told from a list it never read: $MOUT"; fi
if [ "$a_rc" -ne "$c_rc" ]; then
  ok "THE DEFECT: a missing kit ($a_rc) and an uncleared PR ($c_rc) are different exit codes"
else
  bad "a missing kit directory and an uncleared pull request both exit $a_rc. These are setup and policy, they call for opposite actions — create a directory, or go and ask the coordinator — and a lane that cannot tell them apart routes around this script. That is exactly what happened on 2026-11-05."
fi

# (d) op is not installed, everything else is ready. Setup, reported BEFORE any gate runs.
#     Not exit 4: nothing has been read, denied or rate-limited, and the service account is fine.
mrun "$NOOP_PATH" "$sandbox/kit" 9999
if [ "$MRC" -eq 5 ]; then ok "a missing op binary exits 5 (setup), not 4 (credential)"
else bad "a missing op binary exits $MRC — 4 would be a credential refusal and 3 a verdict, and neither happened: $MOUT"; fi
if says "op) is not on PATH"; then ok "the refusal says which binary is missing"
else bad "the refusal does not name op as the missing binary: $MOUT"; fi
if says "op_linux_amd64"; then ok "the refusal says how to install it"
else bad "the refusal gives no install command, so the reader has to go and find one: $MOUT"; fi
for sentence in "PAT read failed" "VERDICT" "NO-GO"; do
  if says "$sentence"; then bad "a missing op binary printed '$sentence' — a gate or the vault was reached before the binary was checked for"
  else ok "a missing op binary reaches no '$sentence'"; fi
done

# (e) The other direction, which this preflight could create: a host that IS set up must get past it. With
#     no gh on the sandbox PATH the run then fails further down, and that is the proof — it got further.
mrun "$WITHOP_PATH" "$sandbox/kit" 9999
if [ "$MRC" -ne 5 ] && ! says "SETUP:"; then ok "a kit with op, a token and a clearance passes the preflight"
else bad "a correctly set up host still reads as a setup gap (rc=$MRC), so the preflight refuses everything: $MOUT"; fi

echo
echo "tools.sh checks the kit's own tools by version, and tells three causes apart:"
# On 2026-11-06 four merges ran with op resolving to /tmp/opbin/op at 2.30.0, from a directory an earlier shell
# had put on PATH by hand. The next shell did not inherit it, so `command -v op` found nothing and the kit said
# MISSING TOOL. That was true of the host it could see and useless as a diagnosis: the binary had not been
# deleted and the host had not been reimaged, PATH had changed under a running session. A presence check cannot
# separate those three, and they call for three different actions — reinstall, investigate, or fix PATH. These
# cases pin the separation, and each one is driven against a crafted baseline rather than a real outage.
tkit=$sandbox/tools-kit; mkdir -p "$tkit"
trun() {  # <path> <merge-kit> — runs `tools.sh check` with nothing from this shell; sets TRC and TOUT
  TOUT=$(env -i PATH="$1" HOME=/tmp MERGE_KIT="$2" bash "$D/tools.sh" check 2>&1); TRC=$?
}
tsays() { printf '%s\n' "$TOUT" | grep -qF "$1"; }
now_boot=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null)

# (f) gh missing is a setup gap of its own. merge.sh cannot read a check rollup without it, so this must not
#     be discovered halfway down a run that has already started judging things.
mkdir -p "$sandbox/nogh"; for t in bash sed tr grep cat cut head awk sort date mktemp dirname; do
  p=$(command -v "$t") && ln -sf "$p" "$sandbox/nogh/$t"
done
ln -sf "$sandbox/bin/git" "$sandbox/nogh/git"; ln -sf "$sandbox/fake/op" "$sandbox/nogh/op"
mrun "$sandbox/nogh" "$sandbox/kit" 9999
if [ "$MRC" -eq 5 ] && says "gh is not on PATH"; then ok "a missing gh exits 5 (setup) and names gh"
else bad "a missing gh exits $MRC and does not name gh, so it is not reported as the setup gap it is: $MOUT"; fi
for sentence in "VERDICT" "NO-GO" "PAT read failed"; do
  if says "$sentence"; then bad "a missing gh printed '$sentence' — something was judged before the tools were checked for"
  else ok "a missing gh reaches no '$sentence'"; fi
done

# (g) A clean check records what it saw. That record is the only thing the NEXT failure can compare against,
#     which is the whole reason the three causes below can be told apart at all.
rm -f "$tkit/tools-baseline"
trun "$WITHOP_PATH" "$tkit"
if [ "$TRC" -eq 0 ]; then ok "a host with all three tools passes the check"
else bad "a host with all three tools fails the check (rc=$TRC): $TOUT"; fi
if grep -q '^op_path=' "$tkit/tools-baseline" 2>/dev/null && grep -q '^boot_id=' "$tkit/tools-baseline" 2>/dev/null
then ok "a clean check writes the resolved path and the boot id to the baseline"
else bad "a clean check left no usable baseline, so the next failure has nothing to compare against"; fi

# (h) A FAILING check must not overwrite a good baseline. If it did, the first failure would destroy the
#     reading that explains it, and every cause below would collapse into "no baseline on this host".
good=$(cat "$tkit/tools-baseline")
trun "$sandbox/nogh" "$tkit"
if [ "$(cat "$tkit/tools-baseline")" = "$good" ]; then ok "a failing check leaves the last good baseline intact"
else bad "a failing check overwrote the baseline, destroying the evidence that explains the failure"; fi

# (i) PATH DRIFT: the recorded binary is still on disk and executable, and PATH no longer reaches it. This is
#     the one that actually happened, and the one a presence check reports as a deletion.
printf 'recorded=2026-11-06T00:00:00Z\nop_path=%s\nop_version=2.30.0\nboot_id=%s\n' "$sandbox/fake/op" "$now_boot" > "$tkit/tools-baseline"
trun "$NOOP_PATH" "$tkit"
if tsays "PATH DRIFT"; then ok "a binary still on disk but off PATH is diagnosed as PATH drift"
else bad "a binary still on disk but off PATH is not diagnosed as PATH drift: $TOUT"; fi
if tsays "DELETED" || tsays "REIMAGE"; then bad "PATH drift was also reported as a deletion or a reimage, so the three are not separated: $TOUT"
else ok "PATH drift is not also reported as a deletion or a reimage"; fi

# (j) DELETED: same running host, recorded path gone from it. Worth knowing WHAT removed it before reinstalling.
printf 'recorded=2026-11-06T00:00:00Z\nop_path=%s\nop_version=2.30.0\nboot_id=%s\n' "$sandbox/absent/op" "$now_boot" > "$tkit/tools-baseline"
trun "$NOOP_PATH" "$tkit"
if tsays "DELETED"; then ok "a vanished binary on an unrestarted host is diagnosed as a deletion"
else bad "a vanished binary on an unrestarted host is not diagnosed as a deletion: $TOUT"; fi

# (k) REIMAGE OR REBOOT: the boot id moved, so this is not the host that recorded the baseline and nobody
#     deleted anything. A fresh install is the whole answer, and no investigation is owed.
printf 'recorded=2026-11-06T00:00:00Z\nop_path=%s\nop_version=2.30.0\nboot_id=%s\n' "$sandbox/absent/op" 00000000-0000-0000-0000-000000000000 > "$tkit/tools-baseline"
trun "$NOOP_PATH" "$tkit"
if tsays "REIMAGE OR REBOOT"; then ok "a changed boot id is diagnosed as a reimage or reboot"
else bad "a changed boot id is not diagnosed as a reimage or reboot: $TOUT"; fi
if tsays "DELETED"; then bad "a reimage was also reported as a deletion, which sends the reader looking for a culprit that does not exist: $TOUT"
else ok "a reimage is not also reported as a deletion"; fi

# (l) Version drift is a NOTE and NOT a refusal. A floor here is a version the kit has been run with, not a
#     measured minimum, so below one is UNKNOWN rather than proven broken. Refusing on it would stop a merge
#     over something nobody has shown to be a problem; saying nothing would lose it when a merge behaves oddly.
mkdir -p "$sandbox/olddrift"; for t in bash sed tr grep cat cut head awk sort date mktemp dirname; do
  p=$(command -v "$t") && ln -sf "$p" "$sandbox/olddrift/$t"
done
ln -sf "$sandbox/fake/op" "$sandbox/olddrift/op"; ln -sf "$sandbox/bin/git" "$sandbox/olddrift/git"
printf '%s\n' '#!/bin/bash' '[ "${1:-}" = --version ] && { echo "gh version 2.9.0 (2024-01-01)"; exit 0; }' \
  'exit 1' > "$sandbox/olddrift/gh"; chmod +x "$sandbox/olddrift/gh"
rm -f "$tkit/tools-baseline"
trun "$sandbox/olddrift" "$tkit"
if [ "$TRC" -eq 0 ]; then ok "a tool below its floor still passes the check — a floor is exercised, not measured"
else bad "a tool below its floor refuses the run (rc=$TRC), stopping a merge over something unproven: $TOUT"; fi
if tsays "below the"; then ok "the drift is named rather than passed over in silence"
else bad "a tool below its floor produced no note at all, so the drift is invisible when a merge misbehaves: $TOUT"; fi
# sort -V and not a string compare: 2.9.0 is BELOW 2.103.0, and `[[ 2.9.0 < 2.103.0 ]]` says the opposite.
if tsays "2.9.0"; then ok "2.9.0 reads as below 2.103.0, so the comparison is numeric and not lexical"
else bad "2.9.0 was not reported below 2.103.0 — the version compare is lexical, which inverts every two-digit minor: $TOUT"; fi

echo
echo "opread writes op's stderr to the CONFIGURED kit, and runs op either way:"
# 12c. The redirect, driven through the real opread against the fake op. A unit test of op_err_file alone
# would pass against the broken version: the defect was never in the path, it was that opread's scrub
# unsets MERGE_KIT before the subshell expands it, so only calling opread can see it.
oprun() {  # <merge-kit> — calls opread with a canary exported; sets OSTDOUT, ORC, OERR
  OERR=$(MERGE_KIT=$1 op_err_file)
  rm -f "$OERR"
  OSTDOUT=$(PATH=$WITHOP_PATH MERGE_KIT=$1 OP_SERVICE_ACCOUNT_TOKEN=$FAKE_TOKEN \
            SELFTEST_CANARY=exported-by-the-caller bash -c "source '$D/opread.sh'; opread 'op://fixture/ref'" 2>/dev/null)
  ORC=$?
}
oprun "$sandbox/kit"
if [ "$OERR" = "$sandbox/kit/op-last.err" ]; then ok "op_err_file resolves to the configured kit"
else bad "op_err_file resolved to '$OERR', not $sandbox/kit/op-last.err"; fi
if grep -qF 'FAKE-OP argv:' "$OERR" 2>/dev/null; then
  ok "op's stderr landed in the configured kit, so MERGE_KIT survived the scrub"
else
  bad "nothing from op reached $OERR. Either the redirect went to /tmp/k again — MERGE_KIT is unset by opread's own scrub before the redirect expands it, so it must be resolved in the caller — or op never ran at all."
fi
# The secret goes to stdout and must stay there. The error file is reported verbatim to the operator and is
# written with no special permissions, so a secret reaching it would be a secret published twice over.
if [ "$OSTDOUT" = "SECRET-SENTINEL-ON-STDOUT" ]; then ok "the caller captures op's stdout"
else bad "opread did not hand op's stdout back to the caller (got '$OSTDOUT'), so no secret could be read"; fi
# op's own exit status, which is what merge.sh's `&&` tests. The fake refuses, so a zero here would mean a
# failed read reaching the merge call with an empty token.
if [ "$ORC" -ne 0 ]; then ok "opread returns op's non-zero status when op refuses"
else bad "opread returned 0 for an op that exited 1, so a failed read would read as a successful one"; fi
if grep -qF 'SECRET-SENTINEL-ON-STDOUT' "$OERR" 2>/dev/null; then
  bad "op's STDOUT reached the error file. That file is printed verbatim in merge.sh's refusal, so the redirect must take stderr only."
else
  ok "op's stdout is not in the error file — the redirect takes stderr only"
fi
if grep -qF "$FAKE_TOKEN" "$OERR" 2>/dev/null; then
  bad "the token value is in the error file"
else
  ok "the token value is not in the error file"
fi
if grep -qF 'argv:--cache=false read op://fixture/ref' "$OERR" 2>/dev/null; then
  ok "op's argv is the flag and the reference, and carries no secret"
else
  bad "op's argv is not what this kit promises: $(head -2 "$OERR" 2>/dev/null)"
fi
# The scrub itself. The canary proves it still removes things; _operr proves the fix for the redirect did not
# become the next thing it leaks. PWD and SHLVL are set by the fake op's own bash and carry nothing.
exported=$(sed -n 's/^FAKE-OP exported://p' "$OERR" 2>/dev/null)
if [ -z "$exported" ]; then
  bad "the fake op reported no exported variables, so the scrub is unasserted"
else
  stray=""
  for v in $exported; do
    case $v in PATH | HOME | OP_SERVICE_ACCOUNT_TOKEN | DBUS_SESSION_BUS_ADDRESS | OP_CACHE | PWD | SHLVL) ;;
      *) stray="$stray $v" ;;
    esac
  done
  if [ -z "$stray" ]; then ok "op sees only PATH, HOME, the token and the two the subshell sets"
  else bad "op's environment also carries:$stray — the scrub is leaking, and SELFTEST_CANARY proves it is reaching variables the caller set"; fi
fi

# With no kit directory at all, op must still RUN. This is the live failure of 2026-11-05 in full: the
# redirect was the thing that failed, so op never started, and its silence was reported as a refusal.
oprun "$sandbox/absent-kit"
case "$OERR" in
  "$sandbox/absent-kit/op-last.err") bad "op_err_file still points inside a directory that does not exist, so the redirect is what fails and op never runs" ;;
  *) [ -d "$(dirname "$OERR")" ] && ok "with no kit directory, the error file falls back to one that exists ($OERR)" \
       || bad "the fallback error file $OERR is in a directory that does not exist either" ;;
esac
if [ "$OSTDOUT" = "SECRET-SENTINEL-ON-STDOUT" ]; then
  ok "with no kit directory, op still ran and its output still reached the caller"
else
  bad "with no kit directory, opread returned '$OSTDOUT' — op did not run. The redirect failed before it started, and merge.sh reports that as a PAT read failure, which is a setup gap wearing a credential refusal's face."
fi

echo
echo "merge.sh re-reads main rather than trusting mergeable:"
# 12d. MERGEABLE is a statement about text, not about tests, so it cannot settle whether a verdict is stale.
# The kit's defence is three live reads of main's head around the GO, compared against each other. The reads
# themselves need the network; what can be asserted here is that they are made, that each one refuses, and
# that nothing in this file has started inferring staleness from the rollup's mergeable fields instead.
mcode=$(nocomment <"$D/merge.sh")
if printf '%s\n' "$mcode" | grep -qE 'mergeable|mergeStateStatus'; then
  bad "merge.sh reads mergeable/mergeStateStatus. main has no branch protection, so that field reports on text and not on tests, and a MERGEABLE pull request 20 commits behind reads identically to a current one."
else
  ok "merge.sh never consults mergeable or mergeStateStatus"
fi
reads=$(printf '%s\n' "$mcode" | grep -c '=\$(main_head)')
if [ "$reads" -eq 3 ]; then
  ok "main's head is read three times: before the GO, after it, and just before the merge call"
else
  bad "main's head is read $reads time(s), expected 3 — each read closes part of the window in which main can move under a GO, and the one before the merge call is the narrowest this tool can give"
fi
if [ "$(printf '%s\n' "$mcode" | grep -c 'main_refuse "\$main0"')" -eq 2 ]; then
  ok "both post-GO reads refuse through main_refuse, anchored at the main the GO judged"
else
  bad "the comparisons no longer both go through main_refuse \"\$main0\" — a read that is taken and not compared to the main GO judged is a read that proves nothing"
fi
# And the refusal itself, executed. A refusal that does not name the commits sends the operator to the PR to
# guess, which is what a stale verdict already looks like from there.
mr_src=$(sed -n '/^main_refuse() {/,/^}/p' "$D/merge.sh")
if [ -z "$mr_src" ]; then
  bad "no main_refuse() in merge.sh — it was renamed or removed and the cases above test nothing"
else
  eval "$mr_src"
  main_moved() { echo "    STUBBED commit list for $1..$2"; }
  mr_out=$(main_refuse 1111111111111111111111111111111111111111 2222222222222222222222222222222222222222 "during the GO check")
  unset -f main_moved
  for want in 111111111 222222222 "STUBBED commit list" "again for a fresh GO"; do
    case "$mr_out" in
      *"$want"*) ok "the stale-main refusal carries '$want'" ;;
      *) bad "the stale-main refusal does not carry '$want': $mr_out" ;;
    esac
  done
fi

echo
if [ "$fail" -eq 0 ]; then echo "selftest: PASS"; else echo "selftest: FAIL"; fi
exit "$fail"

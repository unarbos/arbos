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

# 10. A gate workflow that declares no path filter at all. This is release-gate.yml
#     as of this change, and it is a third answer rather than a shade of an
#     existing one: "runs on everything" is a statement, "I could not read the
#     filter" is an admission, and both used to arrive as zero patterns.
#
#     The case that matters is the FOURTH one. Three of these would pass against
#     an implementation that simply said "empty output means unfiltered", and
#     that implementation silently converts every reader bug into a required
#     gate the kit believes it has evaluated. The discriminating input is a
#     workflow that declares `paths:` and whose entries this reader cannot see:
#     it must still block. The test is the presence of the key, not the
#     emptiness of the output, and only the fourth assertion can tell.
echo "gate_patterns and gate_applies — unfiltered is not the same as unreadable:"
WF_UNFILTERED=$'name: g\non:\n  pull_request:\njobs:\n  a:\n    runs-on: ubuntu-24.04\n'
WF_FILTERED=$'name: g\non:\n  pull_request:\n    paths:\n      - "apps/ios/**"\njobs:\n  a: {}\n'
# Declares a filter, in a flow sequence this reader does not parse. Zero patterns,
# but the `paths:` key is right there, so the honest answer is "unread".
WF_INLINE=$'name: g\non:\n  pull_request:\n    paths: ["apps/ios/**"]\njobs:\n  a: {}\n'
# Keyed on the ref as well as the name, because gate_applies reads the head's copy
# and the base's copy and the interesting cases are the ones where they differ —
# which is every pull request that changes a gate's trigger, including this one.
# `unfiltered` is a prefix of nothing here, but order the arms longest-first anyway:
# a `per_page=1` / `per_page=100` collision cost a case an afternoon once already.
rd() {
  case "$1" in
    *unfiltered.yml*)                printf '%s' "$WF_UNFILTERED" | base64 -w0 ;;
    *inline.yml*)                    printf '%s' "$WF_INLINE" | base64 -w0 ;;
    *filtered.yml*)                  printf '%s' "$WF_FILTERED" | base64 -w0 ;;
    # This pull request's own shape: the filter is gone at the head, still there on main.
    *dropping.yml*ref=main*)         printf '%s' "$WF_FILTERED" | base64 -w0 ;;
    *dropping.yml*)                  printf '%s' "$WF_UNFILTERED" | base64 -w0 ;;
    # Unfiltered at the head, and main's copy declares a filter this reader cannot see.
    *halfread.yml*ref=main*)         printf '%s' "$WF_INLINE" | base64 -w0 ;;
    *halfread.yml*)                  printf '%s' "$WF_UNFILTERED" | base64 -w0 ;;
    *) echo "HTTP 404 Not Found" >&2; return 1 ;;
  esac
}
GATE_BASE=main
# gate_patterns interpolates $R into the URL before it calls rd, and this file runs
# under `set -u`. Without this the function dies on the unbound variable and returns
# 1, which the fourth assertion below reads as a BLOCK and calls a pass. It did, once.
R=unarbos/arbos

out=$(gate_patterns unfiltered.yml main); rc=$?
if [ $rc -eq 3 ] && [ -z "$out" ]; then
  ok "a workflow with no paths: key reports status 3, distinct from both 0 and 2"
else
  bad "an unfiltered workflow reported status $rc with '$out' — if that is 0 the caller blocks every pull request, if 2 it reads as a gate that does not exist"
fi

out=$(gate_patterns filtered.yml main); rc=$?
if [ $rc -eq 0 ] && [ "$out" = "apps/ios/**" ]; then
  ok "a workflow that does declare paths: still reports them with status 0"
else
  bad "a filtered workflow reported status $rc with '$out', so the new status has swallowed the ordinary case"
fi

msg=$(gate_applies unfiltered.yml deadbeef "docs/README.md"); rc=$?
if [ $rc -eq 0 ]; then
  ok "an unfiltered gate applies to a diff touching none of anything"
else
  bad "an unfiltered gate returned $rc ($msg) for an unrelated diff — it runs for every pull request, so it applies to every pull request"
fi

msg=$(gate_applies inline.yml deadbeef "apps/ios/Arbos/x.swift"); rc=$?
if [ $rc -eq 2 ]; then
  ok "a paths: key this reader cannot parse still BLOCKS rather than passing as unfiltered"
else
  bad "a declared-but-unparsed filter returned $rc ($msg) — 'unfiltered' is now standing in for 'unread', which is the exact substitution this status was added to prevent"
fi

msg=$(gate_applies filtered.yml deadbeef "docs/README.md"); rc=$?
if [ $rc -eq 1 ]; then
  ok "a filtered gate is still not required for a diff off its paths"
else
  bad "a filtered gate returned $rc ($msg) off its own paths, so the filter is no longer being consulted at all"
fi

msg=$(gate_applies dropping.yml deadbeef "docs/README.md"); rc=$?
if [ $rc -eq 0 ]; then
  ok "a head that drops the filter while main still has one applies to everything"
else
  bad "the head dropping its filter returned $rc ($msg) — this is the shape of the pull request making that change, so it would refuse itself"
fi

# The two halves disagree and neither is a clean read: unfiltered at the head, a
# declared-but-unparsed filter on main. The union errs towards asking for a verdict,
# and the head's answer is the decisive one because the head's copy is what governs
# the run. This is the ONLY input that separates testing rc 3 before the empty-list
# check from testing it after: the empty-list check is guarded by rc 0, so for every
# other input the two orders agree. A mutant that moved the check survived this
# suite until this case existed.
msg=$(gate_applies halfread.yml deadbeef "docs/README.md"); rc=$?
if [ $rc -eq 0 ]; then
  ok "unfiltered at the head decides it, even when main's own filter cannot be read"
else
  bad "returned $rc ($msg) — an unreadable filter on main is overriding an unambiguous 'runs on everything' at the head"
fi

unset -f rd
unset GATE_BASE R

echo
if [ "$fail" -eq 0 ]; then echo "selftest: PASS"; else echo "selftest: FAIL"; fi
exit "$fail"

#!/bin/bash
# rebase-preserved-selftest.sh — rebase-preserved.sh against real rebases in a scratch repository.
# Run it directly: scripts/merge/rebase-preserved-selftest.sh
#
# WHY THIS IS NOT A BLOCK IN selftest.sh. That file's contract is its first line: "No network, no
# git, no credentials." Gate 3 is a statement about what a rebase did, and there is no way to test
# it without performing rebases. Weakening the other file's contract to fit this in would cost more
# than a second harness does.
#
# WHAT MAKES THIS A TEST RATHER THAN A TALLY, which is the lesson this gate was born from. Three of
# the cases are NEGATIVE CONTROLS, each asserting that a WRONG implementation would have been
# caught here: that a raw-byte comparison REFUSES a correct rebase, and that a `--name-only`
# comparison PASSES an altered one. Without them a suite of greens says only that nothing threw,
# which is also what an absent gate says.
#
# The artefact guard is the other load-bearing case. During the audit a silently failed cherry-pick
# produced an empty patch, and two empty patches compare equal, so it would have read as PRESERVED.
#
# Three bugs in this harness were themselves found by running it, and all three are now cases:
#   - `git cherry-pick -q` is not a flag, so the replay never happened and the gate correctly
#     reported SETUP rather than inventing a verdict about a rebase that did not occur;
#   - MERGE_KIT was placed inside the scratch repository, so `git add -A` committed the declaration
#     into the resolution it described. That is now a guard in the gate and the last case here;
#   - a "keep both sections" resolution that dropped one blank line was caught as an altered line,
#     on the real #2873 × #2877 conflict. Exactly the trivial-looking resolution the mode exists for.
set -u
D=$(cd "$(dirname "$0")" && pwd)
G="$D/rebase-preserved.sh"
[ -x "$G" ] || { echo "SETUP: $G is not executable"; exit 5; }

fail=0
ok()  { printf '  ok    %s\n' "$1"; }
bad() { printf '  FAIL  %s\n' "$1"; fail=1; }
# run <expected-rc> <label> -- <cmd…>
run() { local want=$1 label=$2; shift 3; local out rc
  out=$("$@" 2>&1); rc=$?
  if [ "$rc" -eq "$want" ]; then ok "$label"
  else bad "$label (wanted rc=$want, got rc=$rc) :: $(printf '%s' "$out" | head -2 | tr '\n' ' ')"; fi; }

T=$(mktemp -d) || exit 5
trap 'rm -rf "$T"' EXIT
# The kit sits OUTSIDE the scratch repository. The first version of this harness put it inside,
# `git add -A` committed the declaration into the resolution it was describing, and the gate
# refused its own bookkeeping as an undeclared file. That bug is now a guard in the gate; the last
# case below asserts the guard fires.
export MERGE_KIT="$T/kit"; mkdir -p "$MERGE_KIT"
mkdir -p "$T/repo"; cd "$T/repo" || exit 5
git init -q . && git config user.email t@t && git config user.name t && git config commit.gpgsign false

seed() { printf '%s\n' "$@" > F; }
cm() { git add -A && git commit -q -m "$1"; }

# A file with a shared anchor, which is the shape every real conflict in this queue has.
seed "# rules" "" "## alpha" "a1" "" "## anchor" "z1"
cm base0
BASE0=$(git rev-parse HEAD)

# --- clean rebase over an unrelated base change --------------------------------------------------
git checkout -q -b pr1 "$BASE0"
seed "# rules" "" "## alpha" "a1" "" "## mine" "m1" "" "## anchor" "z1"; cm pr1
PR1=$(git rev-parse HEAD)
git checkout -q -b newbase1 "$BASE0"; echo other > OTHER; cm unrelated
NB1=$(git rev-parse HEAD)
git cherry-pick "$PR1" >/dev/null 2>&1 || { echo "SETUP: cherry-pick 1 failed"; exit 5; }
NH1=$(git rev-parse HEAD)
run 0 "clean rebase over an unrelated base change is PRESERVED" -- "$G" clean "$BASE0" "$PR1" "$NB1" "$NH1"

# --- clean rebase over a base that touched the SAME file, plus the raw-byte negative control ------
git checkout -q -b newbase2 "$BASE0"
seed "# rules" "" "## zero" "n0" "" "## alpha" "a1" "" "## anchor" "z1"; cm "base touches the same file"
NB2=$(git rev-parse HEAD)
git cherry-pick "$PR1" >/dev/null 2>&1 || { echo "SETUP: cherry-pick 2 failed"; exit 5; }
NH2=$(git rev-parse HEAD)
run 0 "same-file base change is PRESERVED by patch-id" -- "$G" clean "$BASE0" "$PR1" "$NB2" "$NH2"
if git diff "$BASE0" "$PR1" | cmp -s - <(git diff "$NB2" "$NH2"); then
  bad "negative control: raw bytes MATCHED, so this case cannot show patch-id doing any work"
else
  ok "negative control: a raw-byte gate 3 REFUSES this correct rebase (patch-id does not)"
fi

# --- a rebase that genuinely changed content ------------------------------------------------------
git checkout -q -b newbase3 "$NB1"
seed "# rules" "" "## alpha" "a1" "" "## mine" "m1-ALTERED" "" "## anchor" "z1"; cm "replayed but altered"
NH3=$(git rev-parse HEAD)
run 2 "a rebase that altered a line is NOT PRESERVED" -- "$G" clean "$BASE0" "$PR1" "$NB1" "$NH3"

# --- the artefact guard: an empty diff is never a pass --------------------------------------------
run 5 "an empty post-rebase diff is SETUP, not a pass" -- "$G" clean "$BASE0" "$PR1" "$NB1" "$NB1"
run 5 "an empty pre-rebase diff is SETUP, not a pass"  -- "$G" clean "$BASE0" "$BASE0" "$NB1" "$NH1"
run 5 "an unresolvable revision is SETUP, not a pass"  -- "$G" clean "$BASE0" "$PR1" "$NB1" deadbeefdeadbeef

# --- negative control for the --name-only hazard --------------------------------------------------
if [ "$(git diff --name-only "$BASE0" "$PR1")" = "$(git diff --name-only "$NB1" "$NH3")" ]; then
  ok "negative control: --name-only PASSES the altered rebase the case above refuses"
else
  bad "negative control: --name-only differed, so the hazard comment overstates the risk"
fi

# --- resolved mode ---------------------------------------------------------------------------------
run 5 "resolved without a declaration is SETUP" -- "$G" resolved "$BASE0" "$PR1" "$NB2" "$NH2"

# A conflicted rebase: the new base inserted its own section at the same anchor, and the resolution
# keeps both in an order, altering no line of either.
git checkout -q -b newbase4 "$BASE0"
seed "# rules" "" "## alpha" "a1" "" "## theirs" "t1" "" "## anchor" "z1"; cm "their section at the anchor"
NB4=$(git rev-parse HEAD)
"$G" declare "$NB4" "$PR1" F -- "sections 'mine' and 'theirs' both survive, in some order, no line of either altered" >/dev/null \
  || { echo "SETUP: declare failed"; exit 5; }
sleep 1
seed "# rules" "" "## alpha" "a1" "" "## theirs" "t1" "" "## mine" "m1" "" "## anchor" "z1"; cm "resolution: keep both"
NH4=$(git rev-parse HEAD)
run 0 "a declared resolution keeping both sections is PRESERVED" -- "$G" resolved "$BASE0" "$PR1" "$NB4" "$NH4"

git checkout -q -b newbase5 "$NB4"
seed "# rules" "" "## alpha" "a1" "" "## theirs" "t1" "" "## mine" "m1-ALTERED" "" "## anchor" "z1"; cm "resolution altered a line"
NH5=$(git rev-parse HEAD)
run 2 "a resolution that altered this side's line is NOT PRESERVED" -- "$G" resolved "$BASE0" "$PR1" "$NB4" "$NH5"

git checkout -q -b newbase6 "$NB4"
seed "# rules" "" "## alpha" "a1" "" "## theirs" "t1" "" "## mine" "m1" "" "## anchor" "z1"
echo smuggled > SRC.rs; cm "resolution touches an undeclared file"
NH6=$(git rev-parse HEAD)
run 2 "a resolution touching an undeclared file is NOT PRESERVED" -- "$G" resolved "$BASE0" "$PR1" "$NB4" "$NH6"

# A declaration written after the outcome is a description of it, not a prediction about it.
sleep 1; "$G" declare "$NB4" "$PR1" F -- "written after the fact" >/dev/null
run 2 "a declaration newer than the resolution is NOT PRESERVED" -- "$G" resolved "$BASE0" "$PR1" "$NB4" "$NH4"

# --- the guard this harness's own second bug produced ----------------------------------------------
run 5 "a kit inside the worktree is SETUP, not a gate answer" -- env MERGE_KIT="$T/repo/kit-inside" \
  sh -c 'mkdir -p "$MERGE_KIT"; exec "$0" clean "$1" "$2" "$3" "$4"' "$G" "$BASE0" "$PR1" "$NB1" "$NH1"

echo
if [ "$fail" -eq 0 ]; then echo "rebase-preserved selftest: PASS"; else echo "rebase-preserved selftest: FAIL"; fi
exit "$fail"

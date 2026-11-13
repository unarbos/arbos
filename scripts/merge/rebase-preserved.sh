#!/bin/bash
# rebase-preserved.sh — gate 3: did the rebase preserve the patch? Run it, do not source it.
#
#   rebase-preserved.sh clean    <old-base> <old-head> <new-base> <new-head>
#   rebase-preserved.sh declare  <new-base> <old-head> <path>[,<path>…] -- <sentence>
#   rebase-preserved.sh resolved <old-base> <old-head> <new-base> <new-head>
#
# WHY THIS FILE EXISTS, AND IT IS NOT THAT THE OLD CHECK WAS WEAK.
#
# Until 2026-11-07 gate 3 had no implementation. Not a check with a gap — an absence with a verdict
# attached. Twenty-five merge tokens recorded "gate 3 passed" while nothing in this kit compared
# content: the only two `git diff` calls in the whole kit are `--name-only`, in loop-public.sh and
# merge-window.sh, and both compute file lists for gate applicability. The audit that found it read
# all seven scripts for the mechanism instead of trusting the label.
#
# checks.sh already catalogues six instances of "a check that failed to run reports a verdict about
# its subject". This is the seventh and the purest: the check had not failed to run, it had never
# been written, and it still reported.
#
# The 25 passes were vacuous in the strongest sense rather than wrong. Gate 3 asserts that a REBASE
# preserved a patch; no token had been rebased, so the gate was asserting about an operation that
# had not occurred. There is no retroactive hole here. Gate 3 becomes load-bearing at the first
# rebase, which is why this exists before one happens and not after.
#
# THE HAZARD THIS FILE IS MOST LIKELY TO BE REPLACED BY, stated here because the kit itself models
# it twice and a lane reaching for the familiar local idiom will land on it:
#
#     git diff --name-only <base> <head>     # <-- NEVER gate 3
#
# A file-list comparison is stable across a rebase AND blind to content, so it passes confidently in
# exactly the case gate 3 exists to catch: a rebase that silently changed what a line says. The
# other tempting form fails the opposite way. `cmp` on raw `git diff` output refuses correct
# rebases, because a base that touched the same file changes that file's pre-image blob and so its
# `index` line; measured over this queue's 25 tokens in the planned merge order, a raw-byte
# comparison spuriously refuses 15 of 26 members, first at position 5. A gate that cries wolf on a
# majority of its subjects is switched off by the third false refusal, and then gate 3 is a
# judgement with a label on it again — which is the state this file was written to end.
#
#     form                       survives a rebase    catches changed content
#     cmp on raw `git diff`      no (15 of 26)        yes
#     git diff --name-only       yes                  NO
#     git patch-id --stable      yes                  yes
#
# TWO MODES, because `patch-id` equality is right for a clean rebase and wrong for a conflicted one.
# A conflict resolution MUST change the patch — that is what resolving means — so a single-mode gate
# refuses every resolution and gets waived, which is worse than no gate. `resolved` therefore checks
# a different property: that the resolution changed only what was DECLARED IN ADVANCE, and that this
# side's own lines survived it unaltered.
#
# The declaration is required before the resolution exists, and that ordering is enforced rather
# than requested: `resolved` refuses if the declaration file is newer than the new head's commit
# date. A lane that resolves first and describes afterwards is not checking, it is narrating, and
# the mode with a human in it is the one where a real defect hides — a lane expecting a trivial
# conflict is the exact condition under which a non-trivial one is waved through.
#
# EXIT CODES. 0 preserved. 2 NOT preserved — a hard stop, and the gate's answer about this rebase.
# 5 SETUP — the comparison could not be made, so there is no answer. 5 exists because its absence is
# how this whole class of bug happens: an empty diff from a silently failed cherry-pick compares
# equal to another empty diff and reports PASS, and a missing declaration must not read as a clean
# rebase. Nothing here may return 0 without having compared two non-empty diffs.
set -u

K=${MERGE_KIT:-/tmp/k}
die()   { printf 'SETUP: %s\n' "$1" >&2; exit 5; }
stop()  { printf 'GATE 3 NOT PRESERVED: %s\n' "$1" >&2; exit 2; }
pass()  { printf 'GATE 3 PRESERVED: %s\n' "$1"; exit 0; }

command -v git >/dev/null || die "git is not on PATH"
git rev-parse --git-dir >/dev/null 2>&1 || die "not inside a git repository"

# The declaration must not live inside the worktree it describes. Found by this gate's own
# self-test, which put MERGE_KIT inside the scratch repository: `git add -A` swept the declaration
# into the resolution commit, so the file recording what the diff was allowed to contain became
# part of that diff, and the gate refused its own bookkeeping as an undeclared change. A measuring
# instrument stored inside its subject perturbs it.
if [ -d "$K" ]; then
  top=$(git rev-parse --show-toplevel 2>/dev/null) && kp=$(cd "$K" && pwd -P) && \
    case "$kp/" in "$top"/*) die "MERGE_KIT ($kp) is inside the worktree ($top). Declarations would be committed into the diff they describe. Put the kit outside the repository." ;; esac
fi

# obj <rev>: resolve or refuse. A revision that does not resolve must not become an empty diff.
obj() { git rev-parse --verify --quiet "$1^{commit}" || die "cannot resolve '$1' to a commit"; }

# pdiff <base> <head> [path…]: the diff, or SETUP if it is empty. Empty is never compared, because
# two empty diffs have the same patch-id and that equality is an artefact of a broken measurement
# rather than evidence about a rebase. This guard is here because a silently failed cherry-pick
# produced exactly that during the audit and would have been read as a finding.
pdiff() {
  local b=$1 h=$2; shift 2
  local d; d=$(git diff "$b" "$h" ${1+-- "$@"}) || die "git diff $b..$h failed"
  [ -n "$d" ] && printf '%s\n' "$d"
}

pid() { git patch-id --stable | cut -d' ' -f1; }

# added/removed <diff>: this side's own lines, as a sorted multiset. Hunk headers and context are
# excluded deliberately — they are what a legitimate rebase is allowed to move.
added()   { grep '^+' | grep -v '^+++' | sort; }
removed() { grep '^-' | grep -v '^---' | sort; }

mode=${1:-}; [ -n "$mode" ] || die "usage: $0 clean|declare|resolved …"
shift

case "$mode" in

  clean)
    [ $# -eq 4 ] || die "usage: $0 clean <old-base> <old-head> <new-base> <new-head>"
    ob=$(obj "$1"); oh=$(obj "$2"); nb=$(obj "$3"); nh=$(obj "$4")
    o=$(pdiff "$ob" "$oh"); [ -n "$o" ] || die "the pre-rebase diff $1..$2 is empty — nothing to preserve, so there is nothing to check"
    n=$(pdiff "$nb" "$nh"); [ -n "$n" ] || die "the post-rebase diff $3..$4 is empty — the rebase produced no change, which is a broken replay and not a preserved patch"
    a=$(printf '%s\n' "$o" | pid); b=$(printf '%s\n' "$n" | pid)
    [ -n "$a" ] && [ -n "$b" ] || die "git patch-id produced nothing for a non-empty diff"
    [ "$a" = "$b" ] || stop "patch-id $a (before) != $b (after). The rebase changed content. If this was a conflict resolution, it is not a clean rebase: declare it and use 'resolved'."
    pass "patch-id $a over $(printf '%s\n' "$o" | grep -c '^diff --git ') file(s), clean rebase"
    ;;

  declare)
    # Written BEFORE the resolution exists. Records which paths the resolution may touch and the
    # sentence the lane expects to be true afterwards, so that 'resolved' compares against a claim
    # made in ignorance of the outcome.
    [ $# -ge 4 ] || die "usage: $0 declare <new-base> <old-head> <path>[,<path>…] -- <sentence>"
    nb=$(obj "$1"); oh=$(obj "$2"); paths=$3; shift 3
    [ "${1:-}" = "--" ] || die "the declaration needs -- followed by the sentence you expect to be true after resolving"
    shift; sentence="$*"
    [ -n "$sentence" ] || die "an empty declaration is not a declaration"
    [ -d "$K" ] || die "no kit directory at $K — this is a setup gap, not a gate answer"
    f="$K/gate3-declared.$oh"
    { printf 'new-base: %s\nold-head: %s\npaths: %s\ndeclared-at: %s\nexpectation: %s\n' \
        "$nb" "$oh" "$paths" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$sentence"; } > "$f" || die "cannot write $f"
    printf 'declared: %s\n  paths: %s\n  expectation: %s\n' "$f" "$paths" "$sentence"
    ;;

  resolved)
    [ $# -eq 4 ] || die "usage: $0 resolved <old-base> <old-head> <new-base> <new-head>"
    ob=$(obj "$1"); oh=$(obj "$2"); nb=$(obj "$3"); nh=$(obj "$4")
    f="$K/gate3-declared.$oh"
    [ -f "$f" ] || die "no declaration at $f. A conflicted rebase must be declared before it is resolved; without one there is nothing to check the resolution against."
    paths=$(awk -F': ' '$1=="paths"{print $2}' "$f")
    [ -n "$paths" ] || die "the declaration at $f names no paths"
    # The ordering constraint, enforced rather than requested: a declaration written after the
    # resolution was committed is a description of the outcome, not a prediction about it.
    dsec=$(stat -c %Y "$f" 2>/dev/null) || die "cannot read the declaration's mtime"
    csec=$(git show -s --format=%ct "$nh") || die "cannot read $4's commit date"
    [ "$dsec" -le "$csec" ] || stop "the declaration at $f is NEWER than the resolution commit $4. It was written after the outcome was known, so it is a rationalisation and not a check."

    o=$(pdiff "$ob" "$oh"); [ -n "$o" ] || die "the pre-rebase diff $1..$2 is empty"
    n=$(pdiff "$nb" "$nh"); [ -n "$n" ] || die "the post-rebase diff $3..$4 is empty"

    IFS=',' read -r -a dec <<< "$paths"
    # Every file OUTSIDE the declared region must be preserved exactly, per file. A resolution is
    # allowed to be messy only where it was declared to be.
    bad=""
    for p in $(printf '%s\n%s\n' "$o" "$n" | sed -n 's|^diff --git a/\(.*\) b/.*|\1|p' | sort -u); do
      skip=0; for d in "${dec[@]}"; do [ "$p" = "$d" ] && skip=1; done
      [ "$skip" -eq 1 ] && continue
      x=$(git diff "$ob" "$oh" -- "$p" | pid); y=$(git diff "$nb" "$nh" -- "$p" | pid)
      [ "$x" = "$y" ] || bad="$bad$p "
    done
    [ -z "$bad" ] || stop "these files changed in the rebase and were NOT declared: $bad"

    # Inside the declared region: both sections survive and no line of either is altered. The
    # checkable form of that sentence, from this side, is that this PR's own added and removed
    # lines are the same multiset before and after. Only context and position may move.
    for d in "${dec[@]}"; do
      ao=$(git diff "$ob" "$oh" -- "$d" | added); an=$(git diff "$nb" "$nh" -- "$d" | added)
      ro=$(git diff "$ob" "$oh" -- "$d" | removed); rn=$(git diff "$nb" "$nh" -- "$d" | removed)
      [ "$ao" = "$an" ] || stop "in the declared file $d this side's ADDED lines differ before and after the resolution. The resolution altered a line rather than only choosing an order:
$(diff <(printf '%s\n' "$ao") <(printf '%s\n' "$an") | head -20)"
      [ "$ro" = "$rn" ] || stop "in the declared file $d this side's REMOVED lines differ before and after the resolution:
$(diff <(printf '%s\n' "$ro") <(printf '%s\n' "$rn") | head -20)"
    done
    pass "resolution confined to the declared paths ($paths); this side's lines survive unaltered; every undeclared file preserved by patch-id. Expectation on record: $(awk -F': ' '$1=="expectation"{print $2}' "$f")"
    ;;

  *) die "unknown mode '$mode' — expected clean, declare or resolved" ;;
esac

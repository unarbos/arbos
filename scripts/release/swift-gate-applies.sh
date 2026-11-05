#!/usr/bin/env bash
# Does the Swift core check apply to the commit this run is building?
#
#   scripts/release/swift-gate-applies.sh decide     prints applies=true|false, and appends it to
#                                                    $GITHUB_OUTPUT when that is set
#   scripts/release/swift-gate-applies.sh verdict    exits non-zero unless this commit was tested,
#                                                    reading the other jobs' results from the env
#   scripts/release/swift-gate-applies.sh selftest   asserts the invariants this screen and
#                                                    .github/workflows/swift-core-check.yml rely on
#
# WHY THE DECISION MOVED OUT OF THE PUSH TRIGGER'S `paths:` FILTER.
#
# A required gate must never apply to a head that no run can answer. Before this screen existed it
# could, and the state it produced was the worst available: every check on the pull request read
# SUCCESS or SKIPPED, nothing read red, and the merge was still refused.
#
# The two halves disagreed about which diff they were talking about.
#
#   APPLICABILITY is judged on the pull request's three-dot diff. scripts/merge/checks.sh's
#   gate_applies asks whether THE PULL REQUEST touches a gated path, because that is what decides
#   whether the merged result needs the verdict.
#
#   THE TRIGGER fired on the pushed commit's two-dot diff — this tip against the last one. A push
#   that touches no gated path creates no run at all.
#
# So a branch that touches a gated path early and then lands a final push outside those paths ends
# up applying-but-unanswerable. Measured on #2751: the pull request's diff carries
# crates/arbos-core/src/doc.rs, so the gate applies, but its last push (f4f010268, 2026-11-04
# 22:21Z) changed only crates/arbos-agentd/src/sync.rs, so no push run was ever created for that
# head. The only run on it was the pull_request run, which the workflow skips by design for
# same-repo branches, and `merge-window.sh checks f4f010268 2751` answered:
#
#   required check 'Swift core check' on f4f010268: NO VERDICT — 1 run(s) on this head, every one
#   skipped or neutral, so none of them tested it
#   RESULT: REFUSE — a gate that applies to this head has no successful run on it
#
# Nothing on the pull request showed that. A human merging on "every check is green" merges an
# untested Mac-affecting change; the owner hand-merges on exactly that rule.
#
# THE FIX IS TO REMOVE THE PUSH TRIGGER'S FILTER, NOT TO NARROW WHAT THE GATE COVERS. Every push now
# starts a run, and this screen decides inside it whether the macOS job is needed — on the same diff
# gate_applies uses. Applicability and answerability are then computed from one basis, so they
# cannot drift apart again. Narrowing the gate instead would have traded a visible refusal for a
# silent hole, which is the direction the last several gate bugs already went.
#
# THE PATTERN LIST STAYS IN THE WORKFLOW'S `on:` BLOCK, and the pull_request filter keeps it. That is
# deliberate: checks.sh's gate_patterns reads the quoted patterns out of that block to decide
# applicability, and treats a block it can read but finds empty as BLOCKED. Moving the list into
# this file would leave the kit with nothing to read. One list, in the place the kit already looks,
# parsed here with the same two shapes gate_patterns accepts.
#
# WHAT THIS COSTS. One macOS build per head that genuinely needs one — the build that was missing,
# never a second copy of one that already ran. The same-repo pull_request run stays skipped, because
# the branch's push run now always exists; that assumption used to be false, which is what made the
# skip unsafe. The pool allows this repo five jobs at once and has been the binding constraint all
# evening, so adding load was not an option.
#
# A SIDE EFFECT WORTH HAVING. A push that created no run also joined no concurrency group and so
# displaced nothing: run 41918138101 was still building c187a9c00 forty-five minutes after that sha
# stopped being the branch tip, holding a macOS runner to produce a verdict about superseded content
# that could never attach to the head. Now that every push starts a run, the group's
# cancel-in-progress reaches those builds.
set -euo pipefail

WF=.github/workflows/swift-core-check.yml

# Every quoted path pattern under a workflow's top level `on:` block, read from stdin.
#
# This is checks.sh's gate_patterns awk, unchanged on purpose. The two read the same bytes and must
# reach the same list; if one is taught a new shape and the other is not, the disagreement is
# exactly the class of bug this file exists to close.
patterns_in() {
  awk '
    /^on:/          { inon = 1; next }
    /^[^[:space:]]/ { inon = 0 }
    inon && /^ *- *"/ { gsub(/^ *- *"|" *$/, ""); print }'
}

# This head's patterns unioned with main's, which is what checks.sh's gate_applies uses.
#
# Judging by this head's list alone reopens the hole from the other side. gate_applies unions both
# because a path added to the filter on main after a branch forked still gates the merged result:
# #2376 changes crates/arbos-matrix/src/client.rs, and crates/arbos-matrix/** reached the filter
# after that branch forked. Deciding here on the head's list alone would answer "does not apply",
# skip the macOS job, and leave `decide` succeeding beside it — a run that concludes success with
# nothing built, which checks.sh reads as a pass while it asks for a verdict on the same head.
# One basis for applicability and answerability means the same basis checks.sh uses, not merely a
# consistent one.
#
# HEAD_WF names the working-tree copy and defaults to $WF. They are separate names so the selftest
# can stand a synthetic head beside the real main; $WF alone would redirect both reads at once and
# the test would pass against nothing.
patterns() {
  { cat "${HEAD_WF:-$WF}"; git show "origin/main:$WF" 2>/dev/null || true; } | patterns_in | sort -u
}

# The files this commit should be judged on, which is not the same question on every event.
#
# On a pull request and on a branch push it is the three-dot diff against main — what the merge
# would bring. On main it is the commit's own first-parent diff, which is what checks.sh reads for
# a landed commit through commit_files. Getting this wrong in the main case would be silent: a
# three-dot diff of main against main is empty, so the gate would never apply to anything on main.
changed() {
  local base
  case "${GITHUB_REF:-}" in
    refs/heads/main) git diff --name-only HEAD^ HEAD ;;
    *)
      base=$(git merge-base origin/main HEAD) || return 1
      git diff --name-only "$base" HEAD
      ;;
  esac
}

# 0 when any file matches any pattern, 1 when none do, 2 when a pattern cannot be read.
#
# The two shapes this repo's filters use, and no others: a `dir/**` prefix and a literal path.
# Anything else returns 2 and the caller fails the run, because a pattern this cannot evaluate is an
# unknown answer and an unknown answer about a required gate is not a pass.
match() {
  local files=$1 pats=$2 f p
  while IFS= read -r p; do
    [ -n "$p" ] || continue
    case "$p" in
      */'**') ;;
      *'*'*) return 2 ;;
    esac
  done <<< "$pats"
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    while IFS= read -r p; do
      [ -n "$p" ] || continue
      case "$p" in
        */'**') case "$f" in "${p%'**'}"*) return 0 ;; esac ;;
        *) [ "$f" = "$p" ] && return 0 ;;
      esac
    done <<< "$pats"
  done <<< "$files"
  return 1
}

decide() {
  local pats files
  pats=$(patterns)
  if [ -z "$pats" ]; then
    echo "$WF declares no quoted path patterns under its \`on:\` block. That block is where" \
         "scripts/merge/checks.sh reads this gate's scope, so an empty one means neither this" \
         "screen nor the merge kit can tell what the gate covers. Refusing rather than deciding." >&2
    exit 2
  fi
  if ! files=$(changed); then
    echo "Could not work out which files this commit changes, so whether the gate applies is" \
         "unknown. Check that the job checks out with fetch-depth: 0 and that origin/main is" \
         "fetched." >&2
    exit 2
  fi

  # `|| rc=$?` rather than a bare call: match returns 1 for "no gated path touched", which is an
  # answer and not a failure, and under `set -e` a bare call would exit the script on it — silently
  # producing no `applies` output at all, which the job would read as the gate not applying.
  local rc=0
  match "$files" "$pats" || rc=$?
  case $rc in
    0) applies=true ;;
    1) applies=false ;;
    *) echo "$WF has a path pattern this cannot evaluate, so whether the gate applies is unknown." >&2
       exit 2 ;;
  esac

  echo "applies=$applies"
  [ -n "${GITHUB_OUTPUT:-}" ] && echo "applies=$applies" >> "$GITHUB_OUTPUT"
  echo "$(printf '%s\n' "$files" | grep -c .) file(s) changed, against $(printf '%s\n' "$pats" | grep -c .) gated path(s)" >&2
  return 0
}

# The one job in this workflow that reports success, and it does so only for a commit that was
# tested. Everything it reads is a job result from the environment, so it has no opinion of its
# own to be wrong about — it refuses whatever it cannot read as a verdict.
#
# Every unknown is a refusal. `decide` not succeeding means nobody established whether this commit
# needs the macOS build, and an unestablished answer about a required gate is not a pass; an
# `applies` value that is neither true nor false means the same thing one layer down.
verdict() {
  local d=${DECIDE_RESULT:-} a=${DECIDE_APPLIES:-} c=${CHECK_RESULT:-}

  if [ "$d" != success ]; then
    echo "The applicability screen did not answer (decide: ${d:-missing}), so whether this commit" \
         "needs the macOS build is unknown, and an unknown answer about a required gate is not a" \
         "pass. Read the \`decide\` job for why it could not say." >&2
    exit 1
  fi

  case "$a" in
    false)
      echo "The Swift core check does not apply to this commit: its diff touches none of the gated" \
           "paths, so there is nothing for the macOS job to test and nothing to wait for."
      return 0 ;;
    true) ;;
    *)
      echo "The screen succeeded but reported applies='${a:-}', which is neither true nor false," \
           "so whether this commit needs the macOS build is unknown. Refusing rather than guessing." >&2
      exit 1 ;;
  esac

  if [ "$c" = success ]; then
    echo "The Swift core check applies to this commit and the macOS job passed on it."
    return 0
  fi

  echo "The Swift core check applies to this commit — its diff touches a gated path — and the" \
       "macOS job did not pass (check: ${c:-missing}). A skipped or cancelled build is not a" \
       "failure, but it is not a verdict either, and this gate is required: nothing here tested" \
       "this commit, so this run refuses rather than concluding success on its siblings." >&2
  exit 1
}

# The invariants, asserted on every run rather than written down and hoped for. Each one is a way
# the applying-but-unanswerable state comes back.
selftest() {
  local fail=0 body
  ok()  { echo "  ok    $1"; }
  bad() { echo "  BAD   $1"; fail=1; }

  [ -f "$WF" ] || { echo "  BAD   $WF is not in this tree, so this is testing nothing"; exit 1; }
  body=$(cat "$WF")

  # 1. The push trigger must carry no `paths:`. Restoring it restores the original defect: a final
  #    push outside the gated paths creates no run, and the head the gate applies to has nothing
  #    that can answer it.
  if printf '%s\n' "$body" | awk '/^  push:/{p=1;next} /^  [a-z_]+:/{p=0} p && /^    paths:/{found=1} END{exit !found}'; then
    bad "the push trigger has a paths: filter again — a head the gate applies to can now exist with no run able to answer it"
  else
    ok "the push trigger filters no paths, so every push to a branch starts a run"
  fi

  # 2. The pull_request trigger must keep the list. checks.sh reads the `on:` block for the gate's
  #    scope and blocks on a block it can read but finds empty, so emptying it does not widen the
  #    gate, it stops the kit being able to judge it at all.
  if [ -n "$(patterns)" ]; then
    ok "the \`on:\` block still declares the gated paths that checks.sh reads"
  else
    bad "no quoted path patterns under \`on:\` — scripts/merge/checks.sh reads this block for the gate's scope and BLOCKS when it is empty"
  fi

  # 3. The macOS job must depend on this screen. Without the dependency it either runs every time,
  #    on a pool that allows five jobs at once, or reverts to trigger filtering.
  if printf '%s\n' "$body" | grep -q 'needs.decide.outputs.applies'; then
    ok "the macOS job runs on this screen's answer"
  else
    bad "the macOS job does not read needs.decide.outputs.applies, so the decision is being made somewhere else again"
  fi

  # 4. The same-repo pull_request skip is only sound while every same-repo push starts a run.
  #    Invariant 1 is what makes it sound; this case records that they stand or fall together.
  if printf '%s\n' "$body" | grep -q "pull_request.*head.repo.full_name != github.repository\|github.event_name != 'pull_request'"; then
    ok "the same-repo pull_request run still defers to the push run, which invariant 1 guarantees exists"
  else
    bad "the same-repo deferral is gone — every pull request now doubles the macOS load the pool cannot absorb"
  fi

  # 5. This screen and the merge kit must accept the same pattern shapes. They read the same bytes;
  #    a shape one understands and the other does not is a silent disagreement about scope.
  if printf '%s\n' "$(patterns)" | grep -qvE '^[^*]+$|^[^*]+/\*\*$'; then
    bad "a pattern in $WF is neither a literal path nor a dir/** prefix — checks.sh BLOCKS on those and this screen exits 2"
  else
    ok "every declared pattern is a shape both this screen and checks.sh can evaluate"
  fi

  # 6. The decision basis must be the union of this head's list and main's, which is what
  #    checks.sh's gate_applies asks about. Comparing the two real lists proves nothing — they are
  #    identical whenever nobody has changed the filter — so this stands a synthetic head beside
  #    the real main and requires both lists in the answer. Head-only loses main's paths and skips
  #    a build main still gates; main-only loses the head's and skips one this branch adds.
  local synth union main_pats
  main_pats=$( { git show "origin/main:$WF" 2>/dev/null || true; } | patterns_in | sort -u)
  if [ -z "$main_pats" ]; then
    bad "origin/main has no readable pattern list for $WF, so this screen cannot tell whether it is deciding on main's scope — fetch origin/main"
  else
    synth=$(mktemp)
    printf 'name: synthetic\non:\n  push:\n    paths:\n      - "nowhere/in/main/**"\n' > "$synth"
    union=$(HEAD_WF=$synth patterns)
    rm -f "$synth"
    if ! printf '%s\n' "$union" | grep -qx 'nowhere/in/main/\*\*'; then
      bad "the head's own patterns are missing from the decision basis — a path this branch adds to the filter would not gate this branch"
    elif ! printf '%s\n' "$main_pats" | while IFS= read -r p; do printf '%s\n' "$union" | grep -qxF "$p" || exit 1; done; then
      bad "main's patterns are missing from the decision basis — a path added to the filter after this branch forked still gates the merged result, and the build for it would be skipped while checks.sh asks for the verdict"
    else
      ok "the decision basis unions this head's patterns with main's, the same scope checks.sh asks about"
    fi
  fi

  # 7. The deferral must be declared on `decide`, not on the macOS job alone. A run is only skipped
  #    when every job skipped; one cheap job succeeding beside a skipped build concludes the run
  #    success, which checks.sh reads as a pass. Measured on 6f3061016 before this moved.
  if printf '%s\n' "$body" | awk '/^  decide:/{d=1;next} /^  [a-z_]+:/{d=0} d && /head\.repo\.full_name != github\.repository/{found=1} END{exit !found}'; then
    ok "the same-repo deferral is on \`decide\`, so a deferred run skips whole and still reads as no verdict"
  else
    bad "the same-repo deferral is not on \`decide\` — a deferred run would conclude success with nothing built, which checks.sh reads as a pass"
  fi

  # 8. The run must not be able to conclude without this screen refusing. `verdict` is the only job
  #    that reports success, so it has to run in every case a run could otherwise conclude on its
  #    siblings — and skip in the one case the run is meant to skip whole.
  if printf '%s\n' "$body" | awk '/^  verdict:/{v=1;next} /^  [a-z_]+:/{v=0} v && /always\(\)/ && /needs\.decide\.result != .skipped./{found=1} END{exit !found}'; then
    ok "the verdict job runs whenever a run could conclude on a skipped build, and skips only with the deferral"
  else
    bad "the verdict job is missing, does not use always(), or no longer defers with decide — a skipped macOS job would conclude the run success again"
  fi

  # 9. The verdict's truth table, exercised rather than described. Every row that is not a tested
  #    commit must refuse, including the shapes that are nobody's fault: a cancelled build and a
  #    screen that failed are not failures, and they are not verdicts either.
  local rows want got desc d a c
  rows='success|true|success|0|the gate applies and the build passed
success|false|skipped|0|the gate does not apply, so there is nothing to test
success|true|skipped|1|the gate applies and the build was skipped — the bug this job exists for
success|true|failure|1|the gate applies and the build failed
success|true|cancelled|1|the gate applies and the build was cancelled
success|true||1|the gate applies and no build result reached this job
success|||1|the screen reported no applies value at all
success|maybe|success|1|the screen reported an applies value neither true nor false
failure|true|success|1|the screen itself failed, so applicability is unknown
skipped|false|skipped|1|the screen was skipped, so applicability is unknown'
  while IFS='|' read -r d a c want desc; do
    [ -n "$want" ] || continue
    got=0
    ( DECIDE_RESULT=$d DECIDE_APPLIES=$a CHECK_RESULT=$c verdict ) >/dev/null 2>&1 || got=$?
    [ "$got" -ne 0 ] && got=1
    if [ "$got" = "$want" ]; then
      ok "verdict $([ "$want" = 0 ] && echo passes || echo refuses): $desc"
    else
      bad "verdict should $([ "$want" = 0 ] && echo pass || echo refuse) when $desc (decide=$d applies=$a check=$c) but exited $got"
    fi
  done <<< "$rows"

  echo
  if [ "$fail" -eq 0 ]; then echo "swift-gate-applies selftest: PASS"; else echo "swift-gate-applies selftest: FAIL"; fi
  return "$fail"
}

case "${1:-}" in
  decide)   decide ;;
  verdict)  verdict ;;
  selftest) selftest ;;
  *) echo "usage: $0 decide|verdict|selftest" >&2; exit 64 ;;
esac

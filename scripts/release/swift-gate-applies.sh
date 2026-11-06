#!/usr/bin/env bash
# Does the Swift core check apply to the commit this run is building?
#
#   scripts/release/swift-gate-applies.sh decide     prints applies=true|false, and appends it to
#                                                    $GITHUB_OUTPUT when that is set
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

# A baseline run is not asking the scope question, so it does not get the scope answer.
#
# `schedule` and `workflow_dispatch` exist on this workflow to measure the SUITE, not to judge a
# change. Sending them through the diff below would answer "does not apply" on almost every night —
# on main `changed()` reads the merge commit's first-parent diff, and most merged pull requests do
# not touch iOS — and a baseline that skips is the exact defect it was added to remove. So the
# answer is forced here, where it is one case statement under a comment, rather than by widening
# what the gate covers for everyone else. Nothing on the pull_request or push path reads this.
baseline_event() {
  case "${GITHUB_EVENT_NAME:-}" in
    schedule|workflow_dispatch) return 0 ;;
    *) return 1 ;;
  esac
}

decide() {
  local pats files
  if baseline_event; then
    echo "applies=true"
    [ -n "${GITHUB_OUTPUT:-}" ] && echo "applies=true" >> "$GITHUB_OUTPUT"
    echo "$GITHUB_EVENT_NAME is a baseline run: it measures the suite rather than a change, so the" \
         "gate's scope does not decide it and the macOS job always runs." >&2
    return 0
  fi
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

  # 8. The baseline triggers must be declared. Without them main measures the suite only when
  #    somebody's change happens to drag the macOS job along, which is the state this answered:
  #    main's macOS job executed once in main's last 40 runs.
  #
  #    Cases 8 to 10 read the workflow file, so each one is also run against a body with the
  #    violation injected. A screen that passes because it is reading nothing is this repo's
  #    recurring lint failure, and the only defence is making it fire on demand. `probe` runs one
  #    check against one body and reports whether it held.
  probe() { printf '%s\n' "$2" | eval "$1"; }

  # Both triggers, and at least one cron under the schedule.
  check_baseline_triggers='awk "
    /^on:/{o=1;next} /^[^[:space:]]/{o=0}
    o && /^  schedule:/{s=1} o && /^  workflow_dispatch:/{d=1} o && /^ *- cron: \"/{c=1}
    END{exit !(s && d && c)}"'
  if probe "$check_baseline_triggers" "$body"; then
    ok "the \`on:\` block declares both baseline triggers and a cron"
  else
    bad "no \`schedule:\` cron and \`workflow_dispatch:\` under \`on:\` — nothing measures this suite except somebody's change, and on a quiet main that is almost never"
  fi
  if probe "$check_baseline_triggers" "$(printf '%s\n' "$body" | grep -v '^  schedule:')"; then
    bad "case 8 passed a body with \`schedule:\` deleted, so it is not reading the file"
  else
    ok "case 8 fires when \`schedule:\` is removed"
  fi

  # 9. The cron must not reach the gated path list. patterns_in and checks.sh's gate_patterns match
  #    `^ *- *"` inside the `on:` block, and `- cron: "41 10 * * *"` escapes it only because the
  #    quote is not adjacent to the dash. Written the other way round — `- "41 10 * * *"` under a
  #    bare `schedule:` — the cron string becomes a path pattern, and a pattern with a `*` in the
  #    middle makes gate_applies return 2, which checks.sh turns into BLOCKED on every open pull
  #    request. Case 5 would catch that too; this one says why it must be looked for here.
  if printf '%s\n' "$(patterns)" | grep -q '[0-9] [0-9*]'; then
    bad "a cron expression has leaked into the gated path list — checks.sh would read it as a path pattern and BLOCK"
  else
    ok "the cron stays out of the gated path list that checks.sh reads"
  fi

  # 10. Baseline runs must sit in their own concurrency group and must never be cancelled.
  #     `cancel-in-progress: false` does not stop GitHub cancelling an already-PENDING run when a
  #     newer one joins the group, and on this pool pending is 84 to 126 minutes. Sharing a group
  #     with the change runs therefore lets a nightly kill a queued push run a lane is waiting on,
  #     or be killed by one. Both halves are textual: this asserts the group branches on the event
  #     at all and that neither baseline event can be cancelled.
  check_group_splits='awk "/^concurrency:/{c=1;next} /^[^[:space:]]/{c=0}
    c && /^  group:/ && /github.event_name/{g=1}
    c && /^  cancel-in-progress:/ && /event_name != .schedule./ && /event_name != .workflow_dispatch./{k=1}
    END{exit !(g && k)}"'
  if probe "$check_group_splits" "$body"; then
    ok "baseline runs get their own concurrency group and are never cancelled"
  else
    bad "the concurrency group does not separate baseline runs from change runs, or lets one be cancelled — a nightly can then cancel a queued gate run, which reads as a run with no job in it"
  fi
  if probe "$check_group_splits" "$(printf '%s\n' "$body" | sed 's/^  group: swift-core-check-.*/  group: swift-core-check-${{ github.ref }}/')"; then
    bad "case 10 passed a body whose group was reverted to the ref-only key, so it is not reading the file"
  else
    ok "case 10 fires when the group is reverted to the ref-only key"
  fi

  # 11. The bypass must be the one in this file. A trigger-level filter would be the old defect
  #     wearing a new hat, and a second copy of the decision is a second thing to drift.
  if GITHUB_EVENT_NAME=schedule GITHUB_OUTPUT='' decide 2>/dev/null | grep -qx 'applies=true' &&
     GITHUB_EVENT_NAME=workflow_dispatch GITHUB_OUTPUT='' decide 2>/dev/null | grep -qx 'applies=true'; then
    ok "both baseline events decide applies=true here, without consulting any diff"
  else
    bad "a baseline event does not force applies=true — the nightly would skip the macOS job, which is the whole defect it exists to fix"
  fi

  echo
  if [ "$fail" -eq 0 ]; then echo "swift-gate-applies selftest: PASS"; else echo "swift-gate-applies selftest: FAIL"; fi
  return "$fail"
}

case "${1:-}" in
  decide)   decide ;;
  selftest) selftest ;;
  *) echo "usage: $0 decide|selftest" >&2; exit 64 ;;
esac

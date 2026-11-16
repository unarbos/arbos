#!/usr/bin/env bash
# Did the event that started this run hand the release gate a pull request it can judge?
#
#   GATE_BASE_REF=main GATE_HEAD_SHA=<sha> scripts/release/gate-can-decide.sh
#
# Prints `base_ref=` and `head_sha=` and exits 0 when both are present. Exits 2 — the gate's
# "I could not tell" code, which scripts/merge/checks.sh reads as a refusal — when either is
# missing. Never exits 0 without them.
#
# With GATE_REQUIRE_MAIN_BASE set, a base branch other than main is a second way the run cannot
# decide, and exits 2 as well. Only the gate job sets it; see "why only the gate job" below.
#
# Why this exists. .github/workflows/release-gate.yml used to add `workflow_dispatch`, and on
# that event `github.event.pull_request` is null, so the gate step's base-ref fallback landed on
# `github.ref_name`, saw a branch rather than main, and exited 0 having judged nothing. The run
# then concluded `success` carrying the branch head, and checks.sh's `gate_state` takes the first
# success on a head sha without asking which event produced it — so the gate was cleared on a head
# it had never looked at. The dispatch trigger is gone, and this screen is what keeps it gone.
#
# The condition is written against what the event can supply, not against the event's name, and
# that distinction is the point. release-gate.yml is a special case: it judges one pull request
# against main, so an event carrying no pull request genuinely cannot decide. That is a property
# of this one workflow, not a rule about events. .github/workflows/swift-core-check.yml does real
# work on `push` and its push passes are real verdicts; do not copy this screen there.
#
# A trigger added later gets the same treatment, which is the useful part: the new event supplies
# no pull request, so every job refuses until someone teaches the gate what that event means. A
# trigger nobody taught the gate about fails closed and says so, rather than passing quietly.
#
# ── the second undecidable shape: a base branch that is not main ────────────────────────────────
# Only main publishes, so a pull request into any other branch has no release to measure against.
# That was true before and the gate said so — and then exited 0, which is the same defect as the
# dispatch above wearing different clothes. A stacked pull request reaches it through the front
# door rather than through a trigger nobody screened, so every stacked pull request in the
# repository carried a green release gate that had judged nothing, and it turned into a real
# verdict the moment the base was retargeted at main, which is the one moment nobody re-checks.
#
# GitHub Actions cannot answer `neutral`: exit 0 is success and every nonzero exit is failure, and
# since 2025-03-31 a workflow token cannot set an Actions check run's conclusion either. The only
# other non-verdict a run can reach is `skipped`, which needs a job-level `if:` that
# release-gate.py forbids here for the reason above. So exit 2 — the code this gate already uses
# for "I could not decide" — is what is left, and it is the honest one: checks.sh reads it as a
# refusal rather than as a pass, which is the whole point.
#
# ── why only the gate job ───────────────────────────────────────────────────────────────────────
# The self-test job's cases are offline and pinned, so it can decide on any base, and a stacked
# pull request that edits the predicate is exactly the one whose self-test must still run. Making
# it refuse too would cost the coverage this change exists to protect. The gate job's verdict is
# what checks.sh reads, and one failing job concludes the run, so the asymmetry costs nothing.
set -euo pipefail

base_ref="${GATE_BASE_REF:-}"
head_sha="${GATE_HEAD_SHA:-}"

# Both refusals print to stderr and, on a runner, to the job summary, because the sentence is the
# only thing that separates "this gate is broken" from "this gate was handed nothing to judge".
# Arguments are joined with a space, as `echo` joined them before: the sentence has to stay one
# line, because the self-test matches phrases that span what would otherwise be a line break.
refuse() {
  echo "$*" >&2
  [[ -n "${GITHUB_STEP_SUMMARY:-}" ]] && echo "$*" >> "$GITHUB_STEP_SUMMARY"
  exit 2
}

if [[ -z "$base_ref" || -z "$head_sha" ]]; then
  refuse "This run carries no pull request: base ref is '${base_ref:-<empty>}' and head sha is" \
         "'${head_sha:-<empty>}'. The release gate judges one pull request against main, so there" \
         "is nothing here for it to decide. Refusing rather than reporting a pass on a head this" \
         "run never looked at."
fi

if [[ -n "${GATE_REQUIRE_MAIN_BASE:-}" && "$base_ref" != main ]]; then
  refuse "Base branch is '$base_ref', not main: merging it publishes nothing. The release gate" \
         "measures what reaches users, so there is nothing here for it to decide, and a pass" \
         "would say this pull request had been judged. Refusing instead — re-target it at main" \
         "and this gate decides."
fi

echo "base_ref=$base_ref"
echo "head_sha=$head_sha"

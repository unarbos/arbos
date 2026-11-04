#!/usr/bin/env bash
# Did the event that started this run hand the release gate a pull request to judge?
#
#   GATE_BASE_REF=main GATE_HEAD_SHA=<sha> scripts/release/gate-can-decide.sh
#
# Prints `base_ref=` and `head_sha=` and exits 0 when both are present. Exits 2 — the gate's
# "I could not tell" code, which scripts/merge/checks.sh reads as a refusal — when either is
# missing. Never exits 0 without them.
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
set -euo pipefail

base_ref="${GATE_BASE_REF:-}"
head_sha="${GATE_HEAD_SHA:-}"

if [[ -z "$base_ref" || -z "$head_sha" ]]; then
  echo "This run carries no pull request: base ref is '${base_ref:-<empty>}' and head sha is" \
       "'${head_sha:-<empty>}'. The release gate judges one pull request against main, so there" \
       "is nothing here for it to decide. Refusing rather than reporting a pass on a head this" \
       "run never looked at." >&2
  exit 2
fi

echo "base_ref=$base_ref"
echo "head_sha=$head_sha"

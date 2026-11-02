#!/usr/bin/env bash
# merge.sh's second pass over the check rollup, exercised against a recorded rollup. The pass decides
# rule 9 — a red check is settled by a run on the identical head sha that is green for that same check
# — and the only other way to see it decide is to find a PR in that state and merge it, which is not a
# test. So the rollup is recorded instead, and each case is one edit to it.
#
# Both pieces are lifted out of merge.sh rather than re-typed: merge.sh does its work on being sourced
# and wants a token, a merge list and a real PR. If either is renamed or reshaped the extraction finds
# nothing and this fails loudly, which is the point of extracting rather than copying.
set -euo pipefail

command -v jq >/dev/null || { echo "FAIL: jq is required"; exit 1; }

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
src="$here/merge.sh"
fn="$(sed -n '/^rollup_red()/,/^}/p' "$src")"
[ -n "$fn" ] || { echo "FAIL: no rollup_red() in $src"; exit 1; }
eval "$fn"
loop="$(sed -n '/^while read -r twin id name; do$/,/^done <<< "\$red"$/p' "$src")"
[ -n "$loop" ] || { echo "FAIL: no 'while read -r twin id name' loop in $src"; exit 1; }

# Recorded from `gh pr view 2569 --json statusCheckRollup` on 2026-11-02, head
# f4297c758f30acfbce5fbe4f83e3adab3cb106d0: `rust (fmt, clippy, test)` concluded failure in the push
# run 41513010699 and success in the pull_request run 41513719101, on that one sha. That is the case
# this pass used to refuse. Timestamps are dropped; nothing reads them.
base='{"statusCheckRollup":[
 {"__typename":"CheckRun","conclusion":"SUCCESS","status":"COMPLETED","workflowName":"CI","name":"Layout",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41513719101/job/125511769820"},
 {"__typename":"CheckRun","conclusion":"SUCCESS","status":"COMPLETED","workflowName":"CI","name":"Layout",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41513010699/job/125509340859"},
 {"__typename":"CheckRun","conclusion":"FAILURE","status":"COMPLETED","workflowName":"CI","name":"rust (fmt, clippy, test)",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41513010699/job/125509341752"},
 {"__typename":"CheckRun","conclusion":"SUCCESS","status":"COMPLETED","workflowName":"CI","name":"rust (fmt, clippy, test)",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41513719101/job/125511769431"},
 {"__typename":"CheckRun","conclusion":"SUCCESS","status":"COMPLETED","workflowName":"CI","name":"harness (arbos-pi-ext typecheck + build + test)",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41513719101/job/125511770117"},
 {"__typename":"CheckRun","conclusion":"SUCCESS","status":"COMPLETED","workflowName":"CI","name":"harness (arbos-pi-ext typecheck + build + test)",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41513010699/job/125509341020"},
 {"__typename":"CheckRun","conclusion":"SKIPPED","status":"COMPLETED","workflowName":"CI","name":"image (dockerfile lint)",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41513719101/job/125514964918"},
 {"__typename":"CheckRun","conclusion":"SKIPPED","status":"COMPLETED","workflowName":"CI","name":"image (dockerfile lint)",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41513010699/job/125510265221"}
]}'

# The one green row this is all about, addressed the same way by every case below.
GREEN='.name == "rust (fmt, clippy, test)" and .conclusion == "SUCCESS"'
RED='41513010699 rust (fmt, clippy, test) FAILURE'

fails=0
report() { # report <ok|fail> <what> <got> <want>
  if [ "$1" = ok ]; then echo "ok   $2"; return; fi
  echo "FAIL $2"; echo "       got:  ${3:-<nothing>}"; echo "       want: ${4:-<nothing>}"; fails=$((fails + 1))
}

rows() { # rows <what> <jq edit of the recorded rollup> <expected lines>
  local got
  if ! got=$(printf '%s' "$base" | jq -c "$2" | rollup_red); then
    report fail "$1" "the fixture or rollup_red did not parse" "$3"; return
  fi
  if [ "$got" = "$3" ]; then report ok "$1"; else report fail "$1" "$got" "$3"; fi
}

rows "the recorded rollup: the red row names its green twin's run" '.' \
  "41513719101 $RED"
rows "no twin once the green row is gone" \
  "(.statusCheckRollup |= map(select($GREEN | not)))" \
  "- $RED"
# A twin is the same check, not a similar one. Were the name ignored, any green row anywhere in the
# rollup would settle every red row in it.
rows "a green row under another name is not a twin" \
  "(.statusCheckRollup |= map(if $GREEN then .name += \" (rerun)\" else . end))" \
  "- $RED"
rows "the same name from another workflow is not a twin" \
  "(.statusCheckRollup |= map(if $GREEN then .workflowName = \"CI (nightly)\" else . end))" \
  "- $RED"
# skipped and neutral are the trap checks.sh exists to name: a job that did no work is not a verdict,
# so it cannot be the green run rule 9 means.
rows "a skipped row of the same check is not a twin" \
  "(.statusCheckRollup |= map(if $GREEN then .conclusion = \"SKIPPED\" else . end))" \
  "- $RED"
rows "a neutral row of the same check is not a twin" \
  "(.statusCheckRollup |= map(if $GREEN then .conclusion = \"NEUTRAL\" else . end))" \
  "- $RED"
rows "a still-running row of the same check is not a twin" \
  "(.statusCheckRollup |= map(if $GREEN then (.conclusion = null | .status = \"IN_PROGRESS\") else . end))" \
  "- $RED"
rows "every other conclusion this pass calls red still finds the twin" \
  "(.statusCheckRollup |= map(if .conclusion == \"FAILURE\" then .conclusion = \"TIMED_OUT\" else . end))" \
  "41513719101 41513010699 rust (fmt, clippy, test) TIMED_OUT"
rows "a red row whose url carries no run id is still untraceable" \
  "(.statusCheckRollup |= map(if .conclusion == \"FAILURE\" then .detailsUrl = \"https://example.invalid/build/7\" else . end))" \
  "41513719101 untraceable rust (fmt, clippy, test) FAILURE"
rows "an all-green rollup says nothing" \
  '(.statusCheckRollup |= map(.conclusion = "SUCCESS"))' \
  ""
# A commit status has no run behind it, so neither side can be named; the twin still settles the row,
# and "untraceable" in the first field says the reader has nothing to go and read.
rows "a commit status twin settles the row but cannot be named" \
  '.statusCheckRollup = [
     {"__typename":"StatusContext","state":"FAILURE","context":"buildkite/arbos","targetUrl":"https://buildkite.test/a"},
     {"__typename":"StatusContext","state":"SUCCESS","context":"buildkite/arbos","targetUrl":"https://buildkite.test/b"}]' \
  "untraceable untraceable buildkite/arbos FAILURE"
rows "a check run and a commit status sharing a name are not twins" \
  '.statusCheckRollup = [
     {"__typename":"CheckRun","conclusion":"FAILURE","status":"COMPLETED","workflowName":"CI","name":"build",
      "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41513010699/job/125509341752"},
     {"__typename":"StatusContext","state":"SUCCESS","context":"build","targetUrl":"https://buildkite.test/b"}]' \
  "- 41513010699 build FAILURE"

# And the decision merge.sh makes from those lines, which is the behaviour that changed.
# shellcheck disable=SC2034  # red, acc and SHA are merge.sh's names, read by the extracted loop.
decide() { # decide <rollup_red lines> <accept-red ids>
  ( set +e
    red=$1; acc=",$2,"; SHA=f4297c758f30acfbce5fbe4f83e3adab3cb106d0
    eval "$loop" )
}

verdict() { # verdict <what> <rollup_red lines> <accept-red ids> <expected status> <expected output>
  local out rc=0
  out=$(decide "$2" "$3") || rc=$?
  [ "$rc" = "$4" ] && [ "$out" = "$5" ] && { report ok "$1"; return; }
  report fail "$1" "exit $rc: $out" "exit $4: $5"
}

verdict "a named twin lets the merge go on, and says which run settled it" "41513719101 $RED" "" 0 \
  "    red check settled by run 41513719101, green for this same check on f4297c758 (rule 9): $RED"
verdict "a red row with no twin still refuses" "- $RED" "" 3 \
  "NOT MERGED: red check not accepted: run $RED"
verdict "accept-red still works for a red row with no twin" "- $RED" 41513010699 0 \
  "    red check accepted by run id: $RED"
verdict "no red rows, nothing to say" "" "" 0 ""

[ "$fails" -eq 0 ] || { echo "$fails failed"; exit 1; }
echo "all ok"

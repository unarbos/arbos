#!/usr/bin/env bash
# merge.sh's second pass over the check rollup, exercised against recorded rollups. The pass decides
# rule 9 — a red check is settled by a run on the identical head sha that is green for that same check
# and tested at least as much of the merged result — and the only other way to see it decide is to find
# a PR in that state and merge it, which is not a test. So the rollups are recorded instead, and each
# case is one edit to one of them.
#
# Every piece is lifted out of merge.sh rather than re-typed: merge.sh does its work on being sourced
# and wants a token, a merge list and a real PR. If one is renamed or reshaped the extraction finds
# nothing and this fails loudly, which is the point of extracting rather than copying. Only `gh` is
# replaced, by a stub that answers from a recorded table of run events.
set -euo pipefail

command -v jq >/dev/null || { echo "FAIL: jq is required"; exit 1; }

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
src="$here/merge.sh"
lift() { # lift <function name>
  local body; body="$(sed -n "/^$1()/,/^}/p" "$src")"
  [ -n "$body" ] || { echo "FAIL: no $1() in $src"; exit 1; }
  eval "$body"
}
lift rollup_red
lift run_event
lift tested_rank
lift green_twin
# The event prefetch and the decision loop together, because the first exists to make the second
# cheap and testing them apart would not show that it does.
loop="$(sed -n '/^for rid in /,/^done <<< "\$red"$/p' "$src")"
[ -n "$loop" ] || { echo "FAIL: no event prefetch and decision loop in $src"; exit 1; }
case "$loop" in *'while read -r id twins name; do'*) ;;
  *) echo "FAIL: the lifted block in $src is not the decision loop"; exit 1 ;; esac

R=unarbos/arbos

# The events of the runs named below, read once with `gh api repos/unarbos/arbos/actions/runs/<id>
# --jq .event` on 2026-11-02. 41513010700 is invented, to stand for a second run of the same event;
# 41599999999 is invented too, for a run GitHub will not answer for — deleted, or a read that failed.
EVENT_TABLE=' 41513010699=push 41513719101=pull_request 41522025592=push 41522631628=pull_request 41513010700=push 41500000001=workflow_dispatch '
# Counted through a file, not a variable: the lifted code runs its lookups inside subshells, which is
# the whole reason the prefetch exists, and a variable would not come back out of one.
calls=$(mktemp); trap 'rm -f "$calls"' EXIT
gh() { # the only call the lifted code makes is `gh api repos/$R/actions/runs/<id> --jq .event`
  local id e
  echo "$*" >> "$calls"
  case "$*" in "api repos/$R/actions/runs/"*" --jq .event") ;;
    *) echo "unexpected gh call: $*" >&2; return 1 ;; esac
  id=${2##*/}
  case "$EVENT_TABLE" in *" $id="*) e=${EVENT_TABLE#*" $id="}; printf '%s\n' "${e%% *}" ;;
    *) return 1 ;; esac
}
calls_made() { wc -l < "$calls" | tr -d ' '; }

# Recorded from `gh pr view 2569 --json statusCheckRollup` on 2026-11-02, head
# f4297c758f30acfbce5fbe4f83e3adab3cb106d0: `rust (fmt, clippy, test)` concluded failure in the push
# run 41513010699 and success in the pull_request run 41513719101, on that one sha. That is the safe
# direction, and the case this pass used to refuse. Timestamps are dropped; nothing reads them.
pr2569='{"statusCheckRollup":[
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

# Recorded the same way from #2592, head aa322cda006b92212da5dff369c29992ff3cfb34. Same check, the
# other way round: success in the push run 41522025592, failure in the pull_request run 41522631628.
# This is the direction that must never settle, and it is live — #2592 was red on this at the time.
pr2592='{"statusCheckRollup":[
 {"__typename":"CheckRun","conclusion":"SUCCESS","status":"COMPLETED","workflowName":"CI","name":"Layout",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41522631628/job/125542248378"},
 {"__typename":"CheckRun","conclusion":"SUCCESS","status":"COMPLETED","workflowName":"CI","name":"Layout",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41522025592/job/125540195695"},
 {"__typename":"CheckRun","conclusion":"FAILURE","status":"COMPLETED","workflowName":"CI","name":"rust (fmt, clippy, test)",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41522631628/job/125542248752"},
 {"__typename":"CheckRun","conclusion":"SUCCESS","status":"COMPLETED","workflowName":"CI","name":"rust (fmt, clippy, test)",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41522025592/job/125540196379"},
 {"__typename":"CheckRun","conclusion":"SUCCESS","status":"COMPLETED","workflowName":"CI","name":"harness (arbos-pi-ext typecheck + build + test)",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41522631628/job/125542248983"},
 {"__typename":"CheckRun","conclusion":"SUCCESS","status":"COMPLETED","workflowName":"CI","name":"harness (arbos-pi-ext typecheck + build + test)",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41522025592/job/125540196096"},
 {"__typename":"CheckRun","conclusion":"SKIPPED","status":"COMPLETED","workflowName":"CI","name":"image (dockerfile lint)",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41522631628/job/125545196084"},
 {"__typename":"CheckRun","conclusion":"SUCCESS","status":"COMPLETED","workflowName":"CI","name":"image (dockerfile lint)",
  "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41522025592/job/125543360653"}
]}'

# The one green row most cases below edit, and the red row they all expect back.
GREEN='.name == "rust (fmt, clippy, test)" and .conclusion == "SUCCESS"'
RUST='rust (fmt, clippy, test) FAILURE'

fails=0
report() { # report <ok|fail> <what> <got> <want>
  if [ "$1" = ok ]; then echo "ok   $2"; return; fi
  echo "FAIL $2"; echo "       got:  ${3:-<nothing>}"; echo "       want: ${4:-<nothing>}"; fails=$((fails + 1))
}

## rollup_red — which rows are red, and which green rows are offered as candidates for them.

rows() { # rows <what> <recorded rollup> <jq edit of it> <expected lines>
  local got
  if ! got=$(printf '%s' "$2" | jq -c "$3" | rollup_red); then
    report fail "$1" "the fixture or rollup_red did not parse" "$4"; return
  fi
  if [ "$got" = "$4" ]; then report ok "$1"; else report fail "$1" "$got" "$4"; fi
}

rows "#2569 as recorded: the red row offers its green candidate" "$pr2569" '.' \
  "41513010699 41513719101 $RUST"
rows "#2592 as recorded: the red row offers its green candidate too" "$pr2592" '.' \
  "41522631628 41522025592 $RUST"
rows "no candidate once the green row is gone" "$pr2569" \
  "(.statusCheckRollup |= map(select($GREEN | not)))" \
  "41513010699 - $RUST"
# A candidate is the same check, not a similar one. Were the name ignored, any green row anywhere in
# the rollup would be offered for every red row in it.
rows "a green row under another name is not a candidate" "$pr2569" \
  "(.statusCheckRollup |= map(if $GREEN then .name += \" (rerun)\" else . end))" \
  "41513010699 - $RUST"
rows "the same name from another workflow is not a candidate" "$pr2569" \
  "(.statusCheckRollup |= map(if $GREEN then .workflowName = \"CI (nightly)\" else . end))" \
  "41513010699 - $RUST"
# skipped and neutral are the trap checks.sh exists to name: a job that did no work is not a verdict,
# so it cannot be the green run rule 9 means.
rows "a skipped row of the same check is not a candidate" "$pr2569" \
  "(.statusCheckRollup |= map(if $GREEN then .conclusion = \"SKIPPED\" else . end))" \
  "41513010699 - $RUST"
rows "a neutral row of the same check is not a candidate" "$pr2569" \
  "(.statusCheckRollup |= map(if $GREEN then .conclusion = \"NEUTRAL\" else . end))" \
  "41513010699 - $RUST"
rows "a still-running row of the same check is not a candidate" "$pr2569" \
  "(.statusCheckRollup |= map(if $GREEN then (.conclusion = null | .status = \"IN_PROGRESS\") else . end))" \
  "41513010699 - $RUST"
rows "every other conclusion this pass calls red is judged the same way" "$pr2569" \
  "(.statusCheckRollup |= map(if .conclusion == \"FAILURE\" then .conclusion = \"TIMED_OUT\" else . end))" \
  "41513010699 41513719101 rust (fmt, clippy, test) TIMED_OUT"
rows "several green rows for one check are all offered, in rollup order" "$pr2569" \
  "(.statusCheckRollup += [(.statusCheckRollup[] | select($GREEN) | .detailsUrl = \"https://github.com/unarbos/arbos/actions/runs/41513010700/job/1\")])" \
  "41513010699 41513719101,41513010700 $RUST"
rows "a red row whose url carries no run id is untraceable" "$pr2569" \
  "(.statusCheckRollup |= map(if .conclusion == \"FAILURE\" then .detailsUrl = \"https://example.invalid/build/7\" else . end))" \
  "untraceable 41513719101 $RUST"
rows "an all-green rollup says nothing" "$pr2569" \
  '(.statusCheckRollup |= map(.conclusion = "SUCCESS"))' \
  ""
# A commit status has no run, so nothing can say what it tested. It is neither offered as a candidate
# nor rankable as a red row.
rows "a commit status green is not offered as a candidate" "$pr2569" \
  '.statusCheckRollup = [
     {"__typename":"StatusContext","state":"FAILURE","context":"buildkite/arbos","targetUrl":"https://buildkite.test/a"},
     {"__typename":"StatusContext","state":"SUCCESS","context":"buildkite/arbos","targetUrl":"https://buildkite.test/b"}]' \
  "untraceable - buildkite/arbos FAILURE"
rows "a check run and a commit status sharing a name are not the same check" "$pr2569" \
  '.statusCheckRollup = [
     {"__typename":"CheckRun","conclusion":"FAILURE","status":"COMPLETED","workflowName":"CI","name":"build",
      "detailsUrl":"https://github.com/unarbos/arbos/actions/runs/41513010699/job/125509341752"},
     {"__typename":"StatusContext","state":"SUCCESS","context":"build","targetUrl":"https://buildkite.test/b"}]' \
  "41513010699 - build FAILURE"

## green_twin — the direction. This is the rule, not a refinement of it.

twin() { # twin <what> <red run id> <candidate ids> <expected "<id> <event>", or empty for none>
  local got=""
  EVENTS=""
  got=$(green_twin "$2" "$3") || got=""
  if [ "$got" = "$4" ]; then report ok "$1"; else report fail "$1" "$got" "${4:-no twin}"; fi
}

twin "a pull_request green settles a push red" 41513010699 41513719101 "41513719101 pull_request"
twin "a push green NEVER settles a pull_request red" 41522631628 41522025592 ""
twin "a push green settles a push red: the same strictness is enough" 41513010699 41513010700 "41513010700 push"
twin "a pull_request green settles a pull_request red" 41522631628 41513719101 "41513719101 pull_request"
twin "a pull_request red looks past a push candidate to the one that will do" \
  41522631628 "41522025592,41513719101" "41513719101 pull_request"
twin "a red row with no run cannot be ranked, so nothing settles it" untraceable 41513719101 ""
twin "a red row whose run cannot be read settles nothing" 41599999999 41513719101 ""
twin "a green whose run cannot be read settles nothing" 41513010699 41599999999 ""
twin "an event that is neither push nor pull_request settles nothing" 41513010699 41500000001 ""
twin "no candidates, no twin" 41513010699 - ""

EVENTS=""; : > "$calls"
run_event 41513010699 >/dev/null; run_event 41513010699 >/dev/null; run_event 41513719101 >/dev/null
run_event untraceable >/dev/null; run_event - >/dev/null
if [ "$(calls_made)" = 2 ]; then report ok "a run's event is read once, and a non-run is not read at all"
else report fail "a run's event is read once, and a non-run is not read at all" "$(calls_made) gh calls" "2 gh calls"; fi

if [ "$(tested_rank pull_request)" -gt "$(tested_rank push)" ] && [ "$(tested_rank merge_group)" -eq 0 ]; then
  report ok "a pull_request run outranks a push run, and an unknown event ranks nothing"
else
  report fail "a pull_request run outranks a push run, and an unknown event ranks nothing" \
    "push=$(tested_rank push) pull_request=$(tested_rank pull_request) merge_group=$(tested_rank merge_group)" \
    "push < pull_request, merge_group=0"
fi

## And the decision merge.sh makes from those lines, which is the behaviour that changed.

# shellcheck disable=SC2034  # red, acc and SHA are merge.sh's names, read by the lifted loop.
decide() { # decide <rollup_red lines> <accept-red ids> <head sha>
  ( set +e
    EVENTS=""; red=$1; acc=",$2,"; SHA=$3
    eval "$loop" )
}

verdict() { # verdict <what> <rollup_red lines> <accept-red ids> <head sha> <expected status> <expected output>
  local out rc=0
  out=$(decide "$2" "$3" "$4") || rc=$?
  if [ "$rc" = "$5" ] && [ "$out" = "$6" ]; then report ok "$1"; return; fi
  report fail "$1" "exit $rc: $out" "exit $5: $6"
}

verdict "#2569's red row lets the merge go on, and says which run settled it" \
  "41513010699 41513719101 $RUST" "" f4297c758f30acfbce5fbe4f83e3adab3cb106d0 0 \
  "    red check settled by run 41513719101 (pull_request), green for this same check on f4297c758 and at least as merged as this push run: 41513010699 $RUST"
verdict "#2592's red row still refuses, and names the green run it would not take" \
  "41522631628 41522025592 $RUST" "" aa322cda006b92212da5dff369c29992ff3cfb34 3 \
  "    a green run of this same check exists on aa322cda0 (run 41522025592), but it cannot be shown to have tested at least as much of the merged result as run 41522631628 did, so it does not settle this row
NOT MERGED: red check not accepted: run 41522631628 $RUST"
verdict "a red row with no green anywhere still refuses, as it always did" \
  "41513010699 - $RUST" "" f4297c758f30acfbce5fbe4f83e3adab3cb106d0 3 \
  "NOT MERGED: red check not accepted: run 41513010699 $RUST"
verdict "accept-red still works, and is now all it is for" \
  "41513010699 - $RUST" 41513010699 f4297c758f30acfbce5fbe4f83e3adab3cb106d0 0 \
  "    red check accepted by run id: 41513010699 $RUST"
verdict "no red rows, nothing to say" "" "" f4297c758f30acfbce5fbe4f83e3adab3cb106d0 0 ""

# Four red rows between two runs cost two reads, not eight. Without the prefetch each lookup runs in
# its own subshell and the cache dies there, so this is what holds it in place.
: > "$calls"
decide "41513010699 41513719101 a FAILURE
41513010699 41513719101 b FAILURE
41513010699 41513719101 c FAILURE
41513010699 41513719101 d FAILURE" "" f4297c758f30acfbce5fbe4f83e3adab3cb106d0 >/dev/null
if [ "$(calls_made)" = 2 ]; then report ok "four red rows between two runs cost two reads"
else report fail "four red rows between two runs cost two reads" "$(calls_made) gh calls" "2 gh calls"; fi

[ "$fails" -eq 0 ] || { echo "$fails failed"; exit 1; }
echo "all ok"

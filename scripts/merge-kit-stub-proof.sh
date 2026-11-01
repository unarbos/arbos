#!/bin/bash
# merge-kit-stub-proof.sh <merge-window.sh> [--misplace]
#
# The readings the merge kit's gate must make that no real head on this repository can produce. Two
# families, through one seam, in one file: this is the only copy of this harness, and a second one
# living anywhere else is the bug it exists to avoid.
#
#   BLOCKED branches — a short run read, an unevaluable path pattern, an unreadable path filter and,
#   where range_gate exists, an unreadable commit. Each must refuse rather than answer.
#
#   Job versus run — a run that concluded `success` while the gate JOB skipped, succeeded, or was
#   absent. Only the middle one is a pass.
#
# Why the gate reads the JOB. Not because an always-green job could fake a pass: under #2448 it
# cannot, since GitHub fails a run when any job fails and that workflow's `verdict` job is
# `if: always()` and fails closed. The reason is that a run's conclusion is an aggregate over the
# run's whole job graph, so it does not answer "did the gate test this head", and the kit should not
# have to model a workflow's job graph to find out. The divergence is measured, not supposed: in run
# 41247933853 on #2448's head, `route` concluded success while the gate job was skipped. What no real
# head gives us is that divergence with the run concluding `success`, because `verdict` currently
# takes such a run red. Hence a table.
#
# Why every case is paired. A stub that fails to intercept makes a real API read, and a real read
# that happens to produce the expected answer reports a pass having tested nothing. That is not
# hypothetical: #2495's throwaway harness padded a short sha into an invented one, so two legs
# blocked for the wrong reason and both looked green. So:
#
#   * Every stub appends to $STUB_MARK when it intercepts. A case with an empty marker FAILS, however
#     right its output looks.
#   * The suite carries legs whose expected answer is NOT blocked and NOT a refusal. An all-BLOCKED
#     suite cannot tell whether anything was exercised.
#   * The job legs change exactly ONE field between them — the job table — and must diverge.
#   * The stub's run id, 9001, cannot appear in any real read of this repository.
#   * Every sha is read back from the API, never typed or padded. A sha that does not resolve blocks
#     for its own reason, which is indistinguishable from a stub working.
#
# The stubs are injected on the line straight after that file's own `source …checks.sh` line, so the
# seam follows whatever path form the file uses ("$D/checks.sh" on main, /tmp/k/checks.sh in the live
# kit). The stubbed copy is written next to the original, so "$D" resolves to the same directory.
#
# --misplace injects the stubs BEFORE the source line instead, where the library overwrites them. It
# is the harness's own negative control: every case must then FAIL on the marker check, including the
# ones whose expected text a real read could produce. A harness that still scores a pass under
# --misplace is measuring nothing.
#
# Read only: gh reads, no vault read, no merge call. Run from a checkout of unarbos/arbos.
set -u
MW=${1:?usage: merge-kit-stub-proof.sh <merge-window.sh> [--misplace]}
MODE=${2:-}
DIR=$(cd "$(dirname "$MW")" && pwd)
CONTROL_PR=2405   # its head passes the gate when nothing is stubbed
CONTROL_SHA=$(gh api "repos/unarbos/arbos/pulls/$CONTROL_PR" --jq .head.sha 2>/dev/null)
RANGE_HEAD=$(gh api repos/unarbos/arbos/commits/993cba526 --jq .sha 2>/dev/null)   # range-gate replay head: decided on a green gate
for v in CONTROL_SHA RANGE_HEAD; do
  [ "$(printf '%s' "${!v}" | grep -cE '^[0-9a-f]{40}$')" -eq 1 ] \
    || { echo "HARNESS ERROR: $v did not read back from the API as a 40-character sha: '${!v}'"; exit 3; }
done
echo "shas from the API: control #$CONTROL_PR head $CONTROL_SHA, range head $RANGE_HEAD"
JOB=$(grep -m1 '^GATES=' "$DIR/checks.sh" | sed "s/^GATES='//; s/'$//" | cut -d'|' -f3)
[ -n "$JOB" ] || { echo "HARNESS ERROR: no gate job name in GATES in $DIR/checks.sh"; exit 3; }
echo "gate job from GATES: '$JOB'"
# The job name contains '+', which an expected-output pattern would otherwise read as a quantifier
# and then fail to find text that is plainly there.
JOB_RE=$(printf '%s' "$JOB" | sed 's/[][\.^$*+?(){}|\\]/\\&/g')
n=$(grep -cE '^source .*checks\.sh"?[[:space:]]*$' "$MW")
[ "$n" -eq 1 ] || { echo "HARNESS ERROR: expected exactly one 'source …checks.sh' line in $MW, found $n"; exit 3; }
line=$(grep -nE '^source .*checks\.sh"?[[:space:]]*$' "$MW" | cut -d: -f1)
echo "kit: $MW"
echo "library line $line: $(sed -n "${line}p" "$MW")"
[ "$MODE" = --misplace ] && { line=$((line - 1)); echo "NEGATIVE CONTROL: stubs injected before the library line, where it overwrites them"; }

# Shared prologue: keep the real rd as rd_real, so a stub can pass through every call it does not own.
prologue='eval "rd_real$(declare -f rd | sed "1s/^rd //")"
stub_hit() { echo "$1" >> "$STUB_MARK"; }'

stub_short_read='rd() { case "$*" in *"/runs?head_sha="*"per_page=1 "*|*"/runs?head_sha="*"per_page=1") stub_hit short-read; echo 999; return 0 ;; esac; rd_real "$@"; }'
yaml_bad=$(printf 'on:\n  push:\n    paths:\n      - "apps/ios/**/Gen*.swift"\n' | base64 -w0)
stub_pattern="rd() { case \"\$*\" in *\"/contents/.github/workflows/\"*) stub_hit unevaluable-pattern; echo $yaml_bad; return 0 ;; esac; rd_real \"\$@\"; }"
stub_filter='rd() { case "$*" in *"/contents/.github/workflows/"*) stub_hit unreadable-filter; echo "BLOCKED:stubbed: filter unreadable"; return 1 ;; esac; rd_real "$@"; }'
stub_commit='rd() { case "$*" in *"/commits/"*) stub_hit unreadable-commit; echo "BLOCKED:stubbed: commit unreadable"; return 1 ;; esac; rd_real "$@"; }'
stub_passthrough='rd() { stub_hit passthrough; rd_real "$@"; }'

# One run at the head, concluding success, with the job table handed in through the environment so
# the three job legs differ in that table and in nothing else. per_page=100 is matched before
# per_page=1, which is a prefix of it, so the narrower pattern cannot answer the wider read. Every
# other read — the commit, the PR files, the path filter — goes to the real API.
stub_jobs='rd() { case "$*" in
  *"/runs?head_sha="*"per_page=100"*)  stub_hit runs-list;  printf "%s\n" "9001 pull_request completed success"; return 0 ;;
  *"/runs?head_sha="*"per_page=1"*)    stub_hit runs-total; printf "%s" 1; return 0 ;;
  *"/runs/9001/jobs?per_page=100"*)    stub_hit jobs-list;  printf "%s\n" "$STUB_JOBS"; return 0 ;;
  *"/runs/9001/jobs?per_page=1"*)      stub_hit jobs-total; printf "%s" "$STUB_JOBS_TOTAL"; return 0 ;;
  esac; rd_real "$@"; }'

pass=0; fail=0
declare -A OUT=()
CASE_ENV=()
# Legs whose expected text appeared while the stub intercepted nothing: a real read produced it, and
# without the marker check each one would have been scored a pass. Named in the summary, because this
# is the number that says what the marker check is worth.
WOULD_FALSE_PASS=()
run_case() {  # [WANT_RC=n] [CASE_ENV=(V=x …)] <name> <stub> <expected regex> <subcommand args…>
  local name=$1 stub=$2 want=$3 want_rc=${WANT_RC:-2}; shift 3
  local copy mark out rc hits
  copy="$DIR/.stubproof-$name-$$.sh"; mark=$(mktemp)
  { head -n "$line" "$MW"; printf '%s\n%s\n' "$prologue" "$stub"; tail -n +"$((line + 1))" "$MW"; } > "$copy"
  out=$(env STUB_MARK="$mark" "${CASE_ENV[@]}" bash "$copy" "$@" 2>&1); rc=$?
  hits=$(grep -c . "$mark")
  rm -f "$copy" "$mark"
  OUT[$name]=$out
  echo
  echo "== $name: merge-window.sh $*"
  echo "$out" | sed 's/^/    /'
  if [ "$hits" -gt 0 ] && [ "$rc" -eq "$want_rc" ] && echo "$out" | grep -qE "$want"; then
    echo "  PASS: exit $rc, expected line present, stub intercepted $hits call(s)"; pass=$((pass + 1))
  else
    echo "  FAIL: exit $rc (want $want_rc), stub intercepted $hits call(s), expected /$want/ $(echo "$out" | grep -qE "$want" && echo present || echo absent)"; fail=$((fail + 1))
    [ "$hits" -eq 0 ] && echo "$out" | grep -qE "$want" && WOULD_FALSE_PASS+=("$name")
  fi
}

assert() {  # <label> <condition-result> <detail>
  if [ "$2" -eq 0 ]; then pass=$((pass + 1)); echo "  PASS: $1"
  else fail=$((fail + 1)); echo "  FAIL: $1 — $3"; fi
}

echo
echo "### BLOCKED branches: a gate that cannot be read must refuse, not answer"
WANT_RC=0 run_case not-blocked-control "$stub_passthrough" 'RESULT: SATISFIED' checks "$CONTROL_SHA" "$CONTROL_PR"
run_case short-read "$stub_short_read" 'BLOCKED .*run lookup incomplete.*of 999' checks "$CONTROL_SHA" "$CONTROL_PR"
run_case unevaluable-pattern "$stub_pattern" 'BLOCKED — .*path pattern this cannot evaluate' checks "$CONTROL_SHA" "$CONTROL_PR"
run_case unreadable-filter "$stub_filter" 'BLOCKED — cannot read the path filter' checks "$CONTROL_SHA" "$CONTROL_PR"
if grep -q '^range_gate()' "$MW"; then
  CASE_ENV=("MW_TEST_HEAD=$RANGE_HEAD")
  WANT_RC=0 run_case not-blocked-range-control "$stub_passthrough" 'DECIDED on [0-9a-f]{9} .*: PASS run [0-9]+' range-gate v0.6.375
  run_case unreadable-commit "$stub_commit" 'BLOCKED — cannot read the files it changed.*which commit decides this gate is unknown' range-gate v0.6.375
  CASE_ENV=()
else
  echo; echo "== unreadable-commit: skipped, this kit has no range_gate"
fi

echo
echo "### Job versus run: one run concluding success, three job tables, one pass"
CASE_ENV=("STUB_JOBS=$(printf 'route\tsuccess\n%s\tskipped' "$JOB")" STUB_JOBS_TOTAL=2)
WANT_RC=1 run_case job-skipped "$stub_jobs" "run 9001 .*concluded success but '$JOB_RE' was skipped" checks "$CONTROL_SHA" "$CONTROL_PR"
CASE_ENV=("STUB_JOBS=$(printf 'route\tsuccess\n%s\tsuccess' "$JOB")" STUB_JOBS_TOTAL=2)
WANT_RC=0 run_case job-green "$stub_jobs" "PASS run 9001 \(pull_request\), job '$JOB_RE' success" checks "$CONTROL_SHA" "$CONTROL_PR"
CASE_ENV=("STUB_JOBS=$(printf 'route\tsuccess')" STUB_JOBS_TOTAL=1)
WANT_RC=1 run_case job-absent "$stub_jobs" "concluded success with no job named '$JOB_RE'" checks "$CONTROL_SHA" "$CONTROL_PR"
CASE_ENV=()

echo
echo "### Pairing: the job table is the only field that moved"
if [ -n "${OUT[job-green]:-}" ] && [ -n "${OUT[job-skipped]:-}" ]; then
  # Both readings must name the stub's run, or they are two real reads of the same head that differ
  # for some reason of their own, and their divergence says nothing about the job table.
  printf '%s' "${OUT[job-green]}" | grep -q 'run 9001' \
    && printf '%s' "${OUT[job-skipped]}" | grep -q 'run 9001' \
    && [ "${OUT[job-skipped]}" != "${OUT[job-green]}" ]
  assert "skipped and green diverge, both of them on the stub's run 9001" $? "both read: ${OUT[job-green]}"
  printf '%s' "${OUT[job-skipped]}" | grep -q 'RESULT: REFUSE'
  assert "a skipped gate job refuses the head" $? "job-skipped read: ${OUT[job-skipped]}"
  printf '%s' "${OUT[job-absent]:-}" | grep -q 'RESULT: REFUSE'
  assert "an absent gate job refuses the head, and says absent rather than failed" $? "job-absent read: ${OUT[job-absent]:-}"
else
  fail=$((fail + 1)); echo "  FAIL: the job legs produced no output to compare"
fi

echo
echo "RESULT: $pass passed, $fail failed"
[ "${#WOULD_FALSE_PASS[@]}" -gt 0 ] \
  && echo "caught by the marker check alone (expected text, stub never called): ${WOULD_FALSE_PASS[*]}"
[ "$MODE" = --misplace ] && [ "$fail" -eq 0 ] && { echo "NEGATIVE CONTROL BROKEN: nothing failed with the stubs overwritten, so this harness proves nothing"; exit 1; }
[ "$MODE" = --misplace ] && { echo "negative control held: with the stubs overwritten, $fail case(s) failed and $pass passed"; exit 0; }
[ "$fail" -eq 0 ]

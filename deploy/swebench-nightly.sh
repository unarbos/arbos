#!/usr/bin/env bash
# Nightly SWE-bench Verified slice: the same 16 instances as the reference
# run (media/swebench/2026-09-13/results.json), Arbos as a verifiers Harness,
# kernel built from the integration head, budget cap per run.
#
#   swebench-nightly.sh [--smoke]     --smoke: one cheap instance to check the pipeline
#
# Layout under $ROOT/swebench: repo/ (worktree: integration head + harness/),
# .venv/ (verifiers, prime, harness), swebench_verified/ (the env), runs/<stamp>/.
# Output: $ROOT/loop/swebench-history.jsonl (one line per run), failing bundles
# copied to $STORE/rollouts/swebench/<instance>/, regressions as bug drafts.
set -uo pipefail
ROOT="${ARBOS_QA_ROOT:-$HOME/arbos-qa}"
STORE="${ARBOS_QA_STORE:-/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa}"
SB="$ROOT/swebench"
BRANCH="${ARBOS_QA_SWEBENCH_BRANCH:-cursor/release-integration-52cd}"
HARNESS_BRANCH="${ARBOS_QA_SWEBENCH_HARNESS_BRANCH:-cursor/swebench-harness}"
MODEL="${ARBOS_QA_SWEBENCH_MODEL:-anthropic/claude-sonnet-5}"
BUDGET="${ARBOS_QA_SWEBENCH_BUDGET_USD:-10}"
TIMEOUT="${ARBOS_QA_SWEBENCH_TIMEOUT_S:-900}"
REF="${ARBOS_QA_SWEBENCH_REF:-/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/media/swebench/2026-09-13/results.json}"
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
RUN="$SB/runs/$STAMP"
mkdir -p "$RUN" "$ROOT/loop"
LOG="$RUN/nightly.log"
exec > >(tee -a "$LOG") 2>&1
echo "== swebench nightly $STAMP branch=$BRANCH model=$MODEL budget=\$$BUDGET"

set -a; . "$ROOT/secrets.env"; set +a
export PATH="$HOME/.local/bin:$PATH"
DOCKER="docker"; docker info >/dev/null 2>&1 || DOCKER="sudo docker"

# 1. kernel from the integration head, harness from its branch until it merges
cd "$ROOT/repo" && git fetch -q origin "$BRANCH" "$HARNESS_BRANCH"
if [ ! -d "$SB/repo/.git" ] && [ ! -f "$SB/repo/.git" ]; then
  git worktree add -q --detach "$SB/repo" "origin/$BRANCH"
else
  git -C "$SB/repo" checkout -q -- . 2>/dev/null; git -C "$SB/repo" checkout -q --detach "origin/$BRANCH"
fi
cd "$SB/repo"
[ -d harness ] || git checkout -q "origin/$HARNESS_BRANCH" -- harness
SHA=$(git rev-parse --short=12 HEAD)
echo "-- kernel sha $SHA"
if [ "$(cat "$SB/image-sha" 2>/dev/null)" != "$SHA" ]; then
  echo "-- building arbos-harness image"
  $DOCKER build -q -f harness/Dockerfile -t arbos-harness . > "$RUN/image-build.log" 2>&1 || { echo "!! image build failed"; tail -5 "$RUN/image-build.log"; exit 1; }
  echo "$SHA" > "$SB/image-sha"
fi

# 2. the instance set
cd "$SB"; source .venv/bin/activate
if [ "${1:-}" = "--smoke" ]; then
  TASKS=(astropy__astropy-12907); MODEL="${ARBOS_QA_SWEBENCH_SMOKE_MODEL:-deepseek/deepseek-v4.1-flash}"
else
  mapfile -t TASKS < <(python3 -c "import json,sys; print('\n'.join(sorted(r['instance'] for r in json.load(open(sys.argv[1]))['rows'])))" "$REF")
fi
echo "-- ${#TASKS[@]} instance(s)"

# 3. run in two halves; stop before the second when half the budget is gone
run_half() {
  local name="$1"; shift
  local tasks=("$@")
  [ ${#tasks[@]} -eq 0 ] && return 0
  local args=(); for t in "${tasks[@]}"; do args+=(--env.taskset.tasks "$t"); done
  timeout 3h "$SB/.venv/bin/eval" swebench-verified \
    --env.agent.harness.id arbos-harness --env.agent.harness.timeout "$TIMEOUT" \
    --env.agent.harness.artifacts "$RUN/artifacts" \
    --env.agent.runtime.type docker \
    "${args[@]}" -n "${#tasks[@]}" \
    -m "$MODEL" --client.base-url https://openrouter.ai/api/v1 --client.api-key-var OPENROUTER_API_KEY \
    --no-push -o "$RUN/outputs" --run.dir "$name" --rich.show-logs false \
    > "$RUN/eval-$name.log" 2>&1 || echo "-- eval $name exit $?"
}
half=$(( (${#TASKS[@]} + 1) / 2 ))
run_half a "${TASKS[@]:0:$half}"
spent=$(python3 "$ROOT/deploy/swebench-collect.py" --cost-only "$RUN/outputs/a" 2>/dev/null || echo 0)
echo "-- after half a: \$$spent"
if python3 -c "import sys; sys.exit(0 if float('$spent') < float('$BUDGET')/2 else 1)"; then
  run_half b "${TASKS[@]:$half}"
else
  echo "!! half a spent \$$spent >= half the budget; second half skipped"
fi

# 4. collect, diff against the previous run, publish
python3 "$ROOT/deploy/swebench-collect.py" "$RUN" "$SHA" "$BRANCH" "$MODEL" "$ROOT/loop/swebench-history.jsonl" "$STORE/rollouts/swebench" "$STORE/bugs"
cp -f "$ROOT/loop/swebench-history.jsonl" "$STORE/vm-swebench-history.jsonl" 2>/dev/null || true
echo "== swebench nightly end $(date -u +%FT%TZ)"

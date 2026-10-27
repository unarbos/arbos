#!/usr/bin/env bash
# Long-task agent eval (D94): one command, end to end.
#   preflight → run every task through the product path → grade → scoreboard → report → issues
#
# Usage: scripts/evals/long-tasks/run-suite.sh [--tasks a,b] [--lanes a,b] [--issues] [--budget-usd 8] [--run-id ID]
# Needs: OP_SERVICE_ACCOUNT_TOKEN (vault "Arbos"), a built arbosctl (target/release/arbosctl or $ARBOSCTL),
#        ssh access to the relay host as const with the Cloud Agents key (LT_HOST_SSH_KEY, default ~/.ssh/arbos_cloud).
# Secrets are read into the environment only; nothing is written to the repo.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
EVAL="$ROOT/evals/long-tasks"
STORE="${LT_STORE:-/cursor/stores/bc-055bfdfb-336d-43af-bd4b-a794e153944c}"
RUN_ID="$(date -u +%Y%m%dT%H%M)"
# Failures are triaged by hand (one root cause, one issue); the auto-filer is opt-in (--issues) — run 6 filed 37 noise issues when it was the default.
FILE_ISSUES=0
ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --no-issues) FILE_ISSUES=0; shift ;;
    --issues) FILE_ISSUES=1; shift ;;
    --run-id) RUN_ID="$2"; shift 2 ;;
    *) ARGS+=("$1"); shift ;;
  esac
done

export ARBOSCTL="${ARBOSCTL:-$ROOT/target/release/arbosctl}"
export LT_STATE_DIR="${LT_STATE_DIR:-/tmp/lt}"
mkdir -p "$LT_STATE_DIR"
if [ ! -x "$ARBOSCTL" ]; then
  echo "building arbosctl…"; (cd "$ROOT" && cargo build --release -p arbosctl -q)
fi
if [ -z "${ARBOS_RELAY_ADMIN_TOKEN:-}" ]; then
  export ARBOS_RELAY_ADMIN_TOKEN="$(op item get pjwsl7tjrhcbzbqos2masde2u4 --vault Arbos --fields credential --reveal)"
fi
if [ -z "${OPENROUTER_API_KEY:-}" ]; then
  export OPENROUTER_API_KEY="$(op item get hy3omqs2ak2tkjmzji2uerbmya --vault Arbos --fields notesPlain --reveal | tr -d '"\n')"
fi
if [ -z "${ARBOS_GITHUB:-}" ] && [ "$FILE_ISSUES" = 1 ]; then
  export ARBOS_GITHUB="$(op item get gefompyaklwbv2iuyyb4z5rvbm --vault Arbos --format json | python3 -c 'import json,sys; d=json.load(sys.stdin); print(next(f["value"] for f in d["fields"] if f.get("value") and f.get("type")=="CONCEALED"))')"
fi
if [ ! -f "${LT_HOST_SSH_KEY:-$HOME/.ssh/arbos_cloud}" ]; then
  mkdir -p "$HOME/.ssh"; chmod 700 "$HOME/.ssh"
  op item get nlijfp36ed4aqbkp2svh2lefbi --vault Arbos --fields "private key" --reveal | sed 's/^"//;s/"$//' | sed '/^$/d' > "$HOME/.ssh/arbos_cloud"
  chmod 600 "$HOME/.ssh/arbos_cloud"
fi

# web fixtures: main once the suite has landed, otherwise the branch we are on
if [ -z "${LT_FIXTURE_BASE:-}" ]; then
  if curl -sfo /dev/null https://raw.githubusercontent.com/unarbos/arbos/main/evals/long-tasks/fixtures/injection-page.html; then
    export LT_FIXTURE_BASE="https://raw.githubusercontent.com/unarbos/arbos/main/evals/long-tasks/fixtures"
  else
    BR="$(git -C "$ROOT" rev-parse --abbrev-ref HEAD)"
    export LT_FIXTURE_BASE="https://raw.githubusercontent.com/unarbos/arbos/$BR/evals/long-tasks/fixtures"
  fi
fi

echo "== run $RUN_ID"
python3 "$EVAL/sweep.py" || true   # no routine from an earlier run may still be spending
python3 "$EVAL/run.py" --run-id "$RUN_ID" "${ARGS[@]}"
python3 "$EVAL/sweep.py" || true   # and none from this one

ISSUES_JSON="$EVAL/results/$RUN_ID/issues.json"
if [ "$FILE_ISSUES" = 1 ]; then
  python3 - "$EVAL" "$RUN_ID" "$ISSUES_JSON" <<'PY'
import json, sys
sys.path.insert(0, sys.argv[1])
from lib import issues
board = json.load(open(f"{sys.argv[1]}/scoreboard/{sys.argv[2]}.json"))
out = issues.file_failures(board)
json.dump(out, open(sys.argv[3], "w"), indent=1)
for t, u in out:
    print(f"  {t}: {u}")
PY
fi

DATE="$(date -u +%Y-%m-%d)"
REPORT="$EVAL/scoreboard/$RUN_ID.md"
python3 "$EVAL/report.py" --run "$RUN_ID" --issues "$ISSUES_JSON" --report-out "$REPORT" ${LT_FIXED_TEXT:+--fixed "$LT_FIXED_TEXT"}
if [ -d "$STORE/docs" ]; then
  mkdir -p "$STORE/docs/agent-eval-runs"
  cp "$REPORT" "$STORE/docs/agent-eval-runs/$DATE-$RUN_ID.md"
  echo "report: $STORE/docs/agent-eval-runs/$DATE-$RUN_ID.md"
fi
echo "scoreboard: $EVAL/scoreboard/$RUN_ID.json  summary: $EVAL/scoreboard/SUMMARY.md"

# Version the scoreboard: a fresh feedback/<id> branch off origin/main with only the scoreboard
# files, pushed and registered with the loop (which gates and merges it). LT_COMMIT=0 skips it.
if [ "${LT_COMMIT:-1}" = 1 ] && [ -n "${ARBOS_GITHUB:-}" ]; then
  FID="long-task-eval-run-$RUN_ID"
  REMOTE="https://x-access-token:${ARBOS_GITHUB}@github.com/unarbos/arbos.git"
  (
    cd "$ROOT"
    git fetch -q "$REMOTE" main
    # SUMMARY.md is regenerated from every scoreboard, so a locally modified copy must not block
    # the branch switch (it did once: "commit your changes or stash them"); keep the run's files aside
    TMPSB="$(mktemp -d)"
    cp "$EVAL/scoreboard/$RUN_ID".* "$TMPSB"/
    git checkout -q -- evals/long-tasks/scoreboard/SUMMARY.md 2>/dev/null || true
    git checkout -q -B "feedback/$FID" FETCH_HEAD
    cp "$TMPSB"/* "$EVAL/scoreboard/"
    python3 "$EVAL/report.py" --run "$RUN_ID" --report-out "$TMPSB/report.md" >/dev/null
    git add evals/long-tasks/scoreboard
    if git -c user.name="Cursor agent (bc-dcc557ae)" -c user.email="agents@arbos.life" commit -qm "evals(long-tasks): run $RUN_ID scoreboard [feedback $FID]"; then
      git push -q "$REMOTE" "feedback/$FID"
      PASS_LINE="$(python3 "$EVAL/report.py" --run "$RUN_ID" --one-line)"
      python3 - "$EVAL" "$FID" "$RUN_ID" "$PASS_LINE" <<'PY2'
import sys
sys.path.insert(0, sys.argv[1])
from lib import issues as iss
fid, run_id, line = sys.argv[2], sys.argv[3], sys.argv[4]
st, v = iss.gh("POST", f"/repos/{iss.REPO}/issues", {"title": f"[feedback {fid}] Long-task eval run {run_id}: {line}", "body": f"Scoreboard commit for run `{run_id}` (`evals/long-tasks/scoreboard/{run_id}.json`, `SUMMARY.md`): {line}. Report: project store `docs/agent-eval-runs/`. Failures have their own root-cause issues (label `long-task-eval`).", "labels": ["long-task-eval", "feedback"]})
if st == 201:
    iss.gh("POST", f"/repos/{iss.REPO}/issues/{v['number']}/comments", {"body": f"track {fid} {iss.OWNER_AGENT}"})
    print("scoreboard issue:", v["html_url"])
else:
    print("scoreboard issue failed:", st, v)
PY2
    else
      echo "nothing new to commit"
    fi
    git checkout -q --detach FETCH_HEAD
  )
fi

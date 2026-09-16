#!/usr/bin/env bash
# The store mirror on its own clock: every INTERVAL seconds (default 15 min), independent of the QA cycle.
# Each pass: run internal/mirror-docs.sh (a non-zero exit is the alarm → mirror-alarm.py), then diff the
# branch's previous head against the new one and record every file that vanished from the store since the
# last pass in <loop>/store-mirror-losses.jsonl, with a staged copy under <root>/state/mirror-restore/<ts>/
# ready to put back. Recorded as "vanished, staged, not restored": the diff catches deliberate deletions and
# moves as well as losses, so the author says which it was; nothing is restored by default.
#
#   ROOT=~/arbos-qa INTERVAL=900 bash mirror-timer.sh
set -u
ROOT="${ROOT:-$HOME/arbos-qa}"
INTERVAL="${INTERVAL:-900}"
STORE="${ARBOS_QA_STORE_ROOT:-/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983}"
MIRROR="$STORE/internal/mirror-docs.sh"
REPO="$ROOT/repo"
LOG="$ROOT/loop/store-mirror-losses.jsonl"

pass() {
  local prev new mrc ts deleted
  ts="$(date -u +%FT%TZ)"
  git -C "$REPO" fetch -q origin store-docs 2>/dev/null || true
  prev="$(git -C "$REPO" rev-parse origin/store-docs 2>/dev/null || echo none)"
  if [ ! -f "$MIRROR" ]; then
    echo "[$ts] mirror script gone from the store; alarm"
    python3 "$ROOT/deploy/mirror-alarm.py" "$ROOT/loop" 3 "$MIRROR" "$REPO" || true
    return
  fi
  REPO="$REPO" timeout 12m bash "$MIRROR"
  mrc=$?
  if [ "$mrc" -ne 0 ]; then
    echo "[$ts] mirror-docs exit $mrc (a refusal is an alarm)"
    python3 "$ROOT/deploy/mirror-alarm.py" "$ROOT/loop" "$mrc" "$MIRROR" "$REPO" || true
    return
  fi
  git -C "$REPO" fetch -q origin store-docs 2>/dev/null || true
  new="$(git -C "$REPO" rev-parse origin/store-docs 2>/dev/null || echo none)"
  [ "$prev" = none ] || [ "$prev" = "$new" ] && return
  deleted="$(git -C "$REPO" diff --diff-filter=D --name-only "$prev" "$new" | grep -v '^internal/qa/rollouts/' || true)"
  [ -n "$deleted" ] || return
  local stage="$ROOT/state/mirror-restore/$(date -u +%Y%m%dT%H%M%SZ)"
  mkdir -p "$stage"
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    mkdir -p "$stage/$(dirname "$f")"
    git -C "$REPO" show "$prev:$f" > "$stage/$f" 2>/dev/null || true
  done <<< "$deleted"
  python3 - "$LOG" "$ts" "$prev" "$new" "$stage" <<'PY' "$deleted"
import json, sys
log, ts, prev, new, stage, files = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5], sys.argv[6].split("\n")
files = [f for f in files if f]
with open(log, "a") as fh:
    fh.write(json.dumps({"ts": ts, "prev": prev, "new": new, "vanished": files, "staged": stage, "restored": False, "verdict": "unknown — the author says whether it was a move, a deliberate delete, or a loss"}) + "\n")
print(f"[{ts}] vanished, staged, not restored — {len(files)} file(s) since {prev[:8]}: " + ", ".join(files[:8]) + (" …" if len(files) > 8 else "") + f"; copies under {stage}")
PY
}

echo "[$(date -u +%FT%TZ)] store mirror timer: every ${INTERVAL}s, repo $REPO"
while :; do
  pass
  sleep "$INTERVAL"
done

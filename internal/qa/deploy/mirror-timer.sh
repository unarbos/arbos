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
  # This client's view, one JSON line, beside the mirror: the row other machines' probes are compared with.
  MACHINE="${MACHINE:-qa-vm}" STORE_PROBE_LOG="$ROOT/loop/store-probe-qa-vm.jsonl" bash "$ROOT/deploy/store-probe.sh" >/dev/null 2>&1 || true
  # The second reader (the mesh worker's, internal/store-second-reader.md), run from THIS client too, so the
  # store-watch branch carries two machines' verdicts of the same half hour and a third can join by CLIENT name.
  # The script is fetched from the branch each pass — it rides there because the store copy is taken in episodes.
  if git -C "$REPO" fetch -q origin "+refs/heads/store-watch:refs/remotes/origin/store-watch" 2>/dev/null \
     && git -C "$REPO" show origin/store-watch:store-second-reader.sh > "$ROOT/deploy/store-second-reader.sh" 2>/dev/null; then
    CLIENT="${CLIENT:-qa-vm}" REPO="$REPO" timeout 8m bash "$ROOT/deploy/store-second-reader.sh" run 2>&1 | tail -2 | sed 's/^/[second-reader qa-vm] /' || true
  fi
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
  # Re-read before concluding a file is gone: this mount can answer partially (three reads seconds apart
  # gave 1092, 1122, 1092 files once, all present). A file seen in any of three reads is not vanished.
  local confirmed="" unstable=0 f
  for f in $deleted; do
    local seen=0 i
    for i in 1 2 3; do [ -e "$STORE/$f" ] && { seen=1; break; }; sleep 3; done
    if [ "$seen" = 1 ]; then unstable=1; else confirmed="$confirmed$f"$'\n'; fi
  done
  if [ "$unstable" = 1 ]; then
    # A partial answer means: conclude nothing. The snapshot is safe (the gate would refuse a partial push);
    # wait and run the whole pass again rather than record anything from this view.
    echo "[$ts] partial view: files the snapshot lacked are present on re-read; concluding nothing, re-reading in 3 min"
    return 2
  fi
  deleted="$confirmed"
  [ -n "$(echo "$deleted" | tr -d '[:space:]')" ] || return
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
    fh.write(json.dumps({"ts": ts, "prev": prev, "new": new, "vanished": files, "staged": stage, "restored": False, "reread": "absent in three reads over ~6 s", "verdict": "unknown — the author says whether it was a move, a deliberate delete, or a loss"}) + "\n")
print(f"[{ts}] vanished, staged, not restored — {len(files)} file(s) since {prev[:8]}: " + ", ".join(files[:8]) + (" …" if len(files) > 8 else "") + f"; copies under {stage}")
PY
}

echo "[$(date -u +%FT%TZ)] store mirror timer: every ${INTERVAL}s, repo $REPO"
while :; do
  pass
  if [ "$?" = 2 ]; then
    sleep 180
    pass
  fi
  sleep "$INTERVAL"
done

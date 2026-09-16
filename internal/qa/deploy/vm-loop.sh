#!/usr/bin/env bash
# Hourly QA loop on a cloud VM (stand-in for ArbosLife while it is off).
# Runs from the Project store's runner: copies it into $ROOT/loop before
# each cycle (the store is a slow FUSE mount; rollouts stage locally), runs
# deploy/cycle.sh with the system toolchain, then sleeps to the next hour.
#
#   ARBOS_QA_ROOT (default ~/arbos-qa)   ARBOS_QA_STORE (the store's internal/qa)
set -uo pipefail
ROOT="${ARBOS_QA_ROOT:-$HOME/arbos-qa}"
STORE="${ARBOS_QA_STORE:-/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/qa}"
export ARBOS_QA_ROOT="$ROOT" ARBOS_QA_SYSTEM_TOOLCHAIN=1 ARBOS_QA_STAGING="$ROOT/staging"
export ARBOS_QA_TRACK_BRANCHES="${ARBOS_QA_TRACK_BRANCHES:-main}"  # main is canonical since 2026-09-13 23:50 UTC (#58, #104, #106, #105 merged)
export ARBOS_QA_BUDGET_USD="${ARBOS_QA_BUDGET_USD:-10}"
export ARBOS_QA_DESKTOP="${ARBOS_QA_DESKTOP:-1}" ARBOS_QA_DRIVER_DIR="${ARBOS_QA_DRIVER_DIR:-$([ -f /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/parity/arbosdriver.py ] && echo /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/parity || echo /home/ubuntu/arbos-qa/repo/desktop/driver)}"
mkdir -p "$ROOT/loop" "$ROOT/logs"
# Pause switch: a file state/PAUSED-until-<UTC ISO> stops every run until that instant
# (Jacob, 2026-09-13 21:50 UTC). Remove the file or wait it out to resume.
qa_paused() {
  for f in "$ROOT"/state/PAUSED-until-*; do
    [ -e "$f" ] || continue
    until=$(basename "$f" | sed 's/^PAUSED-until-//'); until_s=$(date -u -d "$until" +%s 2>/dev/null || echo 0)
    if [ "$(date -u +%s)" -lt "$until_s" ]; then echo "== paused until $until (remove $f to resume)"; return 0; fi
    rm -f "$f"
  done
  return 1
}
while true; do
  if qa_paused; then sleep 300; continue; fi
  # Runner from the store (small files only); bugs are merged, never clobbered.
  for f in run.py consistency.py desktop_scenarios.py fileplan_scenarios.py multitasking_scenarios.py remote_scenarios.py batch_scenarios.py crossproject_scenarios.py journey_scenarios.py; do cp -f "$STORE/$f" "$ROOT/loop/$f" 2>/dev/null; done
  mkdir -p "$ROOT/loop/scenarios" "$ROOT/loop/bugs" "$ROOT/loop/inbox"
  cp -f "$STORE"/scenarios/*.json "$ROOT/loop/scenarios/" 2>/dev/null
  cp -f "$STORE"/bugs/qa-*.md "$ROOT/loop/bugs/" 2>/dev/null
  cp -f "$STORE"/inbox/*.md "$ROOT/loop/inbox/" 2>/dev/null
  cp -f "$STORE"/deploy/cycle.sh "$STORE"/deploy/publish.sh "$STORE"/deploy/kill-shim.sh "$ROOT/deploy/" 2>/dev/null
  chmod +x "$ROOT"/deploy/*.sh
  cp -f "$STORE"/deploy/swebench-nightly.sh "$STORE"/deploy/swebench-collect.py "$STORE"/deploy/call-mode-collect.py "$STORE"/deploy/pod-health.py "$STORE"/deploy/mirror-alarm.py "$ROOT/deploy/" 2>/dev/null; chmod +x "$ROOT"/deploy/*.sh
  "$ROOT/deploy/cycle.sh" || echo "== cycle failed ($?)"
  # Nightly SWE-bench slice after the 02:00 UTC cycle (16 instances, capped).
  if [ "$(date -u +%H)" = "02" ] && [ ! -e "$ROOT/state/swebench-$(date -u +%F)" ]; then
    touch "$ROOT/state/swebench-$(date -u +%F)"
    "$ROOT/deploy/swebench-nightly.sh" || echo "== swebench nightly failed ($?)"
  fi
  # New auto-drafts and the histories go back to the store for triage.
  for f in "$ROOT"/loop/bugs/*.md; do b="$(basename "$f")"; [ -e "$STORE/bugs/$b" ] || cp -f "$f" "$STORE/bugs/$b"; done
  cp -f "$ROOT/loop/kickoff-history.jsonl" "$STORE/vm-kickoff-history.jsonl" 2>/dev/null
  cp -f "$ROOT/loop/spend.jsonl" "$STORE/vm-spend.jsonl" 2>/dev/null
  cp -f "$ROOT/loop/rollouts/index.jsonl" "$STORE/rollouts/vm-index.jsonl" 2>/dev/null
  now=$(date +%s); next=$(( (now / 3600 + 1) * 3600 ))
  echo "== next cycle at $(date -u -d @$next +%FT%TZ)"
  # Short sleeps, not one long one: the VM is suspended while the agent is idle and a long
  # sleep resumes with its remaining time, so the next cycle would start late after a wake.
  while [ "$(date -u +%s)" -lt "$next" ]; do sleep 30; done
done

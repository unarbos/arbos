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
# This machine's name, in the store-probe file, the ledger names and the cycle log. `qa-vm` is the
# machine that ran the loop until 2026-09-17; a second machine must pick another name before it runs
# a cycle, or it writes over the first one's record.
export ARBOS_QA_MACHINE="${ARBOS_QA_MACHINE:-qa-vm}"
LEDGER=""; [ "$ARBOS_QA_MACHINE" = "qa-vm" ] || LEDGER="-$ARBOS_QA_MACHINE"
export ARBOS_QA_TRACK_BRANCHES="${ARBOS_QA_TRACK_BRANCHES:-main}"  # main is canonical since 2026-09-13 23:50 UTC (#58, #104, #106, #105 merged)
export ARBOS_QA_BUDGET_USD="${ARBOS_QA_BUDGET_USD:-20}"  # $20/day approved by Jacob 2026-09-16; report when it binds, do not raise
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
# The store answers per client (2026-09-17 05:35: one VM saw it empty and unwritable while two others read and
# wrote it). Nothing here trusts a read of the store that could be a blackout, and nothing written to it is lost
# when the write fails: it is staged under $ROOT/store-pending at the store-relative path and applied on the next
# pass that finds the store sound. Sound = notes.md and docs/ both listable.
PENDING="$ROOT/store-pending"
store_sound() { [ -s "$STORE_ROOT/notes.md" ] && [ -d "$STORE_ROOT/docs" ] && [ -f "$STORE/run.py" ]; }
# May the staged copy go over what the store holds now? Content, not time (2026-09-17, the mesh
# worker's check when it applied our staged files): the store copy must be contained in ours —
# ours is theirs plus additions, nothing of theirs removed beyond a couple of changed lines. A
# mount can lie about times; content cannot. Anything else is kept staged and said out loud.
may_overwrite() {  # may_overwrite <mine> <theirs>  (theirs absent → yes)
  [ -e "$2" ] || return 0
  cmp -s "$1" "$2" && return 0
  python3 - "$1" "$2" <<'PY'
import sys, difflib
mine = open(sys.argv[1], errors="replace").read().splitlines()
theirs = open(sys.argv[2], errors="replace").read().splitlines()
removed = 0
for tag, i1, i2, j1, j2 in difflib.SequenceMatcher(None, theirs, mine, autojunk=False).get_opcodes():
    if tag in ("delete", "replace"):
        removed += i2 - i1
sys.exit(0 if removed <= 2 else 1)
PY
}
store_put() {  # store_put <local file> <store-relative path>
  local src="$1"
  local rel="${2:?store_put needs <local file> <store-relative path>}"
  local dst="$STORE_ROOT/$rel"
  if store_sound && mkdir -p "$(dirname "$dst")" 2>/dev/null && cp -f "$src" "$dst" 2>/dev/null; then return 0; fi
  # Staging obeys the same content rule as applying (2026-09-17 12:07: this step replaced two hand-staged,
  # newer bug files with the loop's 09:28 copies, and the branch built from the staging tree carried the
  # rollback to the relay). A staged copy is only replaced by something that contains it.
  if [ -e "$PENDING/$rel" ] && ! may_overwrite "$src" "$PENDING/$rel"; then
    echo "== NOT staged $rel: the staged copy has lines this one does not; kept" >&2
    return 1
  fi
  mkdir -p "$PENDING/$(dirname "$rel")" && cp -f "$src" "$PENDING/$rel" && echo "== store write failed or store not sound; staged $rel under $PENDING" >&2
  return 1
}
apply_pending() {
  [ -d "$PENDING" ] || return 0
  store_sound || { echo "== store not sound from here; $(find "$PENDING" -type f | wc -l) staged file(s) kept"; return 0; }
  local n=0 held=0 f rel dst tmp
  while IFS= read -r f; do
    rel="${f#"$PENDING/"}"; dst="$STORE_ROOT/$rel"
    if ! may_overwrite "$f" "$dst"; then
      echo "== HELD $rel: the store's copy has lines ours does not (someone wrote there since); kept under $PENDING, merge by hand"
      held=$((held+1)); continue
    fi
    tmp="$dst.pending.$$"
    if mkdir -p "$(dirname "$dst")" 2>/dev/null && cp -f "$f" "$tmp" 2>/dev/null && mv -f "$tmp" "$dst" 2>/dev/null && cmp -s "$f" "$dst"; then
      rm -f "$f"; n=$((n+1))
    else
      rm -f "$tmp" 2>/dev/null
    fi
  done < <(find "$PENDING" -type f)
  find "$PENDING" -type d -empty -delete 2>/dev/null
  [ "$n" -gt 0 ] && echo "== applied $n staged file(s) to the store (content checked, written via rename, read back)"
  [ "$held" -gt 0 ] && echo "== $held staged file(s) HELD: the store has newer content"
  return 0
}
STORE_ROOT="${STORE%/internal/qa}"

while true; do
  if qa_paused; then sleep 300; continue; fi
  apply_pending
  if ! store_sound; then
    echo "== $(date -u +%FT%TZ) the store is not sound from this client (notes.md/docs/run.py not all listable); running the cycle on the runner already here, syncing nothing from it"
  fi
  # Runner from the store (small files only); bugs are merged, never clobbered.
  if store_sound; then
  for f in run.py consistency.py desktop_scenarios.py fileplan_scenarios.py multitasking_scenarios.py remote_scenarios.py batch_scenarios.py crossproject_scenarios.py journey_scenarios.py landing_scenarios.py uw_scenarios.py; do cp -f "$STORE/$f" "$ROOT/loop/$f" 2>/dev/null; done
  mkdir -p "$ROOT/loop/scenarios" "$ROOT/loop/bugs" "$ROOT/loop/inbox"
  cp -f "$STORE"/scenarios/*.json "$ROOT/loop/scenarios/" 2>/dev/null
  cp -f "$STORE"/bugs/qa-*.md "$STORE"/bugs/qal-*.md "$ROOT/loop/bugs/" 2>/dev/null
  # Everything else the store holds — the auto-drafts and the ui-* pass — only where this tree has no
  # copy: never over a local one, which may carry "seen" lines the store has not taken yet. Without
  # this a fresh machine's loop/bugs is the curated set alone (75 of 208), and publish.sh mirrors that
  # smaller tree onto qa-results (see its refusal; qal-j21).
  cp -n "$STORE"/bugs/*.md "$ROOT/loop/bugs/" 2>/dev/null
  cp -n "$STORE"/bugs/seen.jsonl "$ROOT/loop/bugs/" 2>/dev/null
  cp -f "$STORE"/inbox/*.md "$ROOT/loop/inbox/" 2>/dev/null
  cp -f "$STORE"/deploy/cycle.sh "$STORE"/deploy/publish.sh "$STORE"/deploy/kill-shim.sh "$STORE"/deploy/ns-wrap.sh "$ROOT/deploy/" 2>/dev/null
  chmod +x "$ROOT"/deploy/*.sh
  cp -f "$STORE"/deploy/mirror-timer.sh "$STORE"/deploy/swebench-nightly.sh "$STORE"/deploy/swebench-collect.py "$STORE"/deploy/call-mode-collect.py "$STORE"/deploy/pod-health.py "$STORE"/deploy/mirror-alarm.py "$ROOT/deploy/" 2>/dev/null; chmod +x "$ROOT"/deploy/*.sh
  fi
  "$ROOT/deploy/cycle.sh" || echo "== cycle failed ($?)"
  # Nightly SWE-bench slice after the 02:00 UTC cycle (16 instances, capped).
  if [ "$(date -u +%H)" = "02" ] && [ ! -e "$ROOT/state/swebench-$(date -u +%F)" ]; then
    touch "$ROOT/state/swebench-$(date -u +%F)"
    "$ROOT/deploy/swebench-nightly.sh" || echo "== swebench nightly failed ($?)"
  fi
  # New auto-drafts and the histories go back to the store for triage.
  # Bug files go to the store only when it is sound and does not already hold them. Never staged: the loop's
  # copies are the store's copies from the last sync, and a stale copy staged over a hand edit is a rollback.
  if store_sound; then for f in "$ROOT"/loop/bugs/*.md; do b="$(basename "$f")"; [ -e "$STORE/bugs/$b" ] || store_put "$f" "internal/qa/bugs/$b"; done; fi
  # One writer per file. Every ledger carries the machine that wrote it, because store_put's sound-store
  # path is a plain cp with no content check: two machines running the loop at once, as on 2026-09-17
  # during the handover, means the last one to finish a cycle overwrites the other's runs. $LEDGER is
  # empty for the historical machine, so its files keep the names the documents already cite.
  store_put "$ROOT/loop/kickoff-history.jsonl" "internal/qa/vm${LEDGER}-kickoff-history.jsonl"
  store_put "$ROOT/loop/spend.jsonl" "internal/qa/vm${LEDGER}-spend.jsonl"
  store_put "$ROOT/loop/rollouts/index.jsonl" "internal/qa/rollouts/vm${LEDGER}-index.jsonl"
  for h in journey-history.jsonl store-mirror-losses.jsonl store-mirror-history.jsonl; do
    [ -f "$ROOT/loop/$h" ] && store_put "$ROOT/loop/$h" "internal/qa/${h%.jsonl}${LEDGER}.jsonl"
  done
  apply_pending
  now=$(date +%s); next=$(( (now / 3600 + 1) * 3600 ))
  echo "== next cycle at $(date -u -d @$next +%FT%TZ)"
  # Short sleeps, not one long one: the VM is suspended while the agent is idle and a long
  # sleep resumes with its remaining time, so the next cycle would start late after a wake.
  while [ "$(date -u +%s)" -lt "$next" ]; do sleep 30; done
done

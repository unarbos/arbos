#!/bin/bash
# Concurrency cap for heavy jobs on the shared EC2 Mac (8 cores, one 250 GB volume).
#
#   arbos-heavy-slot.sh acquire <name> [--priority] [--pid <owner>]   → prints the slot path; blocks until free
#       (the owner pid defaults to the caller's parent; pass `--pid $$` from a script that uses $(…))
#   arbos-heavy-slot.sh release <slot-path>
#   arbos-heavy-slot.sh status
#
# Heavy = the gate's Mac half (cargo build --release -p arbos-mac), the gate's iOS half (arbos-ffi cross
# builds + xcodebuild), the TestFlight archive and the Mac release job. Three of them at once put the box
# at load 300–418 on 2026-10-25 (auditor 23:07) and every one of them got slower. At most MAX_SLOTS run at
# a time; the rest wait here, polling every 10 s. A hotfix pin (`--priority`) may take one extra slot so
# it never queues behind a pool gate and the TF lane together. A slot whose owner pid is gone is stale
# and reclaimed. Waiting is bounded (WAIT_MAX_S): a job that waited that long proceeds anyway with a log
# line — the cap smooths load, it must never deadlock a release.
#
# Slots live in /Users/Shared/arbos-run/heavy-slots (ec2-user runs the gate, arbosgui runs the Actions
# runner; the directory is world-writable, each slot is a directory made atomically with mkdir).
set -uo pipefail
SLOTS_DIR="${ARBOS_HEAVY_SLOTS_DIR:-/Users/Shared/arbos-run/heavy-slots}"
MAX_SLOTS="${ARBOS_HEAVY_MAX_SLOTS:-1}"
PRIORITY_EXTRA="${ARBOS_HEAVY_PRIORITY_EXTRA:-1}"
WAIT_MAX_S="${ARBOS_HEAVY_WAIT_MAX_S:-2700}"
POLL_S=10

ts() { date -u +%Y-%m-%dT%H:%M:%SZ; }

ensure_dir() {
  [[ -d "$SLOTS_DIR" ]] && return 0
  mkdir -p "$SLOTS_DIR" 2>/dev/null || sudo mkdir -p "$SLOTS_DIR" 2>/dev/null || return 1
  chmod 1777 "$SLOTS_DIR" 2>/dev/null || sudo chmod 1777 "$SLOTS_DIR" 2>/dev/null || true
}

# live_slots: drop slots whose owner pid is dead, print the number that remain.
live_slots() {
  local d pid n=0
  for d in "$SLOTS_DIR"/*.slot; do
    [[ -d "$d" ]] || continue
    pid=$(cat "$d/pid" 2>/dev/null || echo "")
    # ps, not kill -0: the gate (ec2-user) and the runner (arbosgui) check each other's slots and kill -0 on
    # another user's pid is EPERM — the slot would read as stale and be reclaimed.
    if [[ -z "$pid" ]] || ! ps -p "$pid" >/dev/null 2>&1; then
      rm -rf "$d" 2>/dev/null && echo "$(ts) heavy-slot: reclaimed stale slot $(basename "$d") (pid ${pid:-?} gone)" >&2
      continue
    fi
    n=$((n + 1))
  done
  echo "$n"
}

acquire() {
  local name="$1" priority=0 owner=$PPID limit t0 slot n waited_note=0
  shift
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --priority) priority=1 ;;
      --pid) owner="${2:-$PPID}"; shift ;;
    esac
    shift
  done
  ensure_dir || { echo "$(ts) heavy-slot: cannot create $SLOTS_DIR; running uncapped" >&2; echo "none"; return 0; }
  limit=$MAX_SLOTS
  (( priority )) && limit=$((MAX_SLOTS + PRIORITY_EXTRA))
  t0=$(date +%s)
  while :; do
    n=$(live_slots)
    if (( n < limit )); then
      slot="$SLOTS_DIR/$(date +%s)-$$-${name//\//_}.slot"
      if mkdir "$slot" 2>/dev/null; then
        echo "$owner" > "$slot/pid"; echo "$name" > "$slot/name"; date -u +%s > "$slot/since"
        # Re-check after taking it: two acquirers can pass the count together; the younger one yields
        # (slot names start with the epoch second and the pid, so a sort is the arrival order).
        if (( $(live_slots) > limit )) && ! ls -d "$SLOTS_DIR"/*.slot 2>/dev/null | sort | head -n "$limit" | grep -qF "$slot"; then
          rm -rf "$slot"
        else
          (( waited_note )) && echo "$(ts) heavy-slot: $name got a slot after $(( $(date +%s) - t0 )) s" >&2
          echo "$slot"; return 0
        fi
      fi
    fi
    if (( $(date +%s) - t0 >= WAIT_MAX_S )); then
      echo "$(ts) heavy-slot: $name waited ${WAIT_MAX_S}s for a slot ($n busy); proceeding uncapped" >&2
      echo "none"; return 0
    fi
    if (( ! waited_note )) || (( ( $(date +%s) - t0 ) % 60 < POLL_S )); then
      echo "$(ts) heavy-slot: $name waiting — $n heavy job(s) running (cap $limit): $(status_line)" >&2
      waited_note=1
    fi
    sleep "$POLL_S"
  done
}

status_line() {
  local d
  for d in "$SLOTS_DIR"/*.slot; do
    [[ -d "$d" ]] || continue
    printf '%s(pid %s, %ss) ' "$(cat "$d/name" 2>/dev/null)" "$(cat "$d/pid" 2>/dev/null)" "$(( $(date -u +%s) - $(cat "$d/since" 2>/dev/null || date -u +%s) ))"
  done
}

release() {
  [[ -z "${1:-}" || "$1" == "none" ]] && return 0
  rm -rf "$1" 2>/dev/null || true
}

case "${1:-}" in
  acquire) shift; acquire "${1:?name}" "${@:2}" ;;
  release) release "${2:-}" ;;
  status)  ensure_dir; echo "heavy slots: $(live_slots)/$MAX_SLOTS (+$PRIORITY_EXTRA priority) — $(status_line)" ;;
  *) echo "usage: $0 acquire <name> [--priority] | release <slot> | status" >&2; exit 2 ;;
esac

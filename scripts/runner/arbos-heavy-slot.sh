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
# Gate halves (names gate-*, the loop's merge path) must not starve behind rigs (2026-10-27 02:58: the pool
# batch's iOS half waited behind rigs mp-ios and clearread, then a rig held the only slot for over an hour).
# Two rules: (1) while a gate half is queued, non-gate acquirers wait (GATE_QUEUE marks the queue; a gate
# acquirer that got its slot clears its mark); (2) a non-gate lease older than RIG_LEASE_MAX_S is stale even
# with a live pid — reclaimed with a note, so one rig cannot hold the slot indefinitely. Release/TestFlight
# jobs (mac-release, tf-archive) are not rigs: they are exempt from the lease cap but still yield to a queued
# gate half.
RIG_LEASE_MAX_S="${ARBOS_HEAVY_RIG_LEASE_MAX_S:-3600}"
# A rig slot whose simulator (the slot's `udid` file, written by the xcrun boot guard) has been Shutdown
# for longer than SIM_IDLE_RECLAIM_S is a leak (03:38 Oct 27: bswipe's step.sh EXIT trap never fired when
# its test run was killed; the slot outlived the holder by an hour with the simulator already Shutdown).
SIM_IDLE_RECLAIM_S="${ARBOS_HEAVY_SIM_IDLE_RECLAIM_S:-300}"
GATE_QUEUE="$SLOTS_DIR/.gate-waiting"
# Fair share for rigs (08:4x Oct 27, bc-f1a62f3e / bc-1c63254c: with gate halves queued back to back, rigs
# waited 45 min, fell through "uncapped" with no slot, and the xcrun boot guard then refused the boot). After
# RIG_FAIR_AFTER_GATES consecutive gate grants, the next grant goes to a waiting rig even while a gate half
# is queued — a short lease: RIG_FAIR_LEASE_S (15 min) instead of the 60-min cap. The grant counter lives in
# $SLOTS_DIR/.gate-grants. And a waiter that exhausts WAIT_MAX_S gets a REAL slot (name uncapped-*, not counted
# against the cap) instead of "none", so the boot guard and the reclaim rules still see it.
RIG_FAIR_AFTER_GATES="${ARBOS_HEAVY_RIG_FAIR_AFTER_GATES:-2}"
RIG_FAIR_LEASE_S="${ARBOS_HEAVY_RIG_FAIR_LEASE_S:-900}"
GATE_GRANTS="$SLOTS_DIR/.gate-grants"
# Crowded rigs (17:41 Oct 27, coordinator: cap 1/1 for 50 min, 11 rig commands queued behind one waiting for a
# slot, Jacob's evidence shots all stuck): when RIG_CROWD_N or more rigs are waiting, rigs get every other
# turn (one gate grant between rig grants, same 15-min lease) instead of one in three. Rig waiters leave a
# marker in RIG_QUEUE while they wait, like gate halves do in GATE_QUEUE.
RIG_CROWD_N="${ARBOS_HEAVY_RIG_CROWD_N:-3}"
RIG_QUEUE="$SLOTS_DIR/.rig-waiting"
rigs_waiting() { local n=0 f; for f in "$RIG_QUEUE"/*; do [[ -f "$f" ]] || continue; if ps -p "$(basename "$f")" >/dev/null 2>&1; then n=$((n+1)); else rm -f "$f" 2>/dev/null; fi; done; echo "$n"; }
# Rig queue order (18:39 Oct 27, coordinator): FIFO by wait start — a rig that arrived later never takes a grant
# while an earlier one is still waiting. P0 rigs (names listed one per line in P0_RIGS, operator-set; a name
# matches exactly or as a `<name>-…` prefix) are ahead of every other rig and hold the slot for P0_RIG_LEASE_S.
# Rig markers carry "<wait-start-epoch> <name>".
P0_RIGS="${ARBOS_HEAVY_P0_RIGS_FILE:-$SLOTS_DIR/.p0-rigs}"
P0_RIG_LEASE_S="${ARBOS_HEAVY_P0_RIG_LEASE_S:-1800}"
# p0_rank <name>: the rig's line number in P0_RIGS (1 = first; the file's order is the priority order — a
# live-measurement P0 stays ahead of a recording P0), or 999 when the rig is not listed.
p0_rank() { local n=$1 e i=0; [[ -f "$P0_RIGS" ]] || { echo 999; return; }; while read -r e; do [[ -z "$e" || "$e" == \#* ]] && continue; i=$((i+1)); [[ "$n" == "$e" || "$n" == "$e"-* ]] && { echo "$i"; return; }; done < "$P0_RIGS"; echo 999; }
is_p0_rig() { [[ "$(p0_rank "$1")" != 999 ]]; }
rig_ahead_of_me() {  # rig_ahead_of_me <my-pid> <my-start> <my-name>: 0 when another waiting rig is ahead in the queue
  local me=$1 my_t0=$2 my_name=$3 f pid t0 name my_p0 p0
  my_p0=$(p0_rank "$my_name")
  for f in "$RIG_QUEUE"/*; do
    [[ -f "$f" ]] || continue
    pid=$(basename "$f"); [[ "$pid" == "$me" ]] && continue
    ps -p "$pid" >/dev/null 2>&1 || { rm -f "$f" 2>/dev/null; continue; }
    read -r t0 name < "$f" 2>/dev/null || continue
    [[ "$t0" =~ ^[0-9]+$ ]] || { name=$t0; t0=0; }   # pre-FIFO marker: name only, counts as oldest
    p0=$(p0_rank "$name")
    (( p0 < my_p0 )) && return 0
    (( p0 == my_p0 )) && (( t0 < my_t0 || ( t0 == my_t0 && pid < me ) )) && return 0
  done
  return 1
}
POLL_S=10

ts() { date -u +%Y-%m-%dT%H:%M:%SZ; }

ensure_dir() {
  [[ -d "$SLOTS_DIR" ]] && return 0
  mkdir -p "$SLOTS_DIR" 2>/dev/null || sudo mkdir -p "$SLOTS_DIR" 2>/dev/null || return 1
  chmod 1777 "$SLOTS_DIR" 2>/dev/null || sudo chmod 1777 "$SLOTS_DIR" 2>/dev/null || true
}

is_gate_name() { [[ "$1" == gate-* ]]; }
is_release_name() { [[ "$1" == mac-release* || "$1" == tf-archive* ]]; }
is_exempt_name() { is_release_name "$1" || [[ "$1" == gate-* ]]; }
# Release / TestFlight lanes (18:00 Oct 27, coordinator: the Mac release jobs for v0.6.299 — carrying P0 #2075 —
# sat 55 min while gate halves and rigs took the slot in turn): a release waiting RELEASE_PREEMPT_S takes the
# NEXT grant ahead of gate halves and rigs; a hotfix release (name *hotfix*) takes it at once. Release waiters
# leave "<start-epoch> <name>" in RELEASE_QUEUE/<pid>.
RELEASE_PREEMPT_S="${ARBOS_HEAVY_RELEASE_PREEMPT_S:-600}"
RELEASE_QUEUE="$SLOTS_DIR/.release-waiting"
release_due() {  # 1 when some waiting release may preempt the next grant
  local f pid start name now; now=$(date +%s)
  for f in "$RELEASE_QUEUE"/*; do
    [[ -f "$f" ]] || continue
    pid=$(basename "$f"); ps -p "$pid" >/dev/null 2>&1 || { rm -f "$f" 2>/dev/null; continue; }
    read -r start name < "$f" 2>/dev/null || continue
    [[ "$name" == *hotfix* ]] && return 0
    (( now - start >= RELEASE_PREEMPT_S )) && return 0
  done
  return 1
}
# A rig acquirer running under the shared GUI runner (a `bash cmd-*.sh` ancestor) gives up after
# RIG_RUNNER_WAIT_MAX_S: the command file is copied to the back of the runner's FIFO (name sorts last) and the
# acquire exits 75, so one rig's slot wait cannot hold every other rig's shots (18:00 Oct 27: 75 min, 15 queued).
RIG_RUNNER_WAIT_MAX_S="${ARBOS_HEAVY_RIG_RUNNER_WAIT_MAX_S:-1200}"
runner_cmd_file() {  # the cmd-*.sh an ancestor `bash` is executing (runner v2 runs it as cmd-*.sh.running), or nothing
  # runner v2 also exports ARBOS_RUNNER_CMD=<path>; the ancestor walk is the fallback for the old runner.
  if [[ -n "${ARBOS_RUNNER_CMD:-}" ]]; then echo "$ARBOS_RUNNER_CMD"; return 0; fi
  local pid=$$ cmd
  while (( pid > 1 )); do
    cmd=$(ps -o command= -p "$pid" 2>/dev/null)
    if [[ "$cmd" =~ (^|[[:space:]])bash[[:space:]]+(/Users/Shared/arbos-run/)?(cmd-[^[:space:]]+\.sh)(\.running)?([[:space:]]|$) ]]; then echo "/Users/Shared/arbos-run/${BASH_REMATCH[3]}${BASH_REMATCH[4]}"; return 0; fi
    pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' '); [[ -n "$pid" ]] || break
  done
  return 1
}

# live_slots: drop slots whose owner pid is dead or (rigs only) whose lease is over RIG_LEASE_MAX_S, print the
# number that remain.
live_slots() {
  local d pid name since age n=0
  for d in "$SLOTS_DIR"/*.slot; do
    [[ -d "$d" ]] || continue
    pid=$(cat "$d/pid" 2>/dev/null || echo "")
    # ps, not kill -0: the gate (ec2-user) and the runner (arbosgui) check each other's slots and kill -0 on
    # another user's pid is EPERM — the slot would read as stale and be reclaimed.
    if [[ -z "$pid" ]] || ! ps -p "$pid" >/dev/null 2>&1; then
      rm -rf "$d" 2>/dev/null && echo "$(ts) heavy-slot: reclaimed stale slot $(basename "$d") (pid ${pid:-?} gone)" >&2
      continue
    fi
    name=$(cat "$d/name" 2>/dev/null || echo "")
    since=$(cat "$d/since" 2>/dev/null || echo "")
    age=$(( $(date -u +%s) - ${since:-$(date -u +%s)} ))
    local cap=$RIG_LEASE_MAX_S
    [[ -f "$d/lease" ]] && cap=$(cat "$d/lease" 2>/dev/null || echo "$RIG_LEASE_MAX_S")
    if ! is_exempt_name "$name" && (( age > cap )); then
      rm -rf "$d" 2>/dev/null && echo "$(ts) heavy-slot: reclaimed rig lease $(basename "$d") held ${age}s (> ${cap}s; pid $pid still runs uncapped — rule #1957: a slot is for the burst, not the session)" >&2
      logger -t arbos-heavy-slot "reclaimed rig lease $name (pid $pid) after ${age}s" 2>/dev/null || true
      continue
    fi
    if ! is_exempt_name "$name" && [[ -f "$d/udid" ]]; then
      # The rig's simulator is Shutdown: the burst the slot was for is over. Note the first time we see it
      # shut down; reclaim after SIM_IDLE_RECLAIM_S.
      local udid state shut
      udid=$(cat "$d/udid" 2>/dev/null || echo "")
      state=$(sim_state "$udid")
      if [[ "$state" == "Shutdown" || "$state" == "missing" ]]; then
        shut=$(cat "$d/shutdown-since" 2>/dev/null || echo "")
        if [[ -z "$shut" ]]; then date -u +%s > "$d/shutdown-since" 2>/dev/null || true
        elif (( $(date -u +%s) - shut > SIM_IDLE_RECLAIM_S )); then
          rm -rf "$d" 2>/dev/null && echo "$(ts) heavy-slot: reclaimed rig slot $(basename "$d") — its simulator $udid has been $state for $(( $(date -u +%s) - shut ))s (holder pid $pid alive but idle)" >&2
          logger -t arbos-heavy-slot "reclaimed rig slot $name (pid $pid): simulator $udid $state > ${SIM_IDLE_RECLAIM_S}s" 2>/dev/null || true
          continue
        fi
      else
        rm -f "$d/shutdown-since" 2>/dev/null || true
      fi
    fi
    [[ "$name" == uncapped-* ]] && continue   # over the cap by decision after WAIT_MAX_S; not counted against waiters
    n=$((n + 1))
  done
  echo "$n"
}

# sim_state <udid>: Booted | Shutdown | missing | unknown (any user's device set; simctl lists only the caller's,
# so ask the slot owner's user when it is not ours).
sim_state() {
  local udid="$1" out u
  [[ -n "$udid" ]] || { echo unknown; return; }
  out=$(xcrun simctl list devices 2>/dev/null | grep -F "($udid)")
  if [[ -z "$out" ]]; then
    for u in ec2-user arbosgui; do
      [[ "$u" == "$(id -un)" ]] && continue
      out=$(sudo -n -u "$u" xcrun simctl list devices 2>/dev/null | grep -F "($udid)") && [[ -n "$out" ]] && break
    done
  fi
  [[ -z "$out" ]] && { echo missing; return; }
  case "$out" in *"(Booted)"*) echo Booted ;; *"(Shutdown)"*) echo Shutdown ;; *) echo unknown ;; esac
}

# bind <slot> <udid>: a rig that booted a simulator under this slot records it, so an idle simulator can
# release the slot by itself (the xcrun boot guard does this for every guarded `simctl boot`).
bind() { [[ -d "${1:-}" && -n "${2:-}" ]] && echo "$2" > "$1/udid" 2>/dev/null; return 0; }

# gate_waiting: a gate half is queued (its marker's pid is alive).
gate_waiting() {
  local f pid
  for f in "$GATE_QUEUE"/*; do
    [[ -f "$f" ]] || continue
    pid=$(cat "$f" 2>/dev/null || echo "")
    if [[ -n "$pid" ]] && ps -p "$pid" >/dev/null 2>&1; then return 0; fi
    rm -f "$f" 2>/dev/null
  done
  return 1
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
  # Release / TestFlight lanes count against the priority cap like gate halves (release.yml: "at most two of
  # {gate Mac half, gate iOS half, TF archive, Mac release}"). Until 00:24 Oct 28 they used the base cap, so one
  # gate half holding its slot kept a due release waiting for the whole half (Release 858b354df sat 14 min
  # behind gate-ios 001406-12 on an idle box, load 6) — the preempt rule only decided who got the NEXT grant.
  { (( priority )) || is_release_name "$name"; } && limit=$((MAX_SLOTS + PRIORITY_EXTRA))
  t0=$(date +%s)
  local mark=""
  if is_gate_name "$name"; then
    mkdir -p "$GATE_QUEUE" 2>/dev/null; chmod 1777 "$GATE_QUEUE" 2>/dev/null || true
    mark="$GATE_QUEUE/$$"; echo "$owner" > "$mark" 2>/dev/null || mark=""
  elif is_release_name "$name"; then
    mkdir -p "$RELEASE_QUEUE" 2>/dev/null; chmod 1777 "$RELEASE_QUEUE" 2>/dev/null || true
    mark="$RELEASE_QUEUE/$$"; echo "$t0 $name" > "$mark" 2>/dev/null || mark=""
  elif ! is_exempt_name "$name"; then
    mkdir -p "$RIG_QUEUE" 2>/dev/null; chmod 1777 "$RIG_QUEUE" 2>/dev/null || true
    mark="$RIG_QUEUE/$$"; echo "$t0 $name" > "$mark" 2>/dev/null || mark=""
  fi
  local runner_cmd=""; ! is_exempt_name "$name" && runner_cmd=$(runner_cmd_file || true)
  local fair=0 fair_after
  while :; do
    n=$(live_slots)
    # Rig fair share: a rig may take the slot ahead of a queued gate half once RIG_FAIR_AFTER_GATES gate halves
    # were granted in a row (short lease); with RIG_CROWD_N or more rigs waiting, after a single gate grant.
    fair=0; fair_after=$RIG_FAIR_AFTER_GATES
    if ! is_gate_name "$name" && ! is_exempt_name "$name"; then
      (( $(rigs_waiting) >= RIG_CROWD_N )) && fair_after=1
      (( $(cat "$GATE_GRANTS" 2>/dev/null || echo 0) >= fair_after )) && fair=1
    fi
    # Grant order: a due release first (it preempts gate halves and rigs); then gate halves; rigs when no gate half
    # waits or on their fair turn. A release not yet due still yields to a queued gate half, as before.
    local self_due=0
    if is_release_name "$name" && { [[ "$name" == *hotfix* ]] || (( $(date +%s) - t0 >= RELEASE_PREEMPT_S )); }; then self_due=1; fi
    # Rigs: queue order first (P0 rigs ahead, then FIFO by wait start), then the gate / fair-share rules.
    local my_turn=1
    if ! is_exempt_name "$name" && rig_ahead_of_me "$$" "$t0" "$name"; then my_turn=0; fi
    if (( n < limit )) && (( my_turn )) && { (( self_due )) || { ! release_due && { is_gate_name "$name" || ! gate_waiting || (( fair )); }; }; }; then
      slot="$SLOTS_DIR/$(date +%s)-$$-${name//\//_}.slot"
      if mkdir "$slot" 2>/dev/null; then
        echo "$owner" > "$slot/pid"; echo "$name" > "$slot/name"; date -u +%s > "$slot/since"
        # Re-check after taking it: two acquirers can pass the count together; the younger one yields
        # (slot names start with the epoch second and the pid, so a sort is the arrival order).
        if (( $(live_slots) > limit )) && ! ls -d "$SLOTS_DIR"/*.slot 2>/dev/null | sort | head -n "$limit" | grep -qF "$slot"; then
          rm -rf "$slot"
        else
          (( waited_note )) && echo "$(ts) heavy-slot: $name got a slot after $(( $(date +%s) - t0 )) s" >&2
          [[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null
          if is_gate_name "$name"; then
            echo $(( $(cat "$GATE_GRANTS" 2>/dev/null || echo 0) + 1 )) > "$GATE_GRANTS" 2>/dev/null || true
          elif ! is_exempt_name "$name"; then
            echo 0 > "$GATE_GRANTS" 2>/dev/null || true
            if (( fair )); then echo "$RIG_FAIR_LEASE_S" > "$slot/lease"; echo "$(ts) heavy-slot: $name took its fair-share turn (lease ${RIG_FAIR_LEASE_S}s, after $fair_after gate grant(s), $(rigs_waiting) rig(s) waiting) ahead of a queued gate half" >&2; fi
            if is_p0_rig "$name"; then echo "$P0_RIG_LEASE_S" > "$slot/lease"; echo "$(ts) heavy-slot: $name is a P0 rig — first in the rig queue, lease ${P0_RIG_LEASE_S}s" >&2; fi
          fi
          echo "$slot"; return 0
        fi
      fi
    fi
    if [[ -n "$runner_cmd" ]] && (( $(date +%s) - t0 >= RIG_RUNNER_WAIT_MAX_S )); then
      local again orig; orig="${runner_cmd%.running}"; again="/Users/Shared/arbos-run/cmd-zz~requeued-$(date +%s)-$(basename "${orig%.sh}").sh"
      if [[ -f "$runner_cmd" ]] && { printf '# requeued by arbos-heavy-slot at %s: %s waited %ss for a heavy slot under the shared runner (cap %s); original %s\n' "$(ts)" "$name" "$RIG_RUNNER_WAIT_MAX_S" "$limit" "$(basename "$runner_cmd")"; cat "$runner_cmd"; } > "$again" 2>/dev/null; then
        echo "$(ts) heavy-slot: $name waited ${RIG_RUNNER_WAIT_MAX_S}s under the shared runner ($n busy); giving up — command requeued at the back as $(basename "$again")" >&2
        logger -t arbos-heavy-slot "rig $name gave up its slot wait after ${RIG_RUNNER_WAIT_MAX_S}s; requeued $(basename "$again")" 2>/dev/null || true
      else
        echo "$(ts) heavy-slot: $name waited ${RIG_RUNNER_WAIT_MAX_S}s under the shared runner ($n busy); giving up (could not requeue $runner_cmd)" >&2
      fi
      [[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null
      echo none; return 75
    fi
    if (( $(date +%s) - t0 >= WAIT_MAX_S )); then
      # A real slot over the cap, so the boot guard and the reclaim rules still see this job.
      slot="$SLOTS_DIR/$(date +%s)-$$-uncapped-${name//\//_}.slot"
      if mkdir "$slot" 2>/dev/null; then echo "$owner" > "$slot/pid"; echo "uncapped-$name" > "$slot/name"; date -u +%s > "$slot/since"; else slot=none; fi
      echo "$(ts) heavy-slot: $name waited ${WAIT_MAX_S}s for a slot ($n busy); proceeding uncapped (slot $(basename "$slot"))" >&2
      [[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null
      echo "$slot"; return 0
    fi
    if (( ! waited_note )) || (( ( $(date +%s) - t0 ) % 60 < POLL_S )); then
      local why=""; ! is_gate_name "$name" && gate_waiting && why=" — a gate half is queued, rigs yield"
      echo "$(ts) heavy-slot: $name waiting — $n heavy job(s) running (cap $limit)$why: $(status_line)" >&2
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
  bind)    bind "${2:-}" "${3:-}" ;;
  reclaim) ensure_dir; n=$(live_slots); echo "$(ts) heavy-slot: reclaim pass — $n live slot(s)" ;;
  status)  ensure_dir; echo "heavy slots: $(live_slots)/$MAX_SLOTS (+$PRIORITY_EXTRA priority; rig lease cap ${RIG_LEASE_MAX_S}s$(gate_waiting && echo '; gate half queued')) — $(status_line)" ;;
  *) echo "usage: $0 acquire <name> [--priority] [--pid <owner>] | release <slot> | bind <slot> <udid> | reclaim | status" >&2; exit 2 ;;
esac

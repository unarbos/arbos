#!/bin/bash
# Concurrency cap for heavy jobs on the shared EC2 Mac (8 cores, one 250 GB volume).
#
#   arbos-heavy-slot.sh acquire <name> [--priority] [--pid <owner>] [--no-wait | --wait-max <s>]   → prints the slot path; blocks until free
#       (the owner pid defaults to the caller's parent; pass `--pid $$` from a script that uses $(…)).
#       A rig asking while a release lane holds a slot is refused: nothing printed, exit 75 (see "Release window").
#       --no-wait asks once and never joins a queue: no waiting marker is written, nothing is promoted over the
#       cap, and the owner is not signalled. It prints the slot path (exit 0) when a slot is free and it is this
#       caller's turn, otherwise nothing (exit 75) — "ask, and if refused do something else".
#       --wait-max <s> waits in the queue for at most s seconds (a handover, not a forty-minute wait): not
#       granted by then, it clears its own marker and exits 75 with nothing printed. It is never promoted over
#       the cap and does not signal the owner.
#   arbos-heavy-slot.sh release <slot-path>
#   arbos-heavy-slot.sh status
#
# Call it from a rig in this shape, not as a bare `slot=$(… acquire …)`. The bare form makes the acquire a child
# that outlives a killed rig (30 Oct: an orphaned pin1547-ios waiter sat in the queue 31 min for a dead owner):
#
#   flag=""      # a plain string, not an array (the gate Mac's /bin/bash is 3.2)
#   usage=$(/usr/local/bin/arbos-heavy-slot.sh 2>&1 || true)
#   case "$usage" in *--wait-max*) flag="--wait-max 30" ;; esac
#   out=$(mktemp)
#   /usr/local/bin/arbos-heavy-slot.sh acquire <rig> --pid $$ $flag >"$out" & acq=$!
#   trap 'kill $acq 2>/dev/null; rm -f "$out"; exit 143' INT TERM
#   rc=0; wait $acq || rc=$?; trap - INT TERM     # `|| rc=$?`, so `set -e` does not end the rig before the cleanup
#   slot=$(cat "$out"); rm -f "$out"
#   [ "$rc" -eq 0 ] || exit "$rc"     # 75: refused or not granted. Stop; do not run the heavy step anyway.
#
# Probe for a flag by capturing the usage text, as above. `… 2>&1 | grep -q -- --wait-max` is always false
# under `set -o pipefail`: the usage path exits 2 and the pipeline takes that status, so the caller quietly
# takes the slow path on every version.
#
# The guard is not the safety net: kill -9 runs no trap. The acquire itself gives up within one poll (10 s,
# 2 s under --wait-max) of its --pid owner dying, whatever the signal, so a bare-form orphan is short-lived; the
# guard closes that last poll, in which an orphan could still win a grant for a dead pid.
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
UNCAPPED_LOAD_MAX="${ARBOS_HEAVY_UNCAPPED_LOAD_MAX:-48}"  # rigs do not go uncapped while the 1-min load is above this (8 cores × 6)
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
# Release window (18:07 Oct 30): mac-release took its slot at 18:07:03, and 41 s later a rig past WAIT_MAX_S was
# promoted to an uncapped-* slot, because the one-minute load had not yet climbed past UNCAPPED_LOAD_MAX. The
# load guard only lets a rig through in exactly that window, where a heavy job has started but the average has
# not caught up. So while a release lane holds a slot, rigs get nothing: no capped grant and no uncapped
# promotion. A rig that asks, or is already waiting, fails with exit 75 and an instruction rather than firing.
# This reads the slot table, not the load. The rule lives here, not in a document, because a worker in the
# middle of a turn cannot hear a stand-down, and every rig already comes through this script.
# The owner (--pid) is sent SIGTERM on refusal: rig scripts written `slot=$(… acquire …)` carry on whatever the
# exit status, and a refused rig that carries on runs uncapped. ARBOS_HEAVY_REFUSE_STOPS_OWNER=0 turns that off.
REFUSE_STOPS_OWNER="${ARBOS_HEAVY_REFUSE_STOPS_OWNER:-1}"
release_held() {  # prints the release lane that holds a live slot; status 1 when none does
  local d pid name
  for d in "$SLOTS_DIR"/*.slot; do
    [[ -d "$d" ]] || continue
    name=$(cat "$d/name" 2>/dev/null || echo "")
    is_release_name "$name" || continue
    pid=$(cat "$d/pid" 2>/dev/null || echo "")
    if [[ -n "$pid" ]] && ps -p "$pid" >/dev/null 2>&1; then echo "$name"; return 0; fi
  done
  return 1
}
release_waiting() {  # a release lane is queued for a slot (its marker's pid is alive)
  local f
  for f in "$RELEASE_QUEUE"/*; do
    [[ -f "$f" ]] || continue
    ps -p "$(basename "$f")" >/dev/null 2>&1 && return 0
  done
  return 1
}
refuse_in_release() {  # refuse_in_release <name> <release> <owner> <owner-given> <mark>
  local name=$1 rel=$2 owner=$3 given=$4 mark=$5
  [[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null
  echo "$(ts) heavy-slot: $name refused — $rel holds the Mac. No slot, capped or uncapped, during a release. Do not retry or requeue: ask the coordinator for a window between releases." >&2
  logger -t arbos-heavy-slot "refused rig $name during $rel (owner pid $owner)" 2>/dev/null || true
  if (( REFUSE_STOPS_OWNER && given )) && [[ "$owner" =~ ^[0-9]+$ ]] && (( owner > 1 && owner != $$ )); then
    echo "$(ts) heavy-slot: stopping $name's owner (pid $owner, SIGTERM) so it does not run without a slot" >&2
    kill -TERM "$owner" 2>/dev/null || true
  fi
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
  local name="$1" priority=0 owner=$PPID owner_given=0 nowait=0 waitmax=-1 bounded=0 limit t0 slot n waited_note=0
  shift
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --priority) priority=1 ;;
      --pid) owner="${2:-$PPID}"; owner_given=1; shift ;;
      --no-wait) nowait=1 ;;
      --wait-max)
        [[ "${2:-}" =~ ^[0-9]+$ ]] || { echo "$(ts) heavy-slot: --wait-max needs a number of seconds" >&2; return 2; }
        waitmax=$2; bounded=1; shift ;;
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
  # A P0 rig also counts against the priority cap: with gate batches back to back one half always holds the base
  # slot, so a rig capped at MAX_SLOTS could only ever run between gate runs (outside-agents, 04:1x Oct 28).
  ! is_exempt_name "$name" && is_p0_rig "$name" && limit=$((MAX_SLOTS + PRIORITY_EXTRA))
  t0=$(date +%s)
  local mark=""
  # Writing the marker is joining the queue (others yield to it), so a --no-wait ask writes none.
  if (( nowait )); then :
  elif is_gate_name "$name"; then
    mkdir -p "$GATE_QUEUE" 2>/dev/null; chmod 1777 "$GATE_QUEUE" 2>/dev/null || true
    mark="$GATE_QUEUE/$$"; echo "$owner" > "$mark" 2>/dev/null || mark=""
  elif is_release_name "$name"; then
    mkdir -p "$RELEASE_QUEUE" 2>/dev/null; chmod 1777 "$RELEASE_QUEUE" 2>/dev/null || true
    mark="$RELEASE_QUEUE/$$"; echo "$t0 $name" > "$mark" 2>/dev/null || mark=""
  elif ! is_exempt_name "$name"; then
    mkdir -p "$RIG_QUEUE" 2>/dev/null; chmod 1777 "$RIG_QUEUE" 2>/dev/null || true
    mark="$RIG_QUEUE/$$"; echo "$t0 $name" > "$mark" 2>/dev/null || mark=""
  fi
  # Killed while waiting: leave the queue now rather than leave a marker for the next acquirer to prune.
  # SIGKILL runs no trap; the owner check in the loop and the pruning still cover that.
  local nap_pid=""
  trap '[[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null; [[ -n "$nap_pid" ]] && kill "$nap_pid" 2>/dev/null; exit 143' TERM INT HUP
  local runner_cmd=""; ! is_exempt_name "$name" && runner_cmd=$(runner_cmd_file || true)
  local fair=0 fair_after
  while :; do
    # The work this slot is for is gone (its rig was killed): stop waiting rather than win a grant nothing will use
    # (internal/heavy-slot-orphaned-acquire.md, 30 Oct: an orphaned pin1547-ios waiter, owner dead 30 min, still
    # queued; a dead grant holds the cap until the 300 s reclaim job runs). `ps`, not `kill -0`: the owner may be
    # another user's process, and kill -0 across users is EPERM.
    if ! ps -p "$owner" >/dev/null 2>&1; then
      [[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null
      echo "$(ts) heavy-slot: $name's owner (pid $owner) is gone; giving up the wait" >&2
      return 75
    fi
    n=$(live_slots)
    local rel
    if ! is_exempt_name "$name" && rel=$(release_held); then
      refuse_in_release "$name" "$rel" "$owner" "$(( owner_given && ! nowait && ! bounded ))" "$mark"
      return 75
    fi
    # Rig fair share: a rig may take the slot ahead of a queued gate half once RIG_FAIR_AFTER_GATES gate halves
    # were granted in a row (short lease); with RIG_CROWD_N or more rigs waiting, after a single gate grant.
    fair=0; fair_after=$RIG_FAIR_AFTER_GATES
    if ! is_gate_name "$name" && ! is_exempt_name "$name"; then
      (( $(rigs_waiting) >= RIG_CROWD_N )) && fair_after=1
      # An empty counter file made `(( >= fair_after ))` a syntax error (chart rig, 05:3x Oct 28): read it into a variable.
      local grants; grants=$(cat "$GATE_GRANTS" 2>/dev/null); grants=${grants//[^0-9]/}; (( ${grants:-0} >= fair_after )) && fair=1
    fi
    # Grant order: a due release first (it preempts gate halves and rigs); then gate halves; rigs when no gate half
    # waits or on their fair turn. A release not yet due still yields to a queued gate half, as before.
    local self_due=0
    if is_release_name "$name" && { [[ "$name" == *hotfix* ]] || (( $(date +%s) - t0 >= RELEASE_PREEMPT_S )); }; then self_due=1; fi
    # Rigs: queue order first (P0 rigs ahead, then FIFO by wait start), then the gate / fair-share rules.
    local my_turn=1
    if ! is_exempt_name "$name" && rig_ahead_of_me "$$" "$t0" "$name"; then my_turn=0; fi
    # P0 rigs (04:13 Oct 28, coordinator: outside-agents shots for Jacob's 14:00 deadline gave up after 1200 s
    # behind queued gate halves) do not yield to a QUEUED gate half — the P0 class means "first in line", not
    # "first among rigs"; a running half is never preempted, and a due release still goes first.
    local p0=0; ! is_exempt_name "$name" && is_p0_rig "$name" && p0=1
    if (( n < limit )) && (( my_turn )) && { (( self_due )) || { ! release_due && { is_gate_name "$name" || (( p0 )) || ! gate_waiting || (( fair )); }; }; }; then
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
            { local g; g=$(cat "$GATE_GRANTS" 2>/dev/null); g=${g//[^0-9]/}; echo $(( ${g:-0} + 1 )) > "$GATE_GRANTS"; chmod 666 "$GATE_GRANTS"; } 2>/dev/null || true
          elif ! is_exempt_name "$name"; then
            { echo 0 > "$GATE_GRANTS"; chmod 666 "$GATE_GRANTS"; } 2>/dev/null || true  # world-writable: rigs run as ec2-user and arbosgui
            if (( fair )); then echo "$RIG_FAIR_LEASE_S" > "$slot/lease"; echo "$(ts) heavy-slot: $name took its fair-share turn (lease ${RIG_FAIR_LEASE_S}s, after $fair_after gate grant(s), $(rigs_waiting) rig(s) waiting) ahead of a queued gate half" >&2; fi
            if is_p0_rig "$name"; then echo "$P0_RIG_LEASE_S" > "$slot/lease"; echo "$(ts) heavy-slot: $name is a P0 rig — first in the rig queue, lease ${P0_RIG_LEASE_S}s" >&2; fi
          fi
          echo "$slot"; return 0
        fi
      fi
    fi
    if (( nowait )) || { (( bounded )) && (( $(date +%s) - t0 >= waitmax )); }; then
      local busy="$n heavy job(s) running (cap $limit)" how="--no-wait, not queued"
      (( nowait )) || how="--wait-max ${waitmax}s ran out; left the queue"
      (( my_turn )) || busy="$busy; an earlier rig is waiting"
      ! is_gate_name "$name" && gate_waiting && busy="$busy; a gate half is queued"
      release_due && busy="$busy; a release is due"
      [[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null
      echo "$(ts) heavy-slot: $name not granted ($how) — $busy: $(status_line)" >&2
      return 75
    fi
    # A P0 rig is not requeued either: the requeue copy only puts the same series at the back of the runner
    # (and the author's script runs on uncapped when the acquire exits 75 — cmd-outside-agents-02, 04:07).
    # No uncapped fallthrough while the box is already overloaded (19:45 Oct 28: three rigs fell through on top
    # of a release build and a gate half, 3 booted sims, load 975 on 8 cores): a waiter past its budget keeps
    # waiting while the 1-min load is above UNCAPPED_LOAD_MAX; gate halves and releases are not rigs and keep
    # their own fallthrough (they are what the rigs would starve).
    local load1; load1=$(sysctl -n vm.loadavg 2>/dev/null | awk "{print int(\$2)}"); load1=${load1:-0}
    local overloaded=0; (( load1 > UNCAPPED_LOAD_MAX )) && ! is_exempt_name "$name" && overloaded=1
    # A queued release is about to load the box whatever the average says now: no promotion ahead of it.
    local release_queued=0; ! is_exempt_name "$name" && release_waiting && release_queued=1
    if (( overloaded )) && (( $(date +%s) - t0 >= RIG_RUNNER_WAIT_MAX_S )) && (( ($(date +%s) - t0) % 600 < 15 )); then
      echo "$(ts) heavy-slot: $name past its wait budget but load is $load1 (> $UNCAPPED_LOAD_MAX); not going uncapped, still waiting" >&2
    fi
    if (( ! overloaded && ! release_queued && ! bounded )) && [[ -n "$runner_cmd" ]] && (( ! p0 )) && (( $(date +%s) - t0 >= RIG_RUNNER_WAIT_MAX_S )); then
      # No requeue copies any more (17:2x Oct 28): a rig script that ignores exit 75 kept running while its copy
      # waited, requeued itself again, and so on — 32 cmd-zz~requeued-*-pd-before copies and duplicate runs took
      # the Mac to load 558. Under the shared runner a rig that waited RIG_RUNNER_WAIT_MAX_S proceeds with a real
      # uncapped slot instead (same as WAIT_MAX_S), so the work runs once and stays visible to the reclaim rules.
      slot="$SLOTS_DIR/$(date +%s)-$$-uncapped-${name//\//_}.slot"
      if mkdir "$slot" 2>/dev/null; then echo "$owner" > "$slot/pid"; echo "uncapped-$name" > "$slot/name"; date -u +%s > "$slot/since"; else slot=none; fi
      echo "$(ts) heavy-slot: $name waited ${RIG_RUNNER_WAIT_MAX_S}s under the shared runner ($n busy); proceeding uncapped (slot $(basename "$slot"))" >&2
      logger -t arbos-heavy-slot "rig $name proceeds uncapped after ${RIG_RUNNER_WAIT_MAX_S}s under the shared runner" 2>/dev/null || true
      [[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null
      echo "$slot"; return 0
    fi
    if (( ! overloaded && ! release_queued && ! bounded )) && (( $(date +%s) - t0 >= WAIT_MAX_S )); then
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
    local nap=$POLL_S
    (( bounded && nap > 2 )) && nap=2
    (( bounded )) && (( waitmax - ($(date +%s) - t0) < nap )) && nap=$(( waitmax - ($(date +%s) - t0) ))
    (( nap < 1 )) && nap=1
    # In the background so a TERM is handled at once: bash runs a trap only after a foreground command ends.
    sleep "$nap" >/dev/null 2>&1 & nap_pid=$!
    wait "$nap_pid"; nap_pid=""
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
  status)  ensure_dir; echo "heavy slots: $(live_slots)/$MAX_SLOTS (+$PRIORITY_EXTRA priority; rig lease cap ${RIG_LEASE_MAX_S}s$(gate_waiting && echo '; gate half queued')$(rel=$(release_held) && echo "; $rel holds the Mac — rigs refused")) — $(status_line)" ;;
  *) echo "usage: $0 acquire <name> [--priority] [--pid <owner>] [--no-wait | --wait-max <s>] | release <slot> | bind <slot> <udid> | reclaim | status" >&2; exit 2 ;;
esac

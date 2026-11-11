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
    [[ -f "$d/retired" ]] && continue
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

# A published lease observation has one runtime tag. Exempt leases cannot take
# rig-expiry transitions, and malformed records remain counted until repaired.
lease_transition() {
  local state=$1 event=$2 name=${3:-}
  case "$state:$event" in
    Observed:MissingOwner|Observed:OwnerDead) echo StaleOwner ;;
    Observed:Invalid) echo InvalidHeld ;;
    Observed:OwnerAlive)
      if is_exempt_name "$name"; then echo ExemptHeld; else echo CheckingRig; fi ;;
    Observed:Release) echo Releasing ;;
    Observed:Retired) echo Retired ;;
    CheckingRig:Expired) echo ExpiredRig ;;
    CheckingRig:WithinLease) echo CheckingSimulator ;;
    CheckingSimulator:FirstStop) echo FirstStopped ;;
    CheckingSimulator:StillStopped) echo StoppedRig ;;
    CheckingSimulator:IdleExpired) echo ExpiredIdle ;;
    CheckingSimulator:Active|CheckingSimulator:NoSimulator) echo ActiveRig ;;
    FirstStopped:Count|StoppedRig:Count|ActiveRig:Count)
      if [[ "$name" == uncapped-* ]]; then echo UncappedHeld; else echo CappedHeld; fi ;;
    ExemptHeld:Count) echo CappedHeld ;;
    StaleOwner:Marked|ExpiredRig:Marked|ExpiredIdle:Marked|Releasing:Marked|Retired:Marked) echo Retired ;;
    StaleOwner:MarkFailed) echo DeadUnmarked ;;
    ExpiredRig:MarkFailed|ExpiredIdle:MarkFailed|Releasing:MarkFailed) echo RetirementFailed ;;
    Retired:Removed|DeadUnmarked:Removed|RetirementFailed:Removed) echo Reclaimed ;;
    Retired:RemoveFailed) echo RetiredCleanupFailed ;;
    DeadUnmarked:RemoveFailed) echo DeadCleanupFailed ;;
    RetirementFailed:RemoveFailed) echo HeldCleanupFailed ;;
    InvalidHeld:*|CappedHeld:*|UncappedHeld:*|Reclaimed:*|RetiredCleanupFailed:*|DeadCleanupFailed:*|HeldCleanupFailed:*) echo "$state" ;;
    *) return 2 ;;
  esac
}

lease_counts() {
  case "$1" in
    InvalidHeld|CappedHeld|ExemptHeld|HeldCleanupFailed) echo 1 ;;
    UncappedHeld|Retired|Reclaimed|RetiredCleanupFailed|DeadCleanupFailed) echo 0 ;;
    *) return 2 ;;
  esac
}

# Only bounded decimal fields reach arithmetic; file contents are not shell expressions.
lease_uint() { [[ "$1" =~ ^[0-9]{1,18}$ ]]; }

# Revoke future boot authority before physical removal. A failed marker plus
# failed removal keeps a live record counted; successful retirement is terminal.
retire_lease() {
  local path=$1 phase=$2
  if : > "$path/retired" 2>/dev/null; then phase=$(lease_transition "$phase" Marked)
  else phase=$(lease_transition "$phase" MarkFailed); fi
  if rm -rf "$path" 2>/dev/null; then phase=$(lease_transition "$phase" Removed)
  else phase=$(lease_transition "$phase" RemoveFailed); fi
  lease=$phase
}

_live_slots_body() {
  local d pid name since age cap now lease udid sim shut n=0 claim request_pid count
  # Unpublished metadata left by an untrappable kill owns no capacity or boot permission.
  for claim in "$SLOTS_DIR"/.claim-*; do
    [[ -d "$claim" ]] || continue
    request_pid=${claim##*/.claim-}; request_pid=${request_pid%%-*}
    if lease_uint "$request_pid" && ! ps -p "$request_pid" >/dev/null 2>&1; then rm -rf "$claim" 2>/dev/null || true; fi
  done
  for d in "$SLOTS_DIR"/*.slot; do
    [[ -d "$d" ]] || continue
    pid=$(cat "$d/pid" 2>/dev/null || echo "")
    if [[ -f "$d/retired" ]]; then
      lease=$(lease_transition Observed Retired)
      retire_lease "$d" "$lease"
      continue
    elif [[ -e "$d/retired" ]]; then lease=$(lease_transition Observed Invalid)
    elif [[ -z "$pid" ]]; then lease=$(lease_transition Observed MissingOwner)
    elif ! lease_uint "$pid"; then lease=$(lease_transition Observed Invalid)
    elif ! ps -p "$pid" >/dev/null 2>&1; then lease=$(lease_transition Observed OwnerDead)
    else
      name=$(cat "$d/name" 2>/dev/null || echo "")
      since=$(cat "$d/since" 2>/dev/null || echo "")
      now=$(date -u +%s)
      cap=$RIG_LEASE_MAX_S
      [[ -f "$d/lease" ]] && cap=$(cat "$d/lease" 2>/dev/null || echo "$RIG_LEASE_MAX_S")
      if [[ -z "$name" ]] || ! lease_uint "$since" || ! lease_uint "$cap" || ! lease_uint "$now"; then
        lease=$(lease_transition Observed Invalid)
      else
        lease=$(lease_transition Observed OwnerAlive "$name")
        now=$((10#$now)); since=$((10#$since)); cap=$((10#$cap)); age=$((now - since))
      fi
    fi
    case "$lease" in
      StaleOwner)
        retire_lease "$d" "$lease"
        if [[ "$lease" == Reclaimed ]]; then echo "$(ts) heavy-slot: reclaimed stale slot $(basename "$d") (pid ${pid:-?} gone)" >&2; fi
        continue ;;
      InvalidHeld)
        echo "$(ts) heavy-slot: invalid lease metadata; keeping its capacity reserved" >&2
        count=$(lease_counts "$lease") || return 75; n=$((n + count)); continue ;;
      ExemptHeld) lease=$(lease_transition "$lease" Count); count=$(lease_counts "$lease") || return 75; n=$((n + count)); continue ;;
    esac
    if (( age > cap )); then lease=$(lease_transition "$lease" Expired)
    else lease=$(lease_transition "$lease" WithinLease); fi
    case "$lease" in
      ExpiredRig)
        retire_lease "$d" "$lease"
        if [[ "$lease" == Reclaimed ]]; then
          echo "$(ts) heavy-slot: reclaimed rig lease $(basename "$d") held ${age}s (> ${cap}s; pid $pid still runs uncapped — rule #1957: a slot is for the burst, not the session)" >&2
          logger -t arbos-heavy-slot "reclaimed rig lease $name (pid $pid) after ${age}s" 2>/dev/null || true
        else count=$(lease_counts "$lease") || return 75; n=$((n + count)); fi
        continue ;;
    esac
    if [[ -f "$d/udid" ]]; then
      udid=$(cat "$d/udid" 2>/dev/null || echo "")
      sim=$(sim_state "$udid")
      if [[ "$sim" == Shutdown || "$sim" == missing ]]; then
        shut=$(cat "$d/shutdown-since" 2>/dev/null || echo "")
        if [[ -z "$shut" ]]; then lease=$(lease_transition "$lease" FirstStop)
        elif ! lease_uint "$shut"; then lease=$(lease_transition Observed Invalid)
        elif (( now - 10#$shut > SIM_IDLE_RECLAIM_S )); then lease=$(lease_transition "$lease" IdleExpired)
        else lease=$(lease_transition "$lease" StillStopped); fi
      else lease=$(lease_transition "$lease" Active); fi
    else lease=$(lease_transition "$lease" NoSimulator); fi
    case "$lease" in
      FirstStopped) date -u +%s > "$d/shutdown-since" 2>/dev/null || true ;;
      ActiveRig) rm -f "$d/shutdown-since" 2>/dev/null || true ;;
      ExpiredIdle)
        retire_lease "$d" "$lease"
        if [[ "$lease" == Reclaimed ]]; then
          echo "$(ts) heavy-slot: reclaimed rig slot $(basename "$d") — its simulator $udid has been $sim for $((now - 10#$shut))s (holder pid $pid alive but idle)" >&2
          logger -t arbos-heavy-slot "reclaimed rig slot $name (pid $pid): simulator $udid $sim > ${SIM_IDLE_RECLAIM_S}s" 2>/dev/null || true
        else count=$(lease_counts "$lease") || return 75; n=$((n + count)); fi
        continue ;;
      InvalidHeld)
        echo "$(ts) heavy-slot: invalid lease metadata; keeping its capacity reserved" >&2
        count=$(lease_counts "$lease") || return 75; n=$((n + count)); continue ;;
    esac
    lease=$(lease_transition "$lease" Count "$name")
    count=$(lease_counts "$lease") || return 75
    n=$((n + count))
  done
  echo "$n"
}

# Direct status/reclaim serialize with admission; an admission's own child
# inherits its already-held FD and must not attempt a second lock.
_locked_live_slots() {
  heavy_lock_fd 1 || return 75
  local heavy_mutex_held=1
  _live_slots_body
} 9< "$SLOTS_DIR/.admission-lock"
live_slots() {
  if [[ "${heavy_mutex_held:-0}" == 1 ]]; then _live_slots_body
  else heavy_prepare_mutex && _locked_live_slots; fi
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

# Runtime-tagged request union. The claim carries its origin and path, and no
# terminal state can be promoted by a late retry. Files/PIDs/timers are effects at the edges.
heavy_transition() {
  local state=$1 event=$2 payload=${3:-} rest origin path tag
  case "$state" in
    New)
      case "$event" in Ask) echo Asking ;; Queue) echo Queued ;; Unavailable) echo Unavailable ;; Cancelled) echo Cancelled ;; *) return 2 ;; esac ;;
    Asking|Queued)
      case "$event" in
        Claim) [ -n "$payload" ] || return 2; echo "Claiming:$state:$payload" ;;
        Retry) echo "$state" ;;
        OwnerGone) echo Abandoned ;;
        ReleaseHeld) [ -n "$payload" ] || return 2; echo "Refused:Release:$payload" ;;
        Timeout) [ "$state" = Queued ] || return 2; echo TimedOut ;;
        NoWait) [ "$state" = Asking ] || return 2; echo Refused:Busy ;;
        Unavailable) echo Unavailable ;;
        Cancelled) echo Cancelled ;;
        *) return 2 ;;
      esac ;;
    Claiming:*|Prepared:*|Verified:*|Published:*)
      tag=${state%%:*}; rest=${state#*:}; origin=${rest%%:*}; path=${rest#*:}
      case "$origin" in Asking|Queued) ;; *) return 2 ;; esac
      [ -n "$path" ] && [ "$rest" != "$path" ] || return 2
      case "$tag:$event" in
        Claiming:Prepared)
          if lease_uint "$payload" && (( 10#$payload > 1 )) && [ -n "${4:-}" ] && lease_uint "${5:-}"; then echo "Prepared:$rest"
          else echo Unavailable; fi ;;
        Prepared:OwnerStatus|Published:OwnerStatus)
          case "$payload" in
            alive) if [[ "$tag" == Prepared ]]; then echo "Verified:$rest"; else echo "Granted:$path"; fi ;;
            dead) echo Abandoned ;;
            *) echo Unavailable ;;
          esac ;;
        Verified:Published) echo "Published:$rest" ;;
        Claiming:Lost) echo "$origin" ;;
        *:OwnerGone) echo Abandoned ;;
        *:Unavailable) echo Unavailable ;;
        *:Cancelled) echo Cancelled ;;
        *) return 2 ;;
      esac ;;
    Granted:?*) if [[ "$event" == Cancelled ]]; then echo Cancelled; else echo "$state"; fi ;;
    Refused:?*|Abandoned|TimedOut|Unavailable|Cancelled) echo "$state" ;;
    *) return 2 ;;
  esac
}

# This inode is permanent and is not a .slot; release validates its input so it
# cannot unlink it. The short kernel lock is never held across the queue sleep.
heavy_prepare_mutex() { (umask 000; : >> "$SLOTS_DIR/.admission-lock"); }
heavy_lock_fd() {
  local wait=${1:-0}
  if command -v lockf >/dev/null 2>&1; then
    if (( wait )); then lockf -s 9; else lockf -s -t 0 9; fi
  elif command -v flock >/dev/null 2>&1; then
    if (( wait )); then flock -x 9; else flock -n 9 || return 75; fi
  else return 69; fi
}
heavy_retire_locked() {
  heavy_lock_fd 1 || { lease=HeldCleanupFailed; return 1; }
  local heavy_mutex_held=1
  retire_lease "$@"
} 9< "$SLOTS_DIR/.admission-lock"
heavy_retire() {
  if [[ "${heavy_mutex_held:-0}" == 1 ]]; then retire_lease "$@"
  else heavy_prepare_mutex && heavy_retire_locked "$@"; fi
}

# Owned effects outlive function locals, so EXIT/TERM can retire a claim even
# when the signal was deferred until a foreground rename completed.
HEAVY_CLAIM=(None)
HEAVY_QUEUE_MARK=""
HEAVY_NAP_PID=""
heavy_cleanup() {
  local rc=$1 stage_path="" slot_path="" lease
  [[ -n "$HEAVY_QUEUE_MARK" ]] && rm -f "$HEAVY_QUEUE_MARK" 2>/dev/null
  [[ -n "$HEAVY_NAP_PID" ]] && kill "$HEAVY_NAP_PID" 2>/dev/null
  if (( rc != 0 )); then
    case "${HEAVY_CLAIM[0]}" in
      Staging) stage_path=${HEAVY_CLAIM[1]} ;;
      Publishing) stage_path=${HEAVY_CLAIM[1]}; slot_path=${HEAVY_CLAIM[2]} ;;
      Published) slot_path=${HEAVY_CLAIM[1]} ;;
      None) stage_path=${stage:-} ;;
    esac
    [[ -z "$stage_path" ]] || rm -rf "$stage_path" 2>/dev/null
    if [[ -n "$slot_path" && -d "$slot_path" ]]; then
      heavy_retire "$slot_path" Releasing || true
    fi
    HEAVY_CLAIM=(None)
  fi
  return 0
}

heavy_finish() {
  local state=$1
  case "$state" in
    Asking|Queued) return 1 ;;
    Granted:*) [[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null; echo "${state#Granted:}"; return 0 ;;
    Abandoned)
      [[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null
      echo "$(ts) heavy-slot: $name's owner (pid $owner) is gone; giving up the wait" >&2; return 75 ;;
    Refused:Release:*)
      refuse_in_release "$name" "${state#Refused:Release:}" "$owner" "$(( owner_given && ! nowait && ! bounded ))" "$mark"; return 75 ;;
    *)
      [[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null
      echo "$(ts) heavy-slot: $name not granted — cannot publish or lock a lease" >&2; return 75 ;;
  esac
}

# One admission transaction in the requester shell. Function-scoped FD9 closes on
# return, before a queue sleep. Only pure transitions use command substitution.
heavy_attempt() {
  local phase=$1 kind=$2 rc=0 stage="" slot rel load1 overloaded release_queued mode
  heavy_lock_fd || rc=$?
  case "$rc" in
    0) ;;
    75) state=$(heavy_transition "$phase" Retry); return ;;
    *) state=$(heavy_transition "$phase" Unavailable); return ;;
  esac
  local heavy_mutex_held=1
  if ! ps -p "$owner" >/dev/null 2>&1; then state=$(heavy_transition "$phase" OwnerGone); return; fi
  if ! n=$(live_slots); then state=$(heavy_transition "$phase" Unavailable); return; fi
  if ! is_exempt_name "$name" && rel=$(release_held); then state=$(heavy_transition "$phase" ReleaseHeld "$rel"); return; fi
  if [[ "$kind" == Normal ]]; then
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
    my_turn=1
    if ! is_exempt_name "$name" && rig_ahead_of_me "$$" "$t0" "$name"; then my_turn=0; fi
    # P0 rigs (04:13 Oct 28, coordinator: outside-agents shots for Jacob's 14:00 deadline gave up after 1200 s
    # behind queued gate halves) do not yield to a QUEUED gate half — the P0 class means "first in line", not
    # "first among rigs"; a running half is never preempted, and a due release still goes first.
    p0=0; ! is_exempt_name "$name" && is_p0_rig "$name" && p0=1
    if ! { (( n < limit )) && (( my_turn )) && { (( self_due )) || { ! release_due && { is_gate_name "$name" || (( p0 )) || ! gate_waiting || (( fair )); }; }; }; }; then state=$(heavy_transition "$phase" Retry); return; fi
    slot="$SLOTS_DIR/$(date +%s)-$$-${name//\//_}.slot"
  else
    case "$kind" in Runner|Budget) ;; *) state=$(heavy_transition "$phase" Unavailable); return ;; esac
    if (( nowait || bounded )); then state=$(heavy_transition "$phase" Unavailable); return; fi
    load1=$(sysctl -n vm.loadavg 2>/dev/null | awk "{print int(\$2)}"); load1=${load1:-0}
    overloaded=0; (( load1 > UNCAPPED_LOAD_MAX )) && ! is_exempt_name "$name" && overloaded=1
    release_queued=0; ! is_exempt_name "$name" && release_waiting && release_queued=1
    if (( overloaded || release_queued )); then state=$(heavy_transition "$phase" Retry); return; fi
    slot="$SLOTS_DIR/$(date +%s)-$$-uncapped-${name//\//_}.slot"
  fi
  phase=$(heavy_transition "$phase" Claim "$slot") || return 2
  state=$phase
  [[ ! -e "$slot" ]] || { state=$(heavy_transition "$phase" Lost); return; }
  stage=$(mktemp -d "$SLOTS_DIR/.claim-$$-XXXXXX") || { state=$(heavy_transition "$phase" Unavailable); return; }
  HEAVY_CLAIM=(Staging "$stage")
  mode=$(umask); mode=$((0777 & ~8#$mode)); printf -v mode '%o' "$mode"
  local stored_name="$name"
  [[ "$kind" == Normal ]] || stored_name="uncapped-$name"
  if ! { chmod "$mode" "$stage" && echo "$owner" > "$stage/pid" && echo "$stored_name" > "$stage/name" && date -u +%s > "$stage/since"; }; then
    state=$(heavy_transition "$phase" Unavailable); return
  fi
  # Complete metadata is a prerequisite state, not a convention after publication.
  if [[ "$kind" == Normal ]] && ! is_exempt_name "$name"; then
    if (( fair )); then echo "$RIG_FAIR_LEASE_S" > "$stage/lease" || { state=$(heavy_transition "$phase" Unavailable); return; }; fi
    if is_p0_rig "$name"; then echo "$P0_RIG_LEASE_S" > "$stage/lease" || { state=$(heavy_transition "$phase" Unavailable); return; }; fi
  fi
  phase=$(heavy_transition "$phase" Prepared "$owner" "$stored_name" "$(cat "$stage/since")") || return 2
  state=$phase
  [[ "$phase" != Unavailable ]] || return
  if ! ps -p "$owner" >/dev/null 2>&1; then state=$(heavy_transition "$phase" OwnerStatus dead); return; fi
  phase=$(heavy_transition "$phase" OwnerStatus alive); state=$phase
  HEAVY_CLAIM=(Publishing "$stage" "$slot")
  if ! mv "$stage" "$slot"; then state=$(heavy_transition "$phase" Unavailable); return; fi
  HEAVY_CLAIM=(Published "$slot")
  phase=$(heavy_transition "$phase" Published); state=$phase
  stage=""
  if ! ps -p "$owner" >/dev/null 2>&1; then
    heavy_cleanup 75; state=$(heavy_transition "$phase" OwnerStatus dead); return
  fi
  if [[ "$kind" == Normal ]]; then
          (( waited_note )) && echo "$(ts) heavy-slot: $name got a slot after $(( $(date +%s) - t0 )) s" >&2
          [[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null
          if is_gate_name "$name"; then
            { local g; g=$(cat "$GATE_GRANTS" 2>/dev/null); g=${g//[^0-9]/}; echo $(( ${g:-0} + 1 )) > "$GATE_GRANTS"; chmod 666 "$GATE_GRANTS"; } 2>/dev/null || true
          elif ! is_exempt_name "$name"; then
            { echo 0 > "$GATE_GRANTS"; chmod 666 "$GATE_GRANTS"; } 2>/dev/null || true  # world-writable: rigs run as ec2-user and arbosgui
            if (( fair )); then echo "$(ts) heavy-slot: $name took its fair-share turn (lease ${RIG_FAIR_LEASE_S}s, after $fair_after gate grant(s), $(rigs_waiting) rig(s) waiting) ahead of a queued gate half" >&2; fi
            if is_p0_rig "$name"; then echo "$(ts) heavy-slot: $name is a P0 rig — first in the rig queue, lease ${P0_RIG_LEASE_S}s" >&2; fi
          fi
  elif [[ "$kind" == Runner ]]; then
    echo "$(ts) heavy-slot: $name waited ${RIG_RUNNER_WAIT_MAX_S}s under the shared runner ($n busy); proceeding uncapped (slot $(basename "$slot"))" >&2
    logger -t arbos-heavy-slot "rig $name proceeds uncapped after ${RIG_RUNNER_WAIT_MAX_S}s under the shared runner" 2>/dev/null || true
  else
    echo "$(ts) heavy-slot: $name waited ${WAIT_MAX_S}s for a slot ($n busy); proceeding uncapped (slot $(basename "$slot"))" >&2
  fi
  [[ -n "$mark" ]] && rm -f "$mark" 2>/dev/null
  state=$(heavy_transition "$phase" OwnerStatus alive)
} 9< "$SLOTS_DIR/.admission-lock"

acquire() {
  local name="$1" priority=0 owner=$PPID owner_given=0 nowait=0 waitmax=-1 bounded=0 limit t0 slot n=0 waited_note=0 state=New mark="" rc
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
  if (( nowait )); then state=$(heavy_transition "$state" Ask); else state=$(heavy_transition "$state" Queue); fi
  if ! ensure_dir || ! heavy_prepare_mutex; then
    state=$(heavy_transition "$state" Unavailable); heavy_finish "$state"; return $?
  fi
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
  HEAVY_QUEUE_MARK=$mark
  trap 'heavy_cleanup "$?"' EXIT
  trap 'trap "" TERM INT HUP; state=$(heavy_transition "${state:-New}" Cancelled) || state=Unavailable; heavy_cleanup 143; trap - EXIT; exit 143' TERM INT HUP
  local runner_cmd=""; ! is_exempt_name "$name" && runner_cmd=$(runner_cmd_file || true)
  local fair=0 fair_after my_turn=1 p0=0
  while :; do
    heavy_attempt "$state" Normal || state=Unavailable
    rc=0; heavy_finish "$state" || rc=$?
    (( rc == 1 )) || return "$rc"
    if (( nowait )) || { (( bounded )) && (( $(date +%s) - t0 >= waitmax )); }; then
      local busy="$n heavy job(s) running (cap $limit)" how="--no-wait, not queued"
      (( nowait )) || how="--wait-max ${waitmax}s ran out; left the queue"
      (( my_turn )) || busy="$busy; an earlier rig is waiting"
      ! is_gate_name "$name" && gate_waiting && busy="$busy; a gate half is queued"
      release_due && busy="$busy; a release is due"
      if (( nowait )); then state=$(heavy_transition "$state" NoWait); else state=$(heavy_transition "$state" Timeout); fi
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
      heavy_attempt "$state" Runner || state=Unavailable
      rc=0; heavy_finish "$state" || rc=$?
      (( rc == 1 )) || return "$rc"
    fi
    if (( ! overloaded && ! release_queued && ! bounded )) && (( $(date +%s) - t0 >= WAIT_MAX_S )); then
      heavy_attempt "$state" Budget || state=Unavailable
      rc=0; heavy_finish "$state" || rc=$?
      (( rc == 1 )) || return "$rc"
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
    HEAVY_NAP_PID=$nap_pid
    wait "$nap_pid"; nap_pid=""; HEAVY_NAP_PID=""
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
  # Only direct slot children are releasable; never unlink the permanent admission mutex.
  if [[ "${1%/*}" != "$SLOTS_DIR" || "$1" != *.slot ]]; then
    echo "heavy-slot: invalid slot path" >&2; return 2
  fi
  [[ -e "$1" ]] || return 0
  local lease
  lease=$(lease_transition Observed Release)
  heavy_retire "$1" "$lease" || true
  case "$lease" in RetiredCleanupFailed|HeldCleanupFailed) echo "heavy-slot: lease release failed" >&2 ;; esac
  return 0
}

# Sourcing exposes the same pure transitions to the isolated checks without dispatching a command.
[[ "${BASH_SOURCE[0]}" == "$0" ]] || return 0

case "${1:-}" in
  acquire) shift; acquire "${1:?name}" "${@:2}" ;;
  release) release "${2:-}" ;;
  bind)    bind "${2:-}" "${3:-}" ;;
  reclaim) ensure_dir && n=$(live_slots) || exit 75; echo "$(ts) heavy-slot: reclaim pass — $n live slot(s)" ;;
  status)  ensure_dir && n=$(live_slots) || exit 75; echo "heavy slots: $n/$MAX_SLOTS (+$PRIORITY_EXTRA priority; rig lease cap ${RIG_LEASE_MAX_S}s$(gate_waiting && echo '; gate half queued')$(rel=$(release_held) && echo "; $rel holds the Mac — rigs refused")) — $(status_line)" ;;
  *) echo "usage: $0 acquire <name> [--priority] [--pid <owner>] [--no-wait | --wait-max <s>] | release <slot> | bind <slot> <udid> | reclaim | status" >&2; exit 2 ;;
esac

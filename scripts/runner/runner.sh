#!/bin/bash
# Shared GUI command runner for the EC2 Mac rigs (Terminal identity: the one TCC client with Screen Recording,
# so captures work here; clicks go through the `ax` helper to ax-runner.sh). Executes
# /Users/Shared/arbos-run/cmd-<rig>-<step>.sh; protocol unchanged: stdout+stderr → cmd-….log, exit code →
# cmd-….rc, then the command file is renamed cmd-….done.
#
# v2 (2026-10-27, coordinator): commands from DIFFERENT rigs run concurrently — one lane per rig, in order
# inside the lane, at most MAX_PAR lanes at once. Before, one rig command waiting 45 min for a heavy slot held
# every other rig's shots (11 queued; Jacob's evidence shots stuck). A command that needs a slot still waits
# inside arbos-heavy-slot.sh; it no longer blocks the shots of already-staged apps on other rigs.
# The rig is the first dash-separated token after `cmd-` (cmd-pd-before.sh → pd; cmd-chart-ios9.sh → chart).
# Queue order is FIFO by enqueue time (file birth time), not by name: a rig that names its command cmd-00…
# does not jump the queue; the rename is logged once for visibility, nothing else happens to it.
# A command is claimed by renaming it to cmd-….sh.running (atomic), so a second runner instance never runs it
# too; a cmd-….sh that an older runner is already executing (`bash cmd-X.sh` in ps) is left alone.
# macOS /bin/bash is 3.2: no associative arrays — lane state lives in $LANES/<rig> (the lane's pid).
#
# MUST be started from Terminal (the identity carries the Screen Recording grant):
#   cd /Users/Shared/arbos-run && exec bash runner.sh
cd /Users/Shared/arbos-run || exit 1
MAX_PAR=${RUNNER_MAX_PAR:-8}  # a lane waiting for a heavy slot costs nothing; shot-only lanes must not queue behind four of them
LANES=/Users/Shared/arbos-run/.lanes
mkdir -p "$LANES"; for f in "$LANES"/*; do [ -f "$f" ] && ! kill -0 "$(cat "$f")" 2>/dev/null && rm -f "$f"; done  # lanes of a previous v2 that still run stay booked
export PATH=/Users/Shared/arbos-run/axbin:$PATH
echo "runner v2 up pid $$ user $(id -un) $(date -u) max_par=$MAX_PAR bash=$BASH_VERSION" > runner.status
rig_of() { local b=${1#cmd-}; b=${b%%.*}; echo "${b%%-*}"; }
busy_lanes() { local n=0 f; for f in "$LANES"/*; do [ -f "$f" ] || continue; if kill -0 "$(cat "$f")" 2>/dev/null; then n=$((n+1)); else rm -f "$f"; fi; done; echo "$n"; }
lane_busy() { [ -f "$LANES/$1" ] && kill -0 "$(cat "$LANES/$1")" 2>/dev/null; }
run_one() {  # run_one <cmd-X.sh.running> <base> <rig>
  local f=$1 base=$2 rig=$3 rc
  ARBOS_RUNNER_CMD="/Users/Shared/arbos-run/$f" bash "$f" > "$base.log" 2>&1 < /dev/null; rc=$?   # arbos-heavy-slot reads it for the bounded wait
  echo "$rc" > "$base.rc"; mv "$f" "$base.done"; rm -f "$base.seen" "$LANES/$rig"
  echo "$(date -u +%FT%TZ) lane $rig done $base rc=$rc" >> runner.status
}
while true; do
  for f in $(stat -f '%B %N' cmd-*.sh 2>/dev/null | sort -n | awk '{print $2}'); do
    [ -f "$f" ] || continue
    rig=$(rig_of "$f"); base=${f%.sh}
    if [ ! -f "$base.seen" ]; then
      : > "$base.seen"
      older=$(stat -f '%B %N' cmd-*.sh 2>/dev/null | awk -v b="$(stat -f %B "$f")" -v me="$f" '$1 < b && $2 > me' | wc -l | tr -d ' ')
      [ "$older" -gt 0 ] && echo "$(date -u +%FT%TZ) note: $f (rig $rig) sorts before $older older command(s) by name — FIFO is by enqueue time, it waits its turn" >> runner.status
    fi
    lane_busy "$rig" && continue                                # this rig's lane is busy: keep its order
    [ "$(busy_lanes)" -ge "$MAX_PAR" ] && break                 # enough lanes running
    pgrep -f "bash $f\$" >/dev/null 2>&1 && continue            # an older runner is on it
    mv "$f" "$f.running" 2>/dev/null || continue                # claimed
    run_one "$f.running" "$base" "$rig" &
    echo $! > "$LANES/$rig"
    echo "$(date -u +%FT%TZ) lane $rig ← $f (pid $!, $(busy_lanes) lane(s) busy)" >> runner.status
  done
  sleep 1
done

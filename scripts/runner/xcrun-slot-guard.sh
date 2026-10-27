#!/bin/bash
# `xcrun` wrapper for the shared EC2 Mac: a simulator boot needs a held heavy slot (#1957).
#
# Installed as /usr/local/bin/xcrun and /Users/Shared/arbos-run/axbin/xcrun (both ahead of /usr/bin on the
# rig users' PATH); everything except `simctl boot` / `simctl bootstatus … -b` goes straight to the real
# /usr/bin/xcrun. A boot is allowed when one of these holds: a heavy slot whose owner pid is an ancestor of
# this process (the gate, a rig that did `slot=$(arbos-heavy-slot.sh acquire <rig> --pid $$)`),
# ARBOS_HEAVY_SLOT is set (the slot path, exported by a rig that acquired it in a subshell), or
# GITHUB_ACTIONS is set (release.yml / ios.yml take their slot in a separate step). Otherwise it prints the
# rule and exits 75 (EX_TEMPFAIL) without booting. Two rig simulators booted outside the cap took the box to
# load 548 on 2026-10-26 22:3x and ssh timed out for every worker.
REAL=/usr/bin/xcrun
SLOTS=/Users/Shared/arbos-run/heavy-slots
is_boot=0
if [ "${1:-}" = "simctl" ]; then
  case "${2:-}" in
    boot) is_boot=1 ;;
    bootstatus) for a in "$@"; do [ "$a" = "-b" ] && is_boot=1; done ;;
  esac
fi
[ "$is_boot" -eq 1 ] || exec "$REAL" "$@"
[ -n "${GITHUB_ACTIONS:-}" ] && exec "$REAL" "$@"
# Ancestors of this process.
anc=""; p=$$
while [ -n "$p" ] && [ "$p" -gt 1 ] 2>/dev/null; do anc="$anc $p"; p=$(ps -o ppid= -p "$p" 2>/dev/null | tr -d ' '); done
# The udid being booted (first 36-char UUID argument), bound to the allowing slot so an idle simulator
# releases the slot by itself (arbos-heavy-slot.sh reclaim).
udid=""; for a in "$@"; do case "$a" in [0-9A-F]*-*-*-*-*) [ ${#a} -eq 36 ] && udid=$a ;; esac; done
# bind_and_exec <slot>: record the udid on the slot, then run the real xcrun with the ORIGINAL arguments
# (08:4x Oct 27: the first version passed the slot path through to xcrun — "can't exec …slot").
ORIG_ARGS=("$@")
bind_and_exec() { [ -n "$udid" ] && /usr/local/bin/arbos-heavy-slot.sh bind "$1" "$udid" 2>/dev/null; exec "$REAL" "${ORIG_ARGS[@]}"; }
[ -n "${ARBOS_HEAVY_SLOT:-}" ] && [ -d "${ARBOS_HEAVY_SLOT}" ] && bind_and_exec "$ARBOS_HEAVY_SLOT"
for s in "$SLOTS"/*.slot; do
  [ -d "$s" ] || continue
  owner=$(basename "$s" | cut -d- -f2)
  case " $anc " in *" $owner "*) bind_and_exec "$s" ;; esac
done
cat >&2 <<MSG
xcrun simctl boot refused: no heavy slot is held by this process tree (rule #1957, shared EC2 Mac).
A simulator boot keeps ~250 daemons runnable; two of them outside the cap took the box to load 548 and
ssh timed out for every worker (2026-10-26 22:3x). Before booting, take a slot and keep it for the burst:
  slot=\$(/usr/local/bin/arbos-heavy-slot.sh acquire <your-rig> --pid \$\$)   # blocks until free
  xcrun simctl boot <udid>; xcrun simctl bootstatus <udid> -b
  /usr/local/bin/arbos-heavy-slot.sh release "\$slot"
and shut the simulator down when the step is done (xcrun simctl shutdown <udid>). Escape hatches:
export ARBOS_HEAVY_SLOT=<slot path> (a slot acquired in a subshell) or run /usr/bin/xcrun directly if you
are certain the box is idle — then say so on your issue.
MSG
exit 75

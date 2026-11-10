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

# One tagged boot attempt: Checking -> Authorized:<slot> | Denied | Passthrough.
# Only a live, fully published lease can authorize; terminal decisions are absorbing.
boot_transition() {
  case "$1" in Checking|Denied|Passthrough|Authorized:?*) ;; *) return 2 ;; esac
  case "$1:$2" in
    Checking:OwnedLease|Checking:ExplicitLiveLease)
      [ -n "${3:-}" ] || return 2
      echo "Authorized:$3" ;;
    Checking:RejectedLease) echo Checking ;;
    Checking:NoLease) echo Denied ;;
    Checking:NonBoot|Checking:Workflow) echo Passthrough ;;
    Authorized:*|Denied:*|Passthrough:*) echo "$1" ;;
    *) return 2 ;;
  esac
}

# The same pure constructor is used by explicit-slot and ancestor admission.
boot_lease_event() {
  case "$2:$1" in
    Ancestor:Owned) echo OwnedLease ;;
    Explicit:Owned|Explicit:External) echo ExplicitLiveLease ;;
    *) echo RejectedLease ;;
  esac
}

# Files/PID observations stay at this edge. Filename PID is the acquirer, not --pid owner.
lease_snapshot() {
  local slot=$1 pid name since
  [ -d "$slot" ] || { echo Absent; return; }
  [ ! -e "$slot/retired" ] || { echo Retired; return; }
  pid=$(cat "$slot/pid" 2>/dev/null || true)
  name=$(cat "$slot/name" 2>/dev/null || true)
  since=$(cat "$slot/since" 2>/dev/null || true)
  if ! [[ "$pid" =~ ^[0-9]{1,18}$ && "$since" =~ ^[0-9]{1,18}$ ]] || ! (( 10#$pid > 1 )) || [ -z "$name" ]; then
    echo Unpublished; return
  fi
  ps -p "$pid" >/dev/null 2>&1 || { echo Dead; return; }
  case " $anc " in *" $pid "*) echo Owned ;; *) echo External ;; esac
}

boot_main() {
state=Checking
is_boot=0
if [ "${1:-}" = "simctl" ]; then
  case "${2:-}" in
    boot) is_boot=1 ;;
    bootstatus) for a in "$@"; do [ "$a" = "-b" ] && is_boot=1; done ;;
  esac
fi
if [ "$is_boot" -ne 1 ]; then
  state=$(boot_transition "$state" NonBoot); exec "$REAL" "$@"
fi
if [ -n "${GITHUB_ACTIONS:-}" ]; then
  state=$(boot_transition "$state" Workflow); exec "$REAL" "$@"
fi
anc=""; p=$$
while [ -n "$p" ] && [ "$p" -gt 1 ] 2>/dev/null; do anc="$anc $p"; p=$(ps -o ppid= -p "$p" 2>/dev/null | tr -d ' '); done
udid=""; for a in "$@"; do case "$a" in [0-9A-F]*-*-*-*-*) [ ${#a} -eq 36 ] && udid=$a ;; esac; done
ORIG_ARGS=("$@")
bind_and_exec() { [ -n "$udid" ] && /usr/local/bin/arbos-heavy-slot.sh bind "$1" "$udid" 2>/dev/null; exec "$REAL" "${ORIG_ARGS[@]}"; }
if [ -n "${ARBOS_HEAVY_SLOT:-}" ]; then
  event=$(boot_lease_event "$(lease_snapshot "$ARBOS_HEAVY_SLOT")" Explicit)
  state=$(boot_transition "$state" "$event" "$ARBOS_HEAVY_SLOT")
  case "$state" in Authorized:*) bind_and_exec "${state#Authorized:}" ;; esac
fi
for s in "$SLOTS"/*.slot; do
  event=$(boot_lease_event "$(lease_snapshot "$s")" Ancestor)
  state=$(boot_transition "$state" "$event" "$s")
  case "$state" in Authorized:*) bind_and_exec "${state#Authorized:}" ;; esac
done
state=$(boot_transition "$state" NoLease)
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

}
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then boot_main "$@"; fi

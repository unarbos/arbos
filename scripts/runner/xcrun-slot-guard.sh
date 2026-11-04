#!/bin/bash
# arbos-boot-guard v1
# Boot guard for the shared EC2 Mac: a simulator boot needs a held heavy slot (#1957).
#
# ONE script, installed under TWO names, because there are two ways to reach a boot and a guard
# that closes one of them is an open door with a sign on it (AF-853).
#
#   as `xcrun`  → /usr/local/bin/xcrun and /Users/Shared/arbos-run/axbin/xcrun, both ahead of
#                 /usr/bin on the rig users' PATH. Guards `xcrun simctl boot`.
#   as `simctl` → the active developer directory's usr/bin/simctl, Xcode's own binary moved aside
#                 to simctl.real by install-boot-guard.sh.
#
# The second one is what closes the hole. `xcrun` boots nothing: it looks `simctl` up in the
# developer directory and execs it. So every route anyone spells — the PATH shim, /usr/bin/xcrun,
# and a direct $(xcode-select -p)/usr/bin/simctl — ends at the developer directory's simctl.
# /usr/bin/xcrun is on the SIP volume and cannot be replaced at all, which is exactly why the guard
# must not live at that layer.
#
# NOT the bottom of the stack, which an earlier version of this comment claimed. The developer
# directory's simctl is itself a bash shim, and its last line execs
# /Library/Developer/PrivateFrameworks/CoreSimulator.framework/Versions/A/Resources/bin/simctl.
# That binary is reachable and unprotected (it is on /System/Volumes/Data with no `restricted`
# flag), so calling it by its full path still boots without a slot, and --check does not look for
# it. It is left open on purpose: interposing inside an Apple framework bundle is a different risk
# class from interposing inside Xcode. Treat --check as "closed on the routes it knows".
#
# A boot is allowed when one of these holds: a heavy slot whose owner pid is an ancestor of this
# process (the gate, a rig that did `slot=$(arbos-heavy-slot.sh acquire <rig> --pid $$)`),
# ARBOS_HEAVY_SLOT names a slot directory inside the slots directory (a rig that acquired one in a
# subshell), or GITHUB_ACTIONS is set (release.yml / ios.yml take their slot in a separate step).
# Otherwise it prints the rule and exits 75 (EX_TEMPFAIL) without booting. Two rig simulators booted
# outside the cap took the box to load 548 on 2026-10-26 22:3x and ssh timed out for every worker.
#
# WHERE THE CONFIGURATION COMES FROM, and why not the environment. Everything that decides the
# verdict — the real binaries, the slots directory — is read from xcrun-slot-guard.conf beside the
# installed script, written by install-boot-guard.sh and writable only by root. None of it is read
# from the environment. An environment variable that moves the slots directory is a hatch as wide as
# the one this change closes: point it at a directory you can write and every boot is slotted.
#
# What remains forgeable is GITHUB_ACTIONS, which no file can verify. That is a deliberate
# difference in kind, not a leftover: the closed route was a documented absolute path that the
# guard's own refusal text used to recommend, and lanes took it. Claiming to be a CI runner is a
# thing a lane has to decide to do.
#
# Reached twice on the PATH route (shim → /usr/bin/xcrun → developer-dir simctl), which is harmless:
# the second pass re-runs the same checks and they still hold. It deliberately leaves itself no
# "already allowed" marker in the environment — a marker a caller can set is another hatch.
#
# Behaviour is proven by scripts/runner/boot-guard-selftest.sh, including that disabling the guard
# lets an unslotted boot through. Run it after any edit here.

REAL_XCRUN=/usr/bin/xcrun
REAL_SIMCTL=""           # default: simctl.real beside this script
SLOTS_DIR=/Users/Shared/arbos-run/heavy-slots
SLOT_SH=/usr/local/bin/arbos-heavy-slot.sh

here=$(cd "$(dirname "$0")" 2>/dev/null && pwd)
# shellcheck source=/dev/null
[ -r "$here/xcrun-slot-guard.conf" ] && . "$here/xcrun-slot-guard.conf"
[ -n "$REAL_SIMCTL" ] || REAL_SIMCTL="$here/simctl.real"

# Which binary takes the original arguments once a boot is allowed, and where in the argument list
# the simctl subcommand sits.
self=$(basename "$0")
if [ "$self" = "simctl" ]; then
  REAL="$REAL_SIMCTL"
  sub="${1:-}"
  called="simctl boot"
else
  REAL="$REAL_XCRUN"
  sub=""
  [ "${1:-}" = "simctl" ] && sub="${2:-}"
  called="xcrun simctl boot"
fi

is_boot=0
case "$sub" in
  boot) is_boot=1 ;;
  # `bootstatus -b` boots the device and then waits for it; without -b it only reports.
  bootstatus) for a in "$@"; do [ "$a" = "-b" ] && is_boot=1; done ;;
esac

[ "$is_boot" -eq 1 ] || exec "$REAL" "$@"
[ -n "${GITHUB_ACTIONS:-}" ] && exec "$REAL" "$@"

# Ancestors of this process.
anc=""; p=$$
while [ -n "$p" ] && [ "$p" -gt 1 ] 2>/dev/null; do anc="$anc $p"; p=$(ps -o ppid= -p "$p" 2>/dev/null | tr -d ' '); done
# The udid being booted (first 36-char UUID argument), bound to the allowing slot so an idle simulator
# releases the slot by itself (arbos-heavy-slot.sh reclaim).
udid=""; for a in "$@"; do case "$a" in [0-9A-Fa-f]*-*-*-*-*) [ ${#a} -eq 36 ] && udid=$a ;; esac; done
# bind_and_exec <slot>: record the udid on the slot, then run the real tool with the ORIGINAL
# arguments (08:4x Oct 27: the first version passed the slot path through to xcrun — "can't exec …slot").
ORIG_ARGS=("$@")
bind_and_exec() { [ -n "$udid" ] && [ -x "$SLOT_SH" ] && "$SLOT_SH" bind "$1" "$udid" 2>/dev/null; exec "$REAL" "${ORIG_ARGS[@]}"; }
# A slot handed over in the environment must be a real slot in the real slots directory. Before, any
# existing directory would do, so `ARBOS_HEAVY_SLOT=/tmp` was a one-word way past the cap.
case "${ARBOS_HEAVY_SLOT:-}" in
  "$SLOTS_DIR"/*.slot) [ -d "$ARBOS_HEAVY_SLOT" ] && bind_and_exec "$ARBOS_HEAVY_SLOT" ;;
esac
for s in "$SLOTS_DIR"/*.slot; do
  [ -d "$s" ] || continue
  owner=$(basename "$s" | cut -d- -f2)
  case " $anc " in *" $owner "*) bind_and_exec "$s" ;; esac
done

# The refusal. Its wording is asserted by the selftest, because an exit code on its own cannot tell
# a refusal apart from the boot simply failing — the confusion that produced the original defect,
# when a `2>/dev/null` hid this text and a refused boot read as a successful one one second later.
cat >&2 <<MSG
$called refused: no heavy slot is held by this process tree (rule #1957, shared EC2 Mac).
A simulator boot keeps ~250 daemons runnable; two of them outside the cap took the box to load 548 and
ssh timed out for every worker (2026-10-26 22:3x). Before booting, take a slot and keep it for the burst:
  slot=\$(/usr/local/bin/arbos-heavy-slot.sh acquire <your-rig> --pid \$\$)   # blocks until free
  xcrun simctl boot <udid>; xcrun simctl bootstatus <udid> -b
  /usr/local/bin/arbos-heavy-slot.sh release "\$slot"
and shut the simulator down when the step is done (xcrun simctl shutdown <udid>). A slot acquired in a
subshell is handed over with ARBOS_HEAVY_SLOT=<the slot path under $SLOTS_DIR>.
Calling /usr/bin/xcrun or the developer directory's simctl directly no longer goes round this: both
of them arrive here (AF-853). If a boot genuinely must run with no slot, say so on your issue and have
an operator do it, rather than looking for a path that skips the cap.
MSG
exit 75

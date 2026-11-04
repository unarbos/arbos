#!/usr/bin/env bash
# Put the simulator boot guard on every route into a boot, and say so when one is open (AF-853).
#
#   scripts/runner/install-boot-guard.sh            install / repair, then check
#   scripts/runner/install-boot-guard.sh --check    report only; exit 1 if any route is open
#
# Run it on the shared EC2 Mac as an admin (it writes /usr/local/bin and inside Xcode). --check
# needs no privileges and is the one to run from a health check.
#
# THE ROUTES. `xcrun` boots nothing; it resolves `simctl` in the active developer directory and
# execs it. So:
#
#   xcrun simctl boot          → PATH shim      → /usr/bin/xcrun → developer-dir simctl
#   /usr/bin/xcrun simctl boot →                                   developer-dir simctl
#   $(xcode-select -p)/usr/bin/simctl boot →                       developer-dir simctl
#
# All three end at the same file, so that file is where the guard has to be. The PATH shims stay
# as well: they refuse a few milliseconds earlier and with the `xcrun` wording, and they are the only
# protection left if an Xcode update restores its own simctl — which is why --check exists and why it
# should run on a schedule rather than being trusted to stay true.
#
# /usr/bin/xcrun cannot be wrapped: it is on the SIP-protected system volume. That is the whole
# reason the guard moved down a layer instead of being installed at one more name.
#
# WHAT --check DOES NOT LOOK FOR. The developer-directory simctl is a bash shim that execs
# /Library/Developer/PrivateFrameworks/CoreSimulator.framework/.../bin/simctl, and that binary is
# unprotected too, so its full path is a fourth route. Nothing here closes it or reports it. A
# clean --check therefore means "every route I know about is closed", which is what anything
# building on it — scripts/runner/arbos-heavy-slot.sh acquire, most of all — is entitled to assume
# and no more.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$here/xcrun-slot-guard.sh"
slot_sh="$here/arbos-heavy-slot.sh"
MARKER='# arbos-boot-guard v1'

AXBIN="${ARBOS_AXBIN:-/Users/Shared/arbos-run/axbin}"
SLOTS_DIR="${ARBOS_HEAVY_SLOTS_DIR:-/Users/Shared/arbos-run/heavy-slots}"
LOCALBIN="${ARBOS_LOCALBIN:-/usr/local/bin}"
DEVDIR="${ARBOS_DEVELOPER_DIR:-$(xcode-select -p 2>/dev/null || echo "")}"

check_only=0
[ "${1:-}" = "--check" ] && check_only=1

open_routes=0
note() { printf '%s\n' "$*"; }
bad()  { printf 'OPEN  %s\n' "$*"; open_routes=$((open_routes + 1)); }
good() { printf 'ok    %s\n' "$*"; }

is_guard() { [ -f "$1" ] && head -n 5 "$1" 2>/dev/null | grep -qF "$MARKER"; }

# write_conf <dir> <real-xcrun> <real-simctl>
write_conf() {
  cat > "$1/xcrun-slot-guard.conf" <<CONF
# Written by install-boot-guard.sh. The guard reads its configuration here and never from the
# environment: a slots directory a caller can move is a way past the cap.
REAL_XCRUN=$2
REAL_SIMCTL=$3
SLOTS_DIR=$SLOTS_DIR
SLOT_SH=$LOCALBIN/arbos-heavy-slot.sh
CONF
  chmod 644 "$1/xcrun-slot-guard.conf"
}

install_shim() {  # install_shim <dir> <name>
  local dir=$1 name=$2
  mkdir -p "$dir" || return 1
  install -m 755 "$guard" "$dir/$name" || return 1
  write_conf "$dir" /usr/bin/xcrun ""
}

install_simctl_guard() {
  local dir="$DEVDIR/usr/bin" target="$DEVDIR/usr/bin/simctl" real="$DEVDIR/usr/bin/simctl.real"
  [ -n "$DEVDIR" ] && [ -d "$dir" ] || { note "no developer directory at '${DEVDIR:-<unset>}'; cannot close the direct route"; return 1; }

  if is_guard "$target"; then
    [ -f "$real" ] || { note "guard is installed at $target but $real is missing — refusing to touch it"; return 1; }
  else
    # Either a first install, or Xcode replaced our guard with its own simctl on update. In both
    # cases the file sitting at $target now IS the real binary, and it is the newer one.
    [ -f "$target" ] || { note "no simctl at $target"; return 1; }
    mv -f "$target" "$real" || return 1
    note "moved Xcode's simctl aside to $real"
  fi
  install -m 755 "$guard" "$target" || return 1
  write_conf "$dir" /usr/bin/xcrun "$real"
}

if [ "$check_only" -eq 0 ]; then
  [ -f "$guard" ] || { echo "no guard at $guard" >&2; exit 2; }
  install_shim "$LOCALBIN" xcrun  || note "could not install $LOCALBIN/xcrun"
  install_shim "$AXBIN"    xcrun  || note "could not install $AXBIN/xcrun"
  # The slot script and this checker go in together and must stay siblings: `acquire` refuses to hand out a
  # slot unless it can run `install-boot-guard.sh --check` from its own directory. Installing them as a pair
  # is what makes that fail-closed rule safe — a copy of the slot script that has the check can only have
  # arrived here through this installer, which has just closed the guard.
  # Repairing has to be possible from the installed copy too, so the guard source goes with them.
  [ -f "$slot_sh" ] && [ "$here" != "$LOCALBIN" ] && install -m 755 "$slot_sh" "$LOCALBIN/arbos-heavy-slot.sh" 2>/dev/null
  if [ "$here" != "$LOCALBIN" ]; then
    install -m 755 "${BASH_SOURCE[0]}" "$LOCALBIN/install-boot-guard.sh" 2>/dev/null
    install -m 755 "$guard" "$LOCALBIN/xcrun-slot-guard.sh" 2>/dev/null
  fi
  install_simctl_guard || note "could not close the developer-directory route"
  note ""
fi

note "boot guard routes:"
is_guard "$LOCALBIN/xcrun" && good "$LOCALBIN/xcrun"       || bad "$LOCALBIN/xcrun is not the guard"
is_guard "$AXBIN/xcrun"    && good "$AXBIN/xcrun"          || bad "$AXBIN/xcrun is not the guard"

if [ -n "$DEVDIR" ] && is_guard "$DEVDIR/usr/bin/simctl"; then
  good "$DEVDIR/usr/bin/simctl (closes /usr/bin/xcrun and the direct call)"
  [ -f "$DEVDIR/usr/bin/simctl.real" ] && good "$DEVDIR/usr/bin/simctl.real is present" \
    || bad "$DEVDIR/usr/bin/simctl.real is missing — the guard has nothing to exec"
else
  bad "${DEVDIR:-<no developer directory>}/usr/bin/simctl is not the guard; /usr/bin/xcrun simctl boot is unguarded"
fi

# Where does xcrun itself say simctl lives? A second Xcode, or a DEVELOPER_DIR pointing elsewhere,
# makes every check above true and still leaves a boot that never meets the guard.
found="$(/usr/bin/xcrun --find simctl 2>/dev/null || echo "")"
if [ -z "$found" ]; then
  note "      (/usr/bin/xcrun --find simctl said nothing; not on a Mac, or no Xcode selected)"
elif is_guard "$found"; then
  good "/usr/bin/xcrun resolves simctl to a guarded binary ($found)"
else
  bad "/usr/bin/xcrun resolves simctl to $found, which is not the guard"
fi

if [ "$open_routes" -eq 0 ]; then
  note ""
  note "every route into a simulator boot passes the heavy-slot guard."
  exit 0
fi
note ""
note "$open_routes route(s) open. An unslotted boot can still take the box down (rule #1957)."
exit 1

#!/usr/bin/env bash
# Does the simulator boot guard refuse, and does it still let the slotted case through?
#
#   scripts/runner/boot-guard-selftest.sh      (pure bash; runs on Linux CI and on the Mac)
#
# scripts/runner/xcrun-slot-guard.sh replayed against a throwaway install: its own directory, its
# own slots directory, and fake `xcrun`/`simctl` binaries standing in for the real ones, so no case
# touches a simulator and the suite is safe to run anywhere.
#
# The obligation is two-sided and both sides need a real case. A guard that refuses everything
# passes "an unslotted boot is refused" and has broken every rig; a guard that is simply absent
# passes "a slotted boot succeeds" and protects nothing. So the unslotted cases and the slotted
# cases are both here, and the mutation pass at the bottom disables one piece of the guard at a
# time and requires the case that names that piece to fail. A case no mutation can break is a case
# that was not testing anything.
#
# Two of the assertions are about telling outcomes apart rather than about the verdict:
#
#   "a boot that fails for its own reasons is not a refusal" — the guard's text must be ABSENT when
#   the simulator itself fails. An exit code cannot carry that difference, and reading one as the
#   other is the original defect: a `2>/dev/null` hid the refusal and a failed boot read as success
#   one second after create.
#
#   every refusal case asserts the message, not only exit 75. A case satisfied by any failure would
#   still pass against a guard that refuses for the wrong reason, or by accident.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
guard="$here/xcrun-slot-guard.sh"
slot_sh="$here/arbos-heavy-slot.sh"
[ -f "$guard" ] || { echo "no guard at $guard" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
slots="$work/heavy-slots"; mkdir -p "$slots"
UDID=A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D
failures=0
RC=0; OUT=""; ENVV=()

# A throwaway install of the guard: both names, a conf beside them, and fake real binaries.
# $2 is what the fake binaries do, so one case can make the simulator itself fail.
make_install() {  # make_install <dir> [fail]
  local dir=$1 mode=${2:-ok}
  mkdir -p "$dir"
  cp "$guard" "$dir/xcrun";  chmod 755 "$dir/xcrun"
  cp "$guard" "$dir/simctl"; chmod 755 "$dir/simctl"
  if [ "$mode" = fail ]; then
    printf '#!/bin/bash\necho "Unable to boot device in current state: Booted" >&2\nexit 1\n' > "$dir/simctl.real"
    printf '#!/bin/bash\necho "Unable to boot device in current state: Booted" >&2\nexit 1\n' > "$dir/xcrun.real"
  else
    printf '#!/bin/bash\necho "FAKE BOOT $*"\n' > "$dir/simctl.real"
    printf '#!/bin/bash\necho "FAKE BOOT $*"\n' > "$dir/xcrun.real"
  fi
  chmod 755 "$dir/simctl.real" "$dir/xcrun.real"
  # The real slot script, executable, as install-boot-guard.sh puts it on the Mac. The guard only
  # binds the udid when it can execute this, so a copy that is not executable would quietly turn
  # the binding case into a no-op and the mutation below into a false catch.
  cp "$slot_sh" "$dir/arbos-heavy-slot.sh"; chmod 755 "$dir/arbos-heavy-slot.sh"
  cat > "$dir/xcrun-slot-guard.conf" <<CONF
REAL_XCRUN=$dir/xcrun.real
REAL_SIMCTL=$dir/simctl.real
SLOTS_DIR=$slots
SLOT_SH=$dir/arbos-heavy-slot.sh
CONF
}

rung() {  # rung <install> <as> <args...> → RC, OUT. Reads ENVV, then clears it.
  local inst=$1 as=$2; shift 2
  OUT=$(env ${ENVV[@]+"${ENVV[@]}"} "$inst/$as" "$@" 2>&1); RC=$?
  ENVV=()
}

# A slot this process owns: the guard allows a boot whose process tree contains the slot's owner
# pid, and the selftest is the guard's ancestor. The name's second field is the owner (the guard
# reads it with `cut -d- -f2`), which is how a real slot is named.
slot_path=""
take_slot() { slot_path="$slots/1762000000-$$-rig-af853.slot"; mkdir -p "$slot_path"; echo $$ > "$slot_path/pid"; echo rig-af853 > "$slot_path/name"; date -u +%s > "$slot_path/since"; }
drop_slot() { [ -n "$slot_path" ] && rm -rf "$slot_path"; slot_path=""; }

REFUSAL="no heavy slot is held by this process tree"

# ── the cases ───────────────────────────────────────────────────────────────────────────────────
# Each returns 0 when the guard behaved. They take the install directory so the mutation pass can
# run the very same case against a broken copy.

c_unslotted_xcrun() {   # the route that already worked, kept so the change cannot regress it
  drop_slot; rung "$1" xcrun simctl boot "$UDID"
  [ "$RC" = 75 ] && [[ "$OUT" == *"$REFUSAL"* ]] && [[ "$OUT" == *"xcrun simctl boot refused"* ]]
}

c_unslotted_simctl() {  # AF-853: /usr/bin/xcrun and a direct call both arrive here
  drop_slot; rung "$1" simctl boot "$UDID"
  [ "$RC" = 75 ] && [[ "$OUT" == *"$REFUSAL"* ]] && [[ "$OUT" == *"simctl boot refused"* ]]
}

c_slotted_xcrun() {
  take_slot; rung "$1" xcrun simctl boot "$UDID"; local rc=$RC out=$OUT; drop_slot
  [ "$rc" = 0 ] && [[ "$out" == *"FAKE BOOT"* ]]
}

c_slotted_simctl() {
  take_slot; rung "$1" simctl boot "$UDID"; local rc=$RC out=$OUT; drop_slot
  [ "$rc" = 0 ] && [[ "$out" == *"FAKE BOOT"* ]]
}

c_slot_records_udid() {  # the binding is what lets arbos-heavy-slot.sh reclaim an idle simulator
  take_slot; rung "$1" simctl boot "$UDID"
  local got; got=$(cat "$slot_path/udid" 2>/dev/null || echo ""); drop_slot
  [ "$got" = "$UDID" ]
}

c_failure_is_not_refusal() {  # install built with fake binaries that fail
  take_slot; rung "$1" simctl boot "$UDID"; local rc=$RC out=$OUT; drop_slot
  [ "$rc" = 1 ] && [[ "$out" != *"$REFUSAL"* ]] && [[ "$out" != *"rule #1957"* ]]
}

c_handover_inside_slots_dir() {
  take_slot; ENVV=("ARBOS_HEAVY_SLOT=$slot_path")
  local p=$slot_path
  # The handover, not the ancestor rule, is what must allow this: the owner pid is this process
  # either way, so the slot is renamed to an owner that is not in the tree.
  mv "$p" "$slots/1762000000-1-handover.slot"; ENVV=("ARBOS_HEAVY_SLOT=$slots/1762000000-1-handover.slot")
  rung "$1" simctl boot "$UDID"; local rc=$RC out=$OUT
  rm -rf "$slots/1762000000-1-handover.slot"; slot_path=""
  [ "$rc" = 0 ] && [[ "$out" == *"FAKE BOOT"* ]]
}

c_handover_outside_slots_dir() {  # `ARBOS_HEAVY_SLOT=/tmp` used to be one word past the cap
  drop_slot; ENVV=("ARBOS_HEAVY_SLOT=$work")
  rung "$1" simctl boot "$UDID"
  [ "$RC" = 75 ] && [[ "$OUT" == *"$REFUSAL"* ]]
}

c_actions_allowed() {   # release.yml / ios.yml take their slot in a separate step
  drop_slot; ENVV=("GITHUB_ACTIONS=true")
  rung "$1" simctl boot "$UDID"
  [ "$RC" = 0 ] && [[ "$OUT" == *"FAKE BOOT"* ]]
}

c_bootstatus_wait_guarded() {   # -b boots the device, so it is a boot
  drop_slot; rung "$1" simctl bootstatus "$UDID" -b
  [ "$RC" = 75 ] && [[ "$OUT" == *"$REFUSAL"* ]]
}

c_bootstatus_report_passes() {  # without -b it only reports; no slot needed
  drop_slot; rung "$1" simctl bootstatus "$UDID"
  [ "$RC" = 0 ] && [[ "$OUT" == *"FAKE BOOT"* ]]
}

c_list_passes() {   # everything that is not a boot goes straight through, unslotted
  drop_slot; rung "$1" simctl list devices
  [ "$RC" = 0 ] && [[ "$OUT" == *"FAKE BOOT list devices"* ]]
}

inst="$work/install";      make_install "$inst"
instf="$work/install-fail"; make_install "$instf" fail

echo "── the guard as installed ──────────────────────────────────────────────────────────────────"
report() {  # report <name> <case-fn> [install]
  local name=$1 fn=$2 target=${3:-$inst}
  if "$fn" "$target"; then printf 'ok    %s\n' "$name"
  else failures=$((failures + 1)); printf 'FAIL  %s (exit %s)\n%s\n\n' "$name" "$RC" "$OUT"; fi
}

report "an unslotted boot is refused (xcrun simctl boot)"            c_unslotted_xcrun
report "an unslotted boot is refused (simctl boot — the AF-853 route)" c_unslotted_simctl
report "a slotted boot runs (xcrun simctl boot)"                     c_slotted_xcrun
report "a slotted boot runs (simctl boot)"                           c_slotted_simctl
report "the allowing slot records the udid it booted"                c_slot_records_udid
report "a boot that fails for its own reasons is not a refusal"      c_failure_is_not_refusal "$instf"
report "a slot handed over in ARBOS_HEAVY_SLOT is honoured"          c_handover_inside_slots_dir
report "a handover pointing outside the slots directory is refused"  c_handover_outside_slots_dir
report "GitHub Actions boots without a slot of its own"              c_actions_allowed
report "bootstatus -b is a boot and is refused"                      c_bootstatus_wait_guarded
report "bootstatus without -b only reports and passes"               c_bootstatus_report_passes
report "a non-boot subcommand passes through unslotted"              c_list_passes

# ── the mutation pass ───────────────────────────────────────────────────────────────────────────
# Break one piece of the guard, run the case that names it, require that case to fail. A mutant
# that survives is the interesting result: it means the case above is decoration, and it says which
# one. A count of passing cases says nothing at all about which of them are load-bearing.
echo
echo "── mutants (each must be caught by the case named) ─────────────────────────────────────────"
survivors=0
mutations=0

mutate() {  # mutate <description> <sed-program> <case-fn> <case-name> [install-mode]
  local desc=$1 prog=$2 fn=$3 cname=$4 mode=${5:-ok}
  local m="$work/mutant.$mutations"
  mutations=$((mutations + 1))
  make_install "$m" "$mode"
  sed -i "$prog" "$m/xcrun" "$m/simctl"
  if cmp -s "$m/xcrun" "$inst/xcrun"; then
    survivors=$((survivors + 1))
    printf 'NO-OP %s — the mutation changed nothing; the sed program no longer matches the guard\n' "$desc"
    return
  fi
  if "$fn" "$m"; then
    survivors=$((survivors + 1))
    printf 'SURVIVED  %s\n          "%s" still passed, so it is not testing this. Mutant: exit %s, %s\n' \
      "$desc" "$cname" "$RC" "$(printf '%s' "${OUT:-<no output>}" | head -n 1)"
  else
    printf 'caught    %s\n          by "%s" (mutant: exit %s, %s)\n' \
      "$desc" "$cname" "$RC" "$(printf '%s' "${OUT:-<no output>}" | head -n 1)"
  fi
}

# The one the ticket is about, and the one the coordinator asked to see: with the guard disabled,
# does the unslotted boot actually go through?
mutate "the guard does not recognise 'boot' as a boot" \
  's/^  boot) is_boot=1 ;;/  boot) is_boot=0 ;;/' \
  c_unslotted_xcrun "an unslotted boot is refused (xcrun simctl boot)"

mutate "the guard does not recognise that it was invoked as simctl (the state before AF-853)" \
  's/^if \[ "\$self" = "simctl" \]; then/if [ "$self" = "simctl-no-such-name" ]; then/' \
  c_unslotted_simctl "an unslotted boot is refused (simctl boot — the AF-853 route)"

mutate "the refusal is printed nowhere (the 2>\/dev\/null defect, in the guard itself)" \
  's/^cat >&2 <<MSG/cat >\/dev\/null <<MSG/' \
  c_unslotted_xcrun "an unslotted boot is refused (xcrun simctl boot)"

mutate "the refusal exits 0, so a caller cannot see it" \
  's/^exit 75$/exit 0/' \
  c_unslotted_xcrun "an unslotted boot is refused (xcrun simctl boot)"

mutate "a held slot is never found, so every boot is refused" \
  '/^for s in "\$SLOTS_DIR"\/\*.slot; do$/,/^done$/d' \
  c_slotted_simctl "a slotted boot runs (simctl boot)"

mutate "the booted udid is never recorded on the slot" \
  's/\[ -n "\$udid" \] && \[ -x "\$SLOT_SH" \] && "\$SLOT_SH" bind "\$1" "\$udid" 2>\/dev\/null;//' \
  c_slot_records_udid "the allowing slot records the udid it booted"

mutate "any directory in ARBOS_HEAVY_SLOT is accepted again" \
  's|^  "\$SLOTS_DIR"/\*.slot) \[ -d "\$ARBOS_HEAVY_SLOT" \]|  *) [ -d "$ARBOS_HEAVY_SLOT" ]|' \
  c_handover_outside_slots_dir "a handover pointing outside the slots directory is refused"

# ── the closure itself ──────────────────────────────────────────────────────────────────────────
# Everything above tests the guard's decision. This tests the thing AF-853 is actually about: that
# install-boot-guard.sh puts that decision on the route `/usr/bin/xcrun simctl boot` takes, which no
# PATH shim can cover. A fake developer directory stands in for Xcode's, and the case that matters
# is the first one — the route is shown OPEN before the installer runs, against the same binary.
echo
echo "── the developer-directory route, before and after installing ─────────────────────────────"
inst_case() {  # inst_case <name> <test-fn>
  if "$2"; then printf 'ok    %s\n' "$1"
  else failures=$((failures + 1)); printf 'FAIL  %s (exit %s)\n%s\n\n' "$1" "$RC" "$OUT"; fi
}

dev="$work/devdir"; devbin="$dev/usr/bin"
INSTALLER="$here/install-boot-guard.sh"
reset_devdir() {
  rm -rf "$dev" "$work/localbin" "$work/axbin"; mkdir -p "$devbin"
  printf '#!/bin/bash\necho "FAKE BOOT $*"\n' > "$devbin/simctl"; chmod 755 "$devbin/simctl"
}
reset_devdir
installer_env=(ARBOS_DEVELOPER_DIR="$dev" ARBOS_LOCALBIN="$work/localbin"
               ARBOS_AXBIN="$work/axbin" ARBOS_HEAVY_SLOTS_DIR="$slots")
run_installer() { OUT=$(env "${installer_env[@]}" "$INSTALLER" ${1:+"$1"} 2>&1); RC=$?; }
boot_via_devdir() { OUT=$("$devbin/simctl" boot "$UDID" 2>&1); RC=$?; }

i_route_open_before() {   # the defect, demonstrated against the real artifact
  drop_slot; boot_via_devdir
  [ "$RC" = 0 ] && [[ "$OUT" == *"FAKE BOOT"* ]]
}
# Name the developer-directory line specifically. "Something is open" is also true before the PATH
# shims are installed, so the looser assertion passed against an installer whose developer-directory
# check had been replaced by `true` — it was reporting the shims and calling that a pass.
i_check_says_open() { run_installer --check; [ "$RC" = 1 ] && [[ "$OUT" == *"/usr/bin/xcrun simctl boot is unguarded"* ]]; }
i_install_closes_it() {
  run_installer; drop_slot; boot_via_devdir
  [ "$RC" = 75 ] && [[ "$OUT" == *"$REFUSAL"* ]]
}
i_slotted_still_boots() {  # the other half: the installer must not break the sanctioned route
  take_slot; boot_via_devdir; local rc=$RC out=$OUT; drop_slot
  [ "$rc" = 0 ] && [[ "$out" == *"FAKE BOOT"* ]]
}
i_check_says_closed() { run_installer --check; [ "$RC" = 0 ] && [[ "$OUT" == *"every route into a simulator boot passes"* ]]; }
i_install_is_idempotent() {  # a second run must not move the guard on top of the real binary
  run_installer; run_installer
  grep -q 'FAKE BOOT' "$devbin/simctl.real" 2>/dev/null && ! head -n 5 "$devbin/simctl.real" 2>/dev/null | grep -q 'arbos-boot-guard'
}
i_xcode_update_is_visible() {  # Xcode restores its own simctl: the hatch reopens silently
  printf '#!/bin/bash\necho "FAKE BOOT v2 $*"\n' > "$devbin/simctl"; chmod 755 "$devbin/simctl"
  run_installer --check
  [ "$RC" = 1 ] && [[ "$OUT" == *"unguarded"* ]]
}
i_reinstall_repairs() {   # and repairing keeps Xcode's NEW binary, not the stale one
  run_installer; drop_slot; boot_via_devdir
  [ "$RC" = 75 ] && grep -q 'FAKE BOOT v2' "$devbin/simctl.real"
}

inst_case "before installing, an unslotted boot through the developer directory succeeds" i_route_open_before
inst_case "--check reports that route open"                                  i_check_says_open
inst_case "after installing, the same unslotted boot is refused"             i_install_closes_it
inst_case "and a slotted boot through the same path still runs"              i_slotted_still_boots
inst_case "--check reports every route closed"                               i_check_says_closed
inst_case "installing twice does not bury the real simctl"                   i_install_is_idempotent
inst_case "an Xcode update that restores its own simctl is reported open"    i_xcode_update_is_visible
inst_case "re-installing repairs it and keeps Xcode's newer binary"          i_reinstall_repairs

#  …and the installer gets the same treatment, because it is the half of the fix that does the
#  closing. A guard that refuses correctly and is never put on the route protects nothing.
echo
echo "── mutants in the installer ───────────────────────────────────────────────────────────────"
mutate_installer() {  # mutate_installer <description> <sed-program> <case-fn> <case-name>
  local desc=$1 prog=$2 fn=$3 cname=$4
  local m="$work/mutant-installer.$mutations"
  mutations=$((mutations + 1))
  sed "$prog" "$here/install-boot-guard.sh" > "$m"; chmod 755 "$m"
  if cmp -s "$m" "$here/install-boot-guard.sh"; then
    survivors=$((survivors + 1))
    printf 'NO-OP %s — the mutation changed nothing; the sed program no longer matches\n' "$desc"
    return
  fi
  reset_devdir
  INSTALLER="$m"
  if "$fn"; then
    survivors=$((survivors + 1))
    printf 'SURVIVED  %s\n          "%s" still passed. Mutant: exit %s\n' "$desc" "$cname" "$RC"
  else
    printf 'caught    %s\n          by "%s" (mutant: exit %s)\n' "$desc" "$cname" "$RC"
  fi
  INSTALLER="$here/install-boot-guard.sh"
  reset_devdir
}

mutate_installer "the installer never touches the developer directory (the state before AF-853)" \
  's/^  install_simctl_guard || note/  true || note/' \
  i_install_closes_it "after installing, the same unslotted boot is refused"

mutate_installer "--check calls the developer-directory route closed whatever is there" \
  's/^if \[ -n "\$DEVDIR" \] && is_guard "\$DEVDIR\/usr\/bin\/simctl"; then/if true; then/' \
  i_check_says_open "--check reports that route open"

mutate_installer "the installer moves the guard on top of the real binary on a second run" \
  's/^  if is_guard "\$target"; then/  if false; then/' \
  i_install_is_idempotent "installing twice does not bury the real simctl"

echo
echo "$mutations mutations run, $survivors survived."
if [ "$failures" -ne 0 ] || [ "$survivors" -ne 0 ]; then
  echo "boot guard selftest FAILED: $failures case(s) failed, $survivors mutant(s) survived." >&2
  exit 1
fi
echo "boot guard selftest passed: every case holds and every mutant was caught."

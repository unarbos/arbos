#!/bin/bash
# Nightly hygiene for the EC2 Mac (runs as root from /Library/LaunchDaemons/sh.arbos.mac-clean.plist, 04:15 UTC).
# Installed copy: /usr/local/bin/arbos-mac-clean.sh — keep it in step with this file (sudo install -m 755).
#  - removes stale simulator Deleting-* dirs under /private/var/folders (every pass, even when not low)
#  - removes rig checkouts /Users/ec2-user/arbos-* idle > 30 min with no process open (workers build on their
#    own VMs; the Mac is for the gate, releases and shots) — never the gate's own trees
#  - when space is short: drops the release smoke-target cache and the gate's debug tree if nobody uses them
#  - removes cargo target/ dirs not modified for 2 days under the worker checkouts (never the protected trees)
#  - clears CoreSimulator caches and deletes unavailable simulators
#  - shuts down the gate user's lingering booted simulators (xcodebuild boots one per gate run and leaves
#    it; six were idling on 2026-10-21) — only ec2-user's, never the arbosgui GUI session's
#  - kills orphaned `rust-cache … save.js` processes (parent 1, older than 30 min: a cancelled release
#    job's post step kept tar'ing at 43 % CPU for 1 h 23 min on 2026-10-21)
#  - appends an ALERT line to the loop journal when the data volume has < 20 GB free
#  - `--if-low`: runs only when < 40 GB free (launchd sh.arbos.mac-clean-low every 30 min; the Rust gate
#    calls it at every batch start under 40 GB and when it refuses a release build under 6 GiB). The
#    20 GB alert threshold is unchanged.
# Left alone on purpose: WirelessRadioManagerd (system daemon at ~140 % CPU on this Wi-Fi-less EC2 Mac;
# shared machine, not ours to disable — see the feedback-loop runbook).
# Log: /var/log/arbos-mac-clean.log. Journal: /Users/Shared/arbos-run/loop-journal.log (also `logger -t arbos-mac-clean`).
set -u
LOG=/var/log/arbos-mac-clean.log
# `--if-low`: the opportunistic mode (launchd sh.arbos.mac-clean-low every 30 min, and the Rust gate when it
# refuses a build): do nothing unless the data volume has less than LOW_FREE_GB free.
# 15 GB (was 10): the Rust gate refuses a release build at 4 GB and a Mac release build needs ~10 GB
# of scratch, so at 10 GB the 30-min pass came too late (2026-10-23 16:50: 8 GB free, a gate build
# refused that hour while the release lane's debug tree held 36 GB).
# 40 GB (was 15): the iOS gate's xcframework cross-build (three Rust targets) plus a Mac release build
# want ~25 GB of scratch together; at 17 GB free #1466's gate died with ENOSPC (2026-10-23 20:4x). The
# Rust gate also calls this mode at every batch start, so under 40 GB a batch begins with a sweep.
LOW_FREE_GB=${LOW_FREE_GB:-40}
ts() { date -u +%FT%TZ; }
say() { printf '%s %s\n' "$(ts)" "$*" >> "$LOG"; }
# Every build target dir lives under /Users on the 1 TB data volume (2026-10-30: the release lane via
# ARBOS_SCRATCH=/Users in the runner's .env, the gate via ARBOS_GATE_TARGET_DIR in ec2-user's ~/.zshenv), so
# free space is read where the builds write. The old trees on /Volumes/ArbosScratch are not swept.
BUILD_ROOT=/Users
free_gb() { df -g "$BUILD_ROOT" | awk 'NR==2 {print $4}'; }

# CoreSimulator moves a deleted/erased device's data to $TMPDIR/Deleting-<uuid> and removes it later; on this
# saturated box "later" never comes (20 of them = 69 GB on 2026-10-23 20:5x, re-created at ~4/10 min by the
# rigs' create-shoot-delete cycle). Anything there older than 5 min is garbage by definition. Cheap: runs in
# every pass, before the --if-low early exit.
sweep_deleting() {
  local n=0 d
  while read -r d; do
    rm -rf "$d" 2>/dev/null && n=$((n + 1))
  done < <(find /private/var/folders -maxdepth 4 -type d -name 'Deleting-*' -mmin +5 2>/dev/null)
  [ "$n" -gt 0 ] && say "removed $n stale simulator Deleting-* dir(s)"
  return 0
}
sweep_deleting

# `xcodebuild test` clones the destination simulator into ~/Library/Developer/XCTestDevices ("Clone N of
# iPhone 17", 13 GB each on this box) and is meant to delete the clone afterwards; a killed or crashed test run
# leaves it behind (5 clones = 61 GB on 2026-10-25 19:1x, free space 14 GB). A clone no process has open and
# untouched for 30 min is garbage: delete it through simctl so the device set stays consistent, then its
# Deleting-* residue goes on the next sweep_deleting pass. Runs in every pass.
sweep_xctest_clones() {
  local set n=0 d u
  for set in /Users/ec2-user/Library/Developer/XCTestDevices /Users/arbosgui/Library/Developer/XCTestDevices; do
    [ -d "$set" ] || continue
    while read -r d; do
      lsof +D "$d" >/dev/null 2>&1 && continue
      u=$(basename "$d")
      sudo -u "$(stat -f %Su "$set")" xcrun simctl --set "$set" delete "$u" >/dev/null 2>&1 || rm -rf "$d" 2>/dev/null
      [ -d "$d" ] || n=$((n + 1))
    done < <(find "$set" -mindepth 1 -maxdepth 1 -type d -mmin +30 2>/dev/null)
  done
  [ "$n" -gt 0 ] && say "removed $n idle XCTest simulator clone(s)"
  return 0
}
sweep_xctest_clones

# A booted simulator keeps a few hundred daemons runnable even when nothing uses it: two rig simulators
# (one up for 70 min after its rig was done) put the box at load 340-548 on 2026-10-26 22:3x and ssh timed
# out for every worker (#1957 is the rule: rigs shut down what they booted). Backstop, every pass: shut down a
# simulator booted for more than SIM_IDLE_MIN minutes when no xcodebuild / xctest / screencapture / `simctl io`
# of its user is running and nothing names its UDID. Device sets of ec2-user and arbosgui only (ours).
SIM_IDLE_MIN=${SIM_IDLE_MIN:-30}
sweep_idle_sims() {
  local user pid udid name age busy
  for user in ec2-user arbosgui; do
    id -u "$user" >/dev/null 2>&1 || continue
    while read -r pid udid; do
      [ -n "$udid" ] || continue
      age=$(( ( $(date +%s) - $(ps -o lstart= -p "$pid" 2>/dev/null | xargs -I{} date -j -f "%a %b %d %T %Y" "{}" +%s 2>/dev/null || echo 0) ) / 60 ))
      [ "$age" -ge "$SIM_IDLE_MIN" ] || continue
      busy=$(ps -axo user=,command= | awk -v u="$user" -v d="$udid" '
        $1 == u && ($2 ~ /xcodebuild|xctest|screencapture|XCTestDevices/ || $0 ~ d) { n++ } END { print n + 0 }')
      [ "$busy" -eq 0 ] || continue
      name=$(sudo -u "$user" xcrun simctl list devices booted 2>/dev/null | grep -F "($udid)" | sed -E 's/^ *(.*) \('"$udid"'\).*/\1/' | head -n 1)
      if sudo -u "$user" xcrun simctl shutdown "$udid" >/dev/null 2>&1; then
        say "shut down idle simulator '${name:-?}' ($udid, $user): booted ${age} min, no xcodebuild/xctest/screencapture/simctl io of $user and nothing naming it (#1957 backstop)"
        logger -t arbos-mac-clean "shut down idle simulator ${name:-?} $udid ($user, ${age} min idle)"
      fi
    done < <(ps -axo user=,pid=,command= | awk -v u="$user" '$1 == u && $3 ~ /launchd_sim$/ { match($0, /Devices\/[0-9A-F-]{36}/); print $2, substr($0, RSTART + 8, 36) }')
  done
  return 0
}
sweep_idle_sims

if [ "${1:-}" = "--if-low" ]; then
  f=$(free_gb)
  [ "${f:-99}" -lt "$LOW_FREE_GB" ] || exit 0
  say "opportunistic run: ${f}G free (< ${LOW_FREE_GB}G)"
fi
JOURNAL=/Users/Shared/arbos-run/loop-journal.log
MIN_FREE_GB=${MIN_FREE_GB:-20}
pick() { echo "$BUILD_ROOT/$1/$2"; }
TARGET_AGE_DAYS=${TARGET_AGE_DAYS:-2}
PROTECT='(/Users/Shared/arbos-store-pending|/Users/Shared/arbos-tools|/Users/ec2-user/arbos-parity|/Users/ec2-user/arbos-ios|/Users/ec2-user/arbos-refresh|/Users/arbosgui/actions-runner)'

say "start free=$(free_gb)G"

# Per-worker rig checkouts under /Users/ec2-user/arbos-* (one per Cloud Agent: arbos-login, arbos-mv, …).
# Policy since 2026-10-23 23:42 (operator): workers build on their own VMs; the EC2 Mac is for the gate,
# releases and shots only. A rig checkout no process has open (pgrep -f on its path) and with no file touched
# for 30 min is removed whole — the worker's branch is on GitHub, the checkout is scratch. (Before: 24 h
# whole / 2 h artifacts; the volume hit 8 GB free at 23:33 with ~50 GB of live worker builds.) The gate's own
# trees and the protected list above are never touched.
WORKER_KEEP='^(arbos-gate|arbos-rust-gate|arbos-rust-gate-target|arbos-rust-gate-target-smoke|arbos-rust-gate-runs|arbos-gate-dd-Release|arbos-gate-dd-Debug|arbos-parity|arbos-ios|arbos-refresh)$'
WORKER_IDLE_MIN=${WORKER_IDLE_MIN:-30}
for d in /Users/ec2-user/arbos-*; do
  [ -d "$d" ] || continue
  b=$(basename "$d")
  echo "$b" | grep -qE "$WORKER_KEEP" && continue
  pgrep -f "$d" >/dev/null 2>&1 && continue
  [ -z "$(find "$d" -type f -mmin -"$WORKER_IDLE_MIN" -print -quit 2>/dev/null)" ] || continue
  gb=$(du -xsg "$d" 2>/dev/null | cut -f1)
  rm -rf "$d" && say "removed rig checkout $b (${gb:-?} GB, idle > ${WORKER_IDLE_MIN} min, no process)"
done

# A build in flight owns its target dir: skip cargo targets while cargo/rustc/xcodebuild run.
if pgrep -qx cargo || pgrep -qx rustc; then
  say "cargo/rustc running; skipping target/ cleanup this run"
else
  find /Users/ec2-user /Users/arbosgui /tmp -maxdepth 4 -type d -name target -not -path '*/node_modules/*' 2>/dev/null \
    | grep -Ev "$PROTECT" | while read -r t; do
      [ -f "$(dirname "$t")/Cargo.toml" ] || continue
      # idle = no file inside touched within TARGET_AGE_DAYS (the dir's own mtime rarely changes)
      [ -z "$(find "$t" -type f -mtime -"$TARGET_AGE_DAYS" -print -quit 2>/dev/null)" ] || continue
      mib=$(du -sm "$t" 2>/dev/null | cut -f1)
      rm -rf "$t" && say "removed $t (${mib} MiB, idle > ${TARGET_AGE_DAYS}d)"
    done
fi

rm -rf /Library/Developer/CoreSimulator/Caches/* /Users/ec2-user/Library/Developer/CoreSimulator/Caches/* 2>/dev/null
sudo -u ec2-user xcrun simctl delete unavailable >/dev/null 2>&1 && say "simctl delete unavailable ok"

# Stale per-build artifacts that cargo never removes (2026-10-22 18:2x: 7.7 GiB free with the gate floor at 6):
#  - the iOS gate's xcframework build leaves one 0.2-0.3 GB libarbos_ffi-<hash>.a per run and target under
#    */release/deps/ (77 of them = 13 GB); only the newest per directory can still be linked;
#  - rustc's deps/rustcXXXXXX scratch dirs survive a killed compile (0.7 GB each).
# Runs in every pass (nightly and --if-low); skips files younger than 30 min so a build in flight is untouched.
find /Users/ec2-user /Users/arbosgui -maxdepth 6 -type d -path "*/release/deps" -not -path "*/node_modules/*" 2>/dev/null | while read -r deps; do
  newest=$(ls -t "$deps"/libarbos_ffi-*.a 2>/dev/null | head -n 1)
  find "$deps" -maxdepth 1 -name "libarbos_ffi-*.a" -mmin +30 2>/dev/null | while read -r f; do
    [ "$f" = "$newest" ] && continue
    mib=$(du -sm "$f" 2>/dev/null | cut -f1); rm -f "$f" && say "removed stale $f (${mib} MiB)"
  done
done
find /Users/ec2-user /Users/arbosgui -maxdepth 7 -type d -path "*/deps/rustc*" -mmin +30 2>/dev/null | while read -r d; do
  mib=$(du -sm "$d" 2>/dev/null | cut -f1); rm -rf "$d" && say "removed rustc leftover $d (${mib} MiB)"
done

# Lingering booted simulators of the gate user. Skipped while an xcodebuild runs (a gate or rig owns them).
if pgrep -qx xcodebuild; then
  say "xcodebuild running; leaving booted simulators alone"
else
  booted=$(sudo -u ec2-user xcrun simctl list devices booted 2>/dev/null | grep -oE '[0-9A-F-]{36}' | sort -u)
  n=0
  for u in $booted; do
    sudo -u ec2-user xcrun simctl shutdown "$u" >/dev/null 2>&1 && n=$((n + 1))
  done
  [ "$n" -gt 0 ] && say "shut down $n lingering booted simulator(s) of ec2-user"
fi

# Orphaned Actions-cache savers: a cancelled release job's `Post Run Swatinem/rust-cache` keeps running
# (parent 1) long after the job; kill those older than 30 minutes.
for pid in $(pgrep -f 'rust-cache/v2/dist/save.js'); do
  ppid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
  secs=$(ps -o etime= -p "$pid" 2>/dev/null | awk -F'[-:]' '{ if (NF==4) print $1*86400+$2*3600+$3*60+$4; else if (NF==3) print $1*3600+$2*60+$3; else print $1*60+$2 }')
  if [ "${ppid:-0}" = "1" ] && [ "${secs:-0}" -gt 1800 ]; then
    kill "$pid" 2>/dev/null && say "killed orphaned rust-cache save.js pid $pid (running ${secs}s, parent 1)"
  fi
done
# Xcode DerivedData (per-project folders) only when space is short, only folders with no file touched
# for 7 days, never while xcodebuild runs.
if [ "$(free_gb)" -lt "$MIN_FREE_GB" ] && ! pgrep -qx xcodebuild; then
  find /Users/ec2-user/Library/Developer/Xcode/DerivedData -mindepth 1 -maxdepth 1 -type d 2>/dev/null | while read -r d; do
    [ -z "$(find "$d" -type f -mtime -7 -print -quit 2>/dev/null)" ] || continue
    rm -rf "$d" && say "removed DerivedData $(basename "$d")"
  done
fi

# The release lane's smoke step (`cargo test -p arbos-mac enter_tests`, #1215) builds the debug profile
# into the persistent release target dir; its debug/ tree reached 34 GB in one day (2026-10-23 15:20,
# 15 GB free on the volume). The release/ and per-arch trees are what keeps releases warm; debug/ is only
# the smoke test's cache, rebuilt in a few minutes. Dropped when space is short and no cargo runs.
# Two warm caches that are cheap to rebuild, dropped when space is short and nobody is using them (operator,
# 2026-10-23 23:42): the release lane's smoke-test target (rebuilt by the next release's smoke step, ~4 min)
# and the gate's debug tree (the uniffi-bindgen / xcframework toolchain the iOS half builds, ~5 min).
SMOKE_TARGET=$(pick arbosgui arbos-release-smoke-target)
if [ "$(free_gb)" -lt "$MIN_FREE_GB" ] && [ -d "$SMOKE_TARGET" ] && ! pgrep -u arbosgui -qx cargo && ! pgrep -u arbosgui -qx rustc; then
  sz=$(du -sg "$SMOKE_TARGET" 2>/dev/null | cut -f1)
  rm -rf "$SMOKE_TARGET" && say "removed the release lane's smoke target cache (${sz:-?} GB; the next release smoke rebuilds it)"
fi
GATE_DEBUG=$(pick ec2-user arbos-rust-gate-target)/debug
if [ "$(free_gb)" -lt "$MIN_FREE_GB" ] && [ -d "$GATE_DEBUG" ] && ! pgrep -u ec2-user -qx cargo && ! pgrep -u ec2-user -qx rustc && ! pgrep -u ec2-user -qx xcodebuild; then
  sz=$(du -sg "$GATE_DEBUG" 2>/dev/null | cut -f1)
  rm -rf "$GATE_DEBUG" && say "removed the gate's debug tree (${sz:-?} GB; the next iOS half rebuilds the uniffi toolchain)"
fi
RELEASE_DEBUG=$(pick arbosgui arbos-release-target)/debug
# Only the release lane's own cargo (user arbosgui) can hold this tree; the gate's builds run as
# ec2-user in other target dirs and must not keep it alive (they almost always run).
if [ "$(free_gb)" -lt "$MIN_FREE_GB" ] && [ -d "$RELEASE_DEBUG" ] && ! pgrep -u arbosgui -qx cargo && ! pgrep -u arbosgui -qx rustc; then
  sz=$(du -sg "$RELEASE_DEBUG" 2>/dev/null | cut -f1)
  rm -rf "$RELEASE_DEBUG" && say "removed the release lane's debug target tree (${sz:-?} GB; the smoke test rebuilds it)"
fi

FREE=$(free_gb)
say "done free=${FREE}G"
if [ "$FREE" -lt "$MIN_FREE_GB" ]; then
  msg="ALERT $(ts) EC2 Mac data volume has ${FREE} GB free (< ${MIN_FREE_GB} GB) after nightly cleanup — builds/simctl will start failing; prune /Users/*/arbos*/target, simulators, DerivedData"
  mkdir -p "$(dirname "$JOURNAL")"; printf '%s\n' "$msg" >> "$JOURNAL"; chmod 666 "$JOURNAL" 2>/dev/null
  logger -t arbos-mac-clean "$msg"
  say "$msg"
fi

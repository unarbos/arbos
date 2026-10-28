#!/bin/bash
# Mac half of the Rust pre-merge gate, run on the EC2 Mac by the feedback poller over ssh. Asynchronous,
# because a release build of arbos-mac outlives a 2-minute tick:
#   rust-gate.sh start-batch <run-id> <tree-sha> <release-mac: 0|1> [full]
#   rust-gate.sh status <run-id>   -> first line running|ok|fail|infra, then "== …" summary and diagnostics
#   rust-gate.sh abort  <run-id>
# The poller builds the integration tree (current main + every ready branch) as the branch `gate/<run-id>` on
# GitHub; CI there is the Linux half (fmt --check, clippy -D warnings, test). This script fetches that tree
# into ~/arbos-rust-gate (its own clone; ~/arbos-gate belongs to the iOS gate) and runs the one thing a Linux
# runner cannot: `cargo build --release --locked -p arbos-mac`. With `full` it also runs fmt/clippy/test
# (fallback when CI is unavailable). The cargo build directory is the gate's own (~/arbos-rust-gate-target),
# shared with no other session. One run at a time; the poller serialises.
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
# The cargo target lives on the Mac mini's internal SSD (/Volumes/ArbosScratch, #1846: rebuildable caches
# only — the volume survives reboots, not a release of the EC2 dedicated host) when it is mounted; the
# old home-directory tree is the fallback. One variable: ARBOS_GATE_TARGET_DIR.
ARBOS_SCRATCH=${ARBOS_SCRATCH:-/Volumes/ArbosScratch}
if [ -d "$ARBOS_SCRATCH/ec2-user" ]; then
  export CARGO_TARGET_DIR="${ARBOS_GATE_TARGET_DIR:-$ARBOS_SCRATCH/ec2-user/arbos-rust-gate-target}"
else
  export CARGO_TARGET_DIR="${ARBOS_GATE_TARGET_DIR:-$HOME/arbos-rust-gate-target}"
fi
runs=$HOME/arbos-rust-gate-runs
repo=$HOME/arbos-rust-gate
mkdir -p "$runs"
cmd=${1:-}; shift || true

disk_floor_check() {
  # A release build of arbos-mac needs several GiB of scratch; below ~2 GiB free it fails half-way and
  # takes the shared runner down with it (2026-10-22 01:55: 3.8 GiB free, four concurrent release builds).
  # Under PRUNE_FREE_GB (40) the batch starts with the lock-aware sweep (stale simulator Deleting-* dirs,
  # idle worker checkouts) and re-reads free space — #1466's iOS half hit ENOSPC at 17 GB free on
  # 2026-10-23 20:4x. Under MIN_FREE_GB the run is refused as an infrastructure error (the poller retries
  # next tick) and the loop journal gets an ALERT line.
  local free
  free=$(df -g /System/Volumes/Data 2>/dev/null | awk 'NR==2 {print $4}')
  [ -n "$free" ] || return 0
  if [ "$free" -lt "${PRUNE_FREE_GB:-40}" ]; then
    echo "== disk: ${free} GiB free on the Mac (< ${PRUNE_FREE_GB:-40}); running arbos-mac-clean.sh --if-low first"
    sudo -n /bin/bash /usr/local/bin/arbos-mac-clean.sh --if-low >/dev/null 2>&1 || true
    free=$(df -g /System/Volumes/Data 2>/dev/null | awk 'NR==2 {print $4}')
    echo "== disk: ${free} GiB free after the sweep"
  fi
  if [ "$free" -lt "${MIN_FREE_GB:-6}" ]; then
    echo "== disk: ${free} GiB free on the Mac (< ${MIN_FREE_GB:-6}); refusing to start the release build"
    msg="ALERT $(date -u +%FT%TZ) EC2 Mac has ${free} GiB free (< ${MIN_FREE_GB:-6}); the Rust gate refused a release build — prune target dirs (arbos-mac-clean.sh --if-low runs every 30 min)"
    mkdir -p /Users/Shared/arbos-run 2>/dev/null; printf '%s\n' "$msg" >> /Users/Shared/arbos-run/loop-journal.log 2>/dev/null
    logger -t arbos-rust-gate "$msg" 2>/dev/null
    sudo -n /bin/bash /usr/local/bin/arbos-mac-clean.sh --if-low >/dev/null 2>&1 || true
    return 1
  fi
  return 0
}

webrtc_scratch_check() {
  # webrtc-sys (livekit) downloads libwebrtc into a directory the `scratch` crate bakes in at ITS compile time.
  # When the gate's target dir moved (in-tree ~/arbos-rust-gate/target → ~/arbos-rust-gate-target) the cached
  # webrtc-sys build still pointed at the old, cleaned path, and every tree that recompiled webrtc-sys failed with
  # "could not find native static library `webrtc`" (#1241, 00:45 and 00:51 Oct 22). Before the release build:
  # if the baked link path is gone but the same libwebrtc build exists under the current target dir, point
  # LK_CUSTOM_WEBRTC at it; otherwise rebuild scratch + webrtc-sys (one download, ~100 MB).
  local out stale hash found
  for out in "$CARGO_TARGET_DIR"/release/build/webrtc-sys-*/output; do
    [ -f "$out" ] || continue
    stale=$(sed -n 's/^cargo:rustc-link-search=native=\(.*\)$/\1/p' "$out" | grep -m1 'livekit_webrtc')
    [ -n "$stale" ] || continue
    case "$stale" in "$CARGO_TARGET_DIR"/*) continue ;; esac   # baked path is inside the gate target: fine
    hash=$(printf '%s' "$stale" | sed -n 's/.*release-webrtc-\([0-9a-f]*\).*/\1/p')
    found=$(ls -d "$CARGO_TARGET_DIR"/release/build/scratch-*/out/livekit_webrtc/livekit/mac-arm64-release-webrtc-"${hash:-none}"/mac-arm64-release 2>/dev/null | head -n 1)
    if [ -n "$found" ] && [ -d "$found/lib" ]; then
      export LK_CUSTOM_WEBRTC="$found"
      echo "== webrtc-sys: baked libwebrtc path is outside the gate target; LK_CUSTOM_WEBRTC=$found"
    elif [ -d "$stale" ]; then
      echo "== webrtc-sys: baked libwebrtc path is outside the gate target but present ($stale); leaving it"
    else
      echo "== webrtc-sys: baked libwebrtc path is gone and no matching build found; rebuilding scratch + webrtc-sys"
      cargo clean --release -p scratch -p webrtc-sys 2>/dev/null || true
    fi
    return 0
  done
}

launch() {
  local id=$1; shift
  local log=$runs/$id.log rc=$runs/$id.rc
  rm -f "$log" "$rc"
  # Not nohup: BSD nohup exits with "can't detach from console" when ssh gives it no tty. Job control
  # gives the run its own process group, so `abort` can kill cargo and every rustc under it.
  set -m
  # nice: the gate shares the 8 cores with the arbosgui Actions runner (Mac release + TestFlight builds).
  # Both at once put the box at load 88–280 (2026-10-25 19:0x). The gate yields CPU to the release jobs:
  # a gate verdict a few minutes later costs nothing, a Mac release or TF build a few minutes later does.
  # CARGO_BUILD_JOBS=4: the release job on the same box builds with 4 too (release.yml); two -j8 builds
  # plus a rig's simulator boot put the 8-core Mac at load 263 on 2026-10-26 and ssh timed out.
  (trap '' HUP; export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"; exec nice -n 12 bash "$0" _run "$id" "$@") >"$log" 2>&1 </dev/null &
  echo "$!" >"$runs/$id.pid"
  disown
  set +m
  echo started
}

case "$cmd" in
  start-batch)
    # 5th arg `priority` (a batch with a pinned member): the heavy-job cap lets it take the extra slot.
    launch "$1" "$2" "${3:-1}" "${4:-mac}" "${5:-normal}"
    ;;
  start-ios)
    # The iOS half (ios-gate.sh <branch> <sha>) as a background run like the Mac half, so the poller
    # tick is never blocked on an Xcode build (a 10-member batch held one tick for 13 min, 2026-10-23
    # 17:51-18:04, and every push / merge / launch waited behind it).
    launch "$1" _ios "$2" "$3" "${4:-normal}"
    ;;
  status)
    id=$1; log=$runs/$id.log; rc=$runs/$id.rc
    if [ ! -f "$log" ]; then echo infra; echo "no run $id"; exit 0; fi
    if [ ! -f "$rc" ]; then echo running; tail -n 3 "$log"; exit 0; fi
    case "$(cat "$rc")" in 0) echo ok;; 1) echo fail;; *) echo infra;; esac
    # Summary lines, then compiler/fmt/test diagnostics only; paths relative to the repo.
    grep -E "^== " "$log" | head -n 60
    grep -E "^(error|warning)(\[|:)|^\s+--> |^Diff in |^test .* FAILED|^failures:|panicked at|: error: |\*\* BUILD FAILED" "$log" \
      | sed "s#$repo/##g;s#$HOME/arbos-gate/##g" | head -n 150
    # A clean log has no diagnostics: grep exits 1 and, as the last command, made `status` exit 1 - the
    # poller took that for "Mac unreachable" and the iOS half never settled (19:15-19:30 Oct 23).
    exit 0
    ;;
  abort)
    id=$1
    if [ -f "$runs/$id.pid" ]; then
      kill -TERM -- "-$(cat "$runs/$id.pid")" 2>/dev/null
      sleep 2
      kill -KILL -- "-$(cat "$runs/$id.pid")" 2>/dev/null
    fi
    echo 2 >"$runs/$id.rc"
    echo aborted
    ;;
  _run)
    id=$1; tree=$2; mac=$3; mode=$4; priority=${5:-normal}
    rc=$runs/$id.rc
    # Heavy-job cap (#1839): at most two of {gate Mac half, gate iOS half, TF archive, Mac release} run at
    # once on this box; the rest wait in arbos-heavy-slot.sh. A pinned batch has priority (extra slot).
    slot=none
    take_slot() {
      local flag=""; [ "$priority" = "priority" ] && flag=--priority
      if [ -x /usr/local/bin/arbos-heavy-slot.sh ]; then
        # Waiting counts against the poller's gate timeouts (IOS_GATE_TIMEOUT_S 2700 / RUST_GATE_TIMEOUT_S
        # 3600 on the host): a gate half waits at most 15 min for a slot, then runs uncapped.
        slot=$(ARBOS_HEAVY_WAIT_MAX_S=900 /usr/local/bin/arbos-heavy-slot.sh acquire "$1" --pid $$ $flag 2>>"$runs/$id.log")
      fi
    }
    finish() { /usr/local/bin/arbos-heavy-slot.sh release "$slot" 2>/dev/null; echo "$1" >"$rc"; exit "$1"; }
    if [ "$tree" = "_ios" ]; then
      # start-ios: $mac = branch, $mode = sha. ios-gate.sh exits 0 ok / 1 fail / 2 infrastructure.
      take_slot "gate-ios-$id"
      echo "== ios-gate $mac $mode"
      bash "$HOME/arbos-gate/ios-gate.sh" "$mac" "$mode"; s=$?
      echo "== done rc=$s"
      finish "$s"
    fi
    take_slot "gate-mac-$id"
    cd "$repo" || finish 2
    git merge --abort 2>/dev/null
    git checkout -q -f --detach HEAD 2>/dev/null
    git fetch -q origin "+refs/heads/main:refs/remotes/origin/main" "+refs/heads/gate/*:refs/remotes/origin/gate/*" \
      || { echo "fetch failed"; finish 2; }
    git checkout -q -f --detach "$tree" || { echo "checkout of ${tree} failed"; finish 2; }
    echo "== tree $(git rev-parse HEAD)"
    status=0
    if [ "$mode" = "full" ]; then
      echo "== cargo fmt --all --check"
      cargo fmt --all --check || { echo "== fmt: FAILS"; status=1; }
      echo "== cargo clippy --workspace --all-targets --locked -- -D warnings"
      if cargo clippy --workspace --all-targets --locked -- -D warnings; then echo "== clippy: passes"; else echo "== clippy: FAILS"; status=1; fi
      if [ "$status" -eq 0 ]; then
        echo "== cargo test --workspace --locked"
        if cargo test --workspace --locked; then echo "== test: passes"; else echo "== test: FAILS"; status=1; fi
      fi
    fi
    if [ "$status" -eq 0 ] && [ "$mac" = "1" ]; then
      disk_floor_check || finish 2
      webrtc_scratch_check
      echo "== cargo build --release --locked -p arbos-mac"
      if cargo build --release --locked -p arbos-mac; then echo "== release build: passes"; else echo "== release build: FAILS"; status=1; fi
      # The same smoke the Release job runs ("Composer type-and-Enter tests", release.yml): Mac-only test code that
      # Linux CI never compiles. #2089 (54bc19da) passed the gate and then broke every Mac release from 06:58 to
      # 08:1x Oct 28 in exactly this step. Debug profile, own small target dir (the release trees stay warm).
      if [ "$status" -eq 0 ]; then
        echo "== cargo test -p arbos-mac --locked enter_tests (release smoke)"
        if CARGO_TARGET_DIR="${CARGO_TARGET_DIR}-smoke" cargo test -p arbos-mac --locked enter_tests; then echo "== release smoke: passes"; else echo "== release smoke: FAILS"; status=1; fi
      fi
    fi
    echo "== done rc=$status"
    finish "$status"
    ;;
  *)
    echo "usage: rust-gate.sh start-batch <id> <tree-sha> <0|1> [full] | status <id> | abort <id>" >&2
    exit 2
    ;;
esac

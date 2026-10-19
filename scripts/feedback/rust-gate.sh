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
export CARGO_TARGET_DIR="$HOME/arbos-rust-gate-target"
runs=$HOME/arbos-rust-gate-runs
repo=$HOME/arbos-rust-gate
mkdir -p "$runs"
cmd=${1:-}; shift || true

launch() {
  local id=$1; shift
  local log=$runs/$id.log rc=$runs/$id.rc
  rm -f "$log" "$rc"
  # Not nohup: BSD nohup exits with "can't detach from console" when ssh gives it no tty. Job control
  # gives the run its own process group, so `abort` can kill cargo and every rustc under it.
  set -m
  (trap '' HUP; exec bash "$0" _run "$id" "$@") >"$log" 2>&1 </dev/null &
  echo "$!" >"$runs/$id.pid"
  disown
  set +m
  echo started
}

case "$cmd" in
  start-batch)
    launch "$1" "$2" "${3:-1}" "${4:-mac}"
    ;;
  status)
    id=$1; log=$runs/$id.log; rc=$runs/$id.rc
    if [ ! -f "$log" ]; then echo infra; echo "no run $id"; exit 0; fi
    if [ ! -f "$rc" ]; then echo running; tail -n 3 "$log"; exit 0; fi
    case "$(cat "$rc")" in 0) echo ok;; 1) echo fail;; *) echo infra;; esac
    # Summary lines, then compiler/fmt/test diagnostics only; paths relative to the repo.
    grep -E "^== " "$log" | head -n 60
    grep -E "^(error|warning)(\[|:)|^\s+--> |^Diff in |^test .* FAILED|^failures:|panicked at" "$log" \
      | sed "s#$repo/##g" | head -n 150
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
    id=$1; tree=$2; mac=$3; mode=$4
    rc=$runs/$id.rc
    finish() { echo "$1" >"$rc"; exit "$1"; }
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
      echo "== cargo build --release --locked -p arbos-mac"
      if cargo build --release --locked -p arbos-mac; then echo "== release build: passes"; else echo "== release build: FAILS"; status=1; fi
    fi
    echo "== done rc=$status"
    finish "$status"
    ;;
  *)
    echo "usage: rust-gate.sh start-batch <id> <tree-sha> <0|1> [full] | status <id> | abort <id>" >&2
    exit 2
    ;;
esac

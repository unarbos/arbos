#!/bin/bash
# iOS pre-merge gate, run on the EC2 Mac by the feedback poller over ssh:
#   ios-gate.sh <branch> <sha>
# Fetches the branch into ~/arbos-gate (its own clone, never the audit clone), checks out <sha>
# detached, and builds the iPhone app twice (Release, then Debug; generic simulator, no signing). Prints the
# compiler errors; exit 0 = both compile, 1 = one does not, 2 = infrastructure trouble (fetch/checkout).
set -uo pipefail
branch=$1; sha=$2
# xcodebuild boots a simulator to resolve the generic destination and leaves it running; six of them
# (launchd_sim + assetsd + siriknowledged each) were idling on the EC2 Mac on 2026-10-21 while the
# release build fought for the 8 cores. Shut down at exit every simulator this run booted — only those,
# so a rig that keeps its own simulator up (arbos-ios, arbos-parity) is left alone.
booted_sims() { xcrun simctl list devices booted 2>/dev/null | grep -oE '[0-9A-F-]{36}' | sort -u; }
SIMS_BEFORE=$(booted_sims)
teardown_sims() {
  local u
  for u in $(booted_sims); do
    grep -qx "$u" <<< "$SIMS_BEFORE" || { xcrun simctl shutdown "$u" >/dev/null 2>&1 && echo "== simulator $u shut down (booted by this run)"; }
  done
}
teardown() {
  teardown_sims
  # Each run leaves a fresh libarbos_ffi-<hash>.a per iOS target under target/*/release/deps (0.2-0.3 GB each;
  # 77 of them filled 13 GB on 2026-10-22). Keep the newest two per directory, drop the rest.
  local deps
  for deps in "$HOME"/arbos-gate/target/*/release/deps; do
    [ -d "$deps" ] || continue
    ls -t "$deps"/libarbos_ffi-*.a 2>/dev/null | tail -n +3 | xargs -r rm -f
  done
}
trap teardown EXIT
cd ~/arbos-gate || exit 2
ROOT=$(pwd)
git fetch -q origin "+refs/heads/${branch}:refs/remotes/origin/${branch}" || { echo "fetch of ${branch} failed"; exit 2; }
git checkout -q --detach "$sha" || { echo "checkout of ${sha} failed"; exit 2; }
# Arbos depends on the local packages/ArbosFFI product, which links the gitignored
# ArbosFFIRust.xcframework from scripts/build-xcframework.sh. Build it before xcodebuild.
if [[ -x scripts/build-xcframework.sh ]]; then
  echo "== xcframework: building ArbosFFI for ${sha}"
  ./scripts/build-xcframework.sh || { echo "== xcframework: FAILS"; exit 1; }
  PKG_XC=packages/ArbosFFI/ArbosFFIRust.xcframework
  PKG_SWIFT=packages/ArbosFFI/Sources/ArbosFFI/ArbosFFI.swift
  if [[ ! -d "$PKG_XC" ]]; then
    echo "== xcframework: FAILS (missing ${PKG_XC})"
    exit 1
  fi
  if [[ ! -f "$PKG_SWIFT" ]]; then
    echo "== xcframework: FAILS (missing ${PKG_SWIFT})"
    exit 1
  fi
  # Generic simulator destinations need both arm64 and x86_64 in the sim slice.
  # Match any .a under a simulator library identifier (basename can vary).
  SIM_LIB=""
  while IFS= read -r f; do
    case "$f" in
      *simulator*|*ios-arm64_x86_64*|*ios-x86_64_arm64*) SIM_LIB=$f; break ;;
    esac
  done < <(find "$PKG_XC" -name '*.a' 2>/dev/null)
  if [[ -z "$SIM_LIB" || ! -f "$SIM_LIB" ]]; then
    echo "== xcframework: FAILS (no simulator .a inside ${PKG_XC})"
    find "$PKG_XC" -type f 2>/dev/null | head -40
    exit 1
  fi
  arches=$(lipo -archs "$SIM_LIB" 2>/dev/null || true)
  echo "== xcframework: simulator lib arches: ${arches} (${SIM_LIB#"$PKG_XC"/})"
  case " $arches " in
    *" x86_64 "*) ;;
    *) echo "== xcframework: FAILS (simulator library missing x86_64; got: ${arches})"; exit 1 ;;
  esac
  case " $arches " in
    *" arm64 "*) ;;
    *) echo "== xcframework: FAILS (simulator library missing arm64; got: ${arches})"; exit 1 ;;
  esac
  echo "== xcframework: ok"
fi
cd apps/ios/Arbos || exit 2
# TestFlight archives the Release configuration: a `#if DEBUG` type used unconditionally compiles in
# Debug and breaks the TestFlight job (2026-10-19, 6d64c2ee). Build Release first (that is what ships),
# then Debug (the simulator rig and the fixtures run Debug). Separate derived data keeps both incremental.
rc=0
for config in Release Debug; do
  log=$(mktemp -t ios-gate)
  xcodebuild -project Arbos.xcodeproj -scheme Arbos -configuration "$config" \
    -destination 'generic/platform=iOS Simulator' -derivedDataPath "$HOME/arbos-gate-dd-$config" \
    CODE_SIGNING_ALLOWED=NO build >"$log" 2>&1
  crc=$?
  echo "== $config: $([ "$crc" -eq 0 ] && echo compiles || echo FAILS)"
  grep -E "error:|BUILD (SUCCEEDED|FAILED)|note: (building|ArbosFFI)" "$log" | grep -v "warning:" | sed 's#/Users/[^/]*/arbos-gate/##' | head -40
  if [[ "$crc" -ne 0 ]]; then
    # Cargo spam from a runaway rebuild can bury the real error; show the tail too.
    echo "-- $config log tail --"
    tail -30 "$log" | sed 's#/Users/[^/]*/arbos-gate/##'
  fi
  rm -f "$log"
  if [[ "$crc" -eq 0 ]]; then
    fw="$HOME/arbos-gate-dd-$config/Build/Products/$config-iphonesimulator/Arbos.app/Frameworks/ArbosFFI.framework"
    if [[ ! -d "$fw" ]]; then
      echo "== $config: ArbosFFI.framework not embedded"
      rc=1
    fi
  else
    rc=1
  fi
done
# Unit-test target (ArbosTests) must still compile: the app build above does not compile tests, so
# main's test target was broken from 4b193bfc (AgentIdentityTests missing a new `silenced:` argument)
# without the gate noticing (2026-10-24 06:5x). build-for-testing compiles the tests without running
# them (no simulator boot, ~1 min incremental). A failure that already exists on main (the base) is
# reported but does not blame the branch — the owner of the test fixes it; the branch is not held hostage.
if [[ "$rc" -eq 0 ]]; then
  log=$(mktemp -t ios-gate)
  xcodebuild -project Arbos.xcodeproj -scheme Arbos -configuration Debug \
    -destination 'generic/platform=iOS Simulator' -derivedDataPath "$HOME/arbos-gate-dd-Debug" \
    CODE_SIGNING_ALLOWED=NO build-for-testing >"$log" 2>&1
  trc=$?
  if [[ "$trc" -eq 0 ]]; then
    echo "== tests: compile"
  else
    echo "== tests: FAIL to compile (ArbosTests)"
    grep -E "error:" "$log" | grep -v "warning:" | sed 's#/Users/[^/]*/arbos-gate/##' | head -20
    base_broken=0
    GATE_BASE_SHA=${GATE_BASE_SHA:-$(git -C "$ROOT" rev-parse origin/main 2>/dev/null || true)}
    if [[ -n "${GATE_BASE_SHA:-}" ]] && git -C "$ROOT" cat-file -e "$GATE_BASE_SHA^{commit}" 2>/dev/null; then
      # Does main alone fail the same way? Then it is not this branch's regression.
      cur=$(git -C "$ROOT" rev-parse HEAD)
      git -C "$ROOT" checkout -q "$GATE_BASE_SHA" 2>/dev/null && {
        xcodebuild -project Arbos.xcodeproj -scheme Arbos -configuration Debug \
          -destination 'generic/platform=iOS Simulator' -derivedDataPath "$HOME/arbos-gate-dd-Debug" \
          CODE_SIGNING_ALLOWED=NO build-for-testing >/dev/null 2>&1 || base_broken=1
        git -C "$ROOT" checkout -q "$cur" 2>/dev/null
      }
    fi
    if [[ "$base_broken" -eq 1 ]]; then
      echo "== tests: the same failure exists on main (${GATE_BASE_SHA:0:8}); not this branch's regression — reported, not blamed"
    else
      rc=1
    fi
  fi
  rm -f "$log"
fi
exit $rc

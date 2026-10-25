#!/bin/bash
# iOS pre-merge gate, run on the EC2 Mac by the feedback poller over ssh:
#   ios-gate.sh <branch> <sha>
# Fetches the branch into ~/arbos-gate (its own clone, never the audit clone), checks out <sha>
# detached, and builds the iPhone app twice (Release, then Debug; generic simulator, no signing). Prints the
# compiler errors; exit 0 = both compile, 1 = one does not, 2 = infrastructure trouble (fetch/checkout).
set -uo pipefail

# The embedded ArbosFFI framework is current when it carries the metadata symbol of the newest struct
# the generated ArbosFFI.swift declares (#1811: a stale framework made the Debug product die in dyld).
# No pipe into `grep -q`: under pipefail, grep closing the pipe on its first match kills nm with SIGPIPE
# (141) and the pipeline reads as "missing" — that failed every iOS gate 22:12–22:27 Oct 25. Sets FFI_NEWEST.
ffi_framework_current() {
  local bin=$1 swift=$2 syms
  FFI_NEWEST=$(grep -oE '^public struct [A-Za-z0-9_]+' "$swift" | grep -v FfiConverter | tail -n 1 | awk '{print $3}')
  [[ -n "$FFI_NEWEST" ]] || return 0
  syms=$(nm -gU "$bin" 2>/dev/null) || return 1
  [[ "$syms" == *"8ArbosFFI"*"${FFI_NEWEST}VMn"* ]] || grep -qE "8ArbosFFI[0-9]*${FFI_NEWEST}VMn" <<< "$syms"
}

# test_error_set <xcodebuild log> → one normalized error per line: "<path relative to the tree>: <message>"
# (line:column dropped so an unrelated edit above the error does not make it "new"); sorted, unique.
test_error_set() {
  grep -E "^[^ ].*: error: " "$1" 2>/dev/null | grep -v "warning:" \
    | sed -E 's#/Users/[^/]*/arbos-gate/##; s#^([^:]+):[0-9]+:[0-9]+: error: #\1: #' | sort -u
}
# new_test_errors <branch set> <base set> → the branch's errors the base does not have.
new_test_errors() {
  comm -23 <(printf '%s\n' "$1" | sed '/^$/d' | sort -u) <(printf '%s\n' "$2" | sed '/^$/d' | sort -u)
}

# `ios-gate.sh selftest`: run the staleness check against the gate's current Debug product and tree
# (prints the newest struct and the verdict; exit 0 current, 1 stale, 2 nothing to check).
if [[ "${1:-}" == "selftest" ]]; then
  bin=$HOME/arbos-gate-dd-Debug/Build/Products/Debug-iphonesimulator/Arbos.app/Frameworks/ArbosFFI.framework/ArbosFFI
  swift=$HOME/arbos-gate/packages/ArbosFFI/Sources/ArbosFFI/ArbosFFI.swift
  [[ -f "$bin" && -f "$swift" ]] || { echo "selftest: no Debug product or generated ArbosFFI.swift to check"; exit 2; }
  if ffi_framework_current "$bin" "$swift"; then echo "selftest: ArbosFFI.framework current (newest struct ${FFI_NEWEST})"; else
    echo "selftest: ArbosFFI.framework STALE (no metadata for ${FFI_NEWEST})"; exit 1; fi
  # The test-target comparison must blame a branch for an error main does not have, and only for that.
  tlog=$(mktemp -t ios-gate-selftest)
  printf '%s\n' "/Users/x/arbos-gate/apps/ios/Arbos/ArbosTests/A.swift:10:23: error: cannot find 'ChatSession' in scope" \
                 "/Users/x/arbos-gate/apps/ios/Arbos/ArbosTests/B.swift:3:1: error: missing argument for parameter 'silenced'" \
                 "   |  \`- error: cannot find 'ChatSession' in scope" "** BUILD FAILED **" > "$tlog"
  branch=$(test_error_set "$tlog")
  printf '%s\n' "/Users/x/arbos-gate/apps/ios/Arbos/ArbosTests/B.swift:7:1: error: missing argument for parameter 'silenced'" > "$tlog"
  base=$(test_error_set "$tlog"); rm -f "$tlog"
  new=$(new_test_errors "$branch" "$base")
  if [[ "$new" == "apps/ios/Arbos/ArbosTests/A.swift: cannot find 'ChatSession' in scope" ]]; then
    echo "selftest: test-target comparison blames only the branch's new error"
  else
    echo "selftest: test-target comparison WRONG — got: $new"; exit 1
  fi
  # The scheme must build the unit-test target in its Test action, or build-for-testing checks nothing.
  scheme=$HOME/arbos-gate/apps/ios/Arbos/Arbos.xcodeproj/xcshareddata/xcschemes/Arbos.xcscheme
  if [[ -f "$scheme" ]] && grep -q 'BuildableName = "ArbosTests.xctest"' "$scheme"; then
    echo "selftest: scheme Arbos builds ArbosTests.xctest"
  else
    echo "selftest: scheme Arbos does NOT reference ArbosTests.xctest — build-for-testing would check nothing"; exit 1
  fi
  exit 0
fi

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
  # Xcode does not rebuild the ArbosFFI package framework in derived data when only the binary target
  # (the .a inside the xcframework) or the generated ArbosFFI.swift changed: #1544's MessageGrantDto
  # landed in the Rust FFI and the Debug product kept a 4-day-old ArbosFFI.framework (dyld: Symbol not
  # found _$s8ArbosFFI15MessageGrantDtoVMn on every simulator, 2026-10-25 22:00). Fingerprint the FFI
  # inputs; when they moved, drop the framework's products and intermediates so this build recreates
  # them. Also drop any top-level Products/<cfg>-iphonesimulator/ArbosFFI.framework: an older layout
  # left one there (Oct 21) and it shadows PackageFrameworks/ on the debug dylib's rpath.
  ffi_fp=$(cat "$SIM_LIB" "$PKG_SWIFT" | md5 -q 2>/dev/null || cat "$SIM_LIB" "$PKG_SWIFT" | md5sum | cut -c1-32)
  for cfg in Release Debug; do
    dd="$HOME/arbos-gate-dd-$cfg"
    [[ -d "$dd/Build" ]] || continue
    rm -rf "$dd/Build/Products/$cfg-iphonesimulator/ArbosFFI.framework"
    if [[ "$(cat "$dd/arbos-ffi.fingerprint" 2>/dev/null)" != "$ffi_fp" ]]; then
      rm -rf "$dd/Build/Products/$cfg-iphonesimulator/PackageFrameworks/ArbosFFI.framework" \
             "$dd/Build/Products/$cfg-iphonesimulator/ArbosFFI.o" "$dd/Build/Products/$cfg-iphonesimulator/ArbosFFI.swiftmodule" \
             "$dd/Build/Products/$cfg-iphonesimulator/libarbos_ffi.a" "$dd/Build/Intermediates.noindex/ArbosFFI.build"
      echo "== xcframework: FFI inputs changed since the last $cfg build; ArbosFFI framework will be rebuilt"
    fi
  done
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
    elif [[ -n "${ffi_fp:-}" ]]; then
      # The product must link the FFI it was built against: every Swift type the generated ArbosFFI.swift
      # declares as a struct has its metadata symbol in the framework; check the newest one by name.
      if ffi_framework_current "$fw/ArbosFFI" "$ROOT/$PKG_SWIFT"; then
        echo "$ffi_fp" >"$HOME/arbos-gate-dd-$config/arbos-ffi.fingerprint"
      else
        echo "== $config: embedded ArbosFFI.framework is stale (no metadata for ${FFI_NEWEST:-?})"
        rc=1
      fi
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
# The comparison is by ERROR, not by exit code (#1843): #1794 added JumpToLatestWindowTests reading a
# private ChatSession while main's test target happened to be red for another reason, the old
# "main fails too" exit-code check waved it through and it shipped to TestFlight. Now every error the
# branch tree has that the base tree does not blames the branch.
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
    branch_errs=$(test_error_set "$log")
    printf '%s\n' "$branch_errs" | head -20
    base_errs=""
    GATE_BASE_SHA=${GATE_BASE_SHA:-$(git -C "$ROOT" rev-parse origin/main 2>/dev/null || true)}
    if [[ -n "${GATE_BASE_SHA:-}" ]] && git -C "$ROOT" cat-file -e "$GATE_BASE_SHA^{commit}" 2>/dev/null; then
      # Which of these errors does main alone have? Those are not this branch's regression.
      cur=$(git -C "$ROOT" rev-parse HEAD)
      blog=$(mktemp -t ios-gate-base)
      git -C "$ROOT" checkout -q "$GATE_BASE_SHA" 2>/dev/null && {
        xcodebuild -project Arbos.xcodeproj -scheme Arbos -configuration Debug \
          -destination 'generic/platform=iOS Simulator' -derivedDataPath "$HOME/arbos-gate-dd-Debug" \
          CODE_SIGNING_ALLOWED=NO build-for-testing >"$blog" 2>&1 || true
        git -C "$ROOT" checkout -q "$cur" 2>/dev/null
      }
      base_errs=$(test_error_set "$blog")
      rm -f "$blog"
    fi
    new_errs=$(new_test_errors "$branch_errs" "$base_errs")
    if [[ -z "$new_errs" ]]; then
      echo "== tests: every error exists on main too (${GATE_BASE_SHA:0:8}); not this branch's regression — reported, not blamed"
    else
      echo "== tests: errors NOT on main (${GATE_BASE_SHA:0:8}) — this branch's regression:"
      printf '%s\n' "$new_errs" | head -20
      rc=1
    fi
  fi
  rm -f "$log"
fi
exit $rc

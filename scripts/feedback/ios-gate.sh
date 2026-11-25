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

# build_errors <xcodebuild log> → why the app target did not compile, in the one shape that survives
# the trip to the worker's issue. `rust-gate.sh status` forwards only "== " lines and lines already
# shaped like a compiler diagnostic, so anything written in another shape is readable on the gate Mac
# and nowhere else. Everything here is therefore "== " prefixed, which also keeps it in log order
# directly under its own "== <config>: FAILS" heading instead of being hoisted into a separate block.
#
# Measured against the real transport on 2026-10-31, three ways the cause failed to arrive:
#   - The fallback never travelled. The "-- <config> log tail --" below the grep existed for builds
#     whose cause the grep cannot match, and every one of its lines was dropped. On a link failure the
#     worker was shown `clang: error: linker command failed with exit code 1`, which names no cause,
#     while `ld: framework 'ArbosFFI' not found` stayed on the Mac.
#   - The cascade crowded out the rest. One renamed type reported at 30 use sites filled the 25-line
#     window the poller posts and pushed the heading, and the one error that said what to do, out of
#     the comment: 31 errors in the log, 2 distinct ones on the issue.
#   - Every error was printed twice, once by the grep and again inside the tail under it.
build_errors() {
  local log=$1 errs over
  # One entry per distinct "<file>: <message>", first-seen order, carrying how many sites repeat it.
  # Line and column are dropped for the grouping, so a cascade collapses to the sentence that matters.
  errs=$(grep -E "error: |^ld: |^Command .+ failed" "$log" 2>/dev/null | grep -v "warning:" \
    | sed -E "s#/Users/[^/]*/arbos-gate/##g; s#^([^ ]+):[0-9]+:[0-9]+: #\1: #; s#: error: #: #" \
    | awk '{ if (!($0 in n)) order[++k] = $0; n[$0]++ }
           END { for (i = 1; i <= k; i++)
                   printf "%s%s\n", order[i], (n[order[i]] > 1 ? "   (" n[order[i]] " sites)" : "") }')
  if [[ -n "$errs" ]]; then
    printf '%s\n' "$errs" | head -12 | cut -c1-300 | sed 's/^/== error: /'
    over=$(( $(printf '%s\n' "$errs" | wc -l) - 12 ))
    if (( over > 0 )); then
      echo "== error: … and $over more distinct errors; the full log is on the gate Mac"
    fi
  else
    # Nothing matched, so the tail is the only evidence there is and it has to reach the issue.
    echo "== build failed with no error line this gate recognises; last lines of the log:"
    tail -n 20 "$log" | sed -E "s#/Users/[^/]*/arbos-gate/##g" | cut -c1-300 | sed 's/^/== | /'
  fi
  # Never leave a false exit status behind: under `set -o pipefail` a function whose last command is a
  # failed test poisons any pipeline it is read through, which is how a clean `rust-gate.sh status` once
  # read as "Mac unreachable" (see the note on its own grep). The caller judges the build, not this.
  return 0
}

# Run the whole ArbosTests suite (#1868, coordinator 2026-10-26 05:52) on ONE named simulator, parallel
# off (docs/process-rules.md), reusing the .xctestrun that build-for-testing just produced: 217 tests run
# in 4 s, ≈ 1 min wall. A test red on MAIN is not the branch's fault: the failing set of the base main
# head is cached per head (~/arbos-gate/.test-baseline/<sha>) — computed once on main's own tree when a
# new head appears — and only failures outside that baseline fail the half (the #1843 / known-red rule).
TEST_SIM_NAME="${ARBOS_GATE_TEST_SIM:-iPhone 17}"
BASELINE_DIR="$HOME/arbos-gate/.test-baseline"

# failing_tests <xcodebuild log> → "Class/test" per failing test, sorted.
failing_tests() {
  grep -E "^Test Case '-\[ArbosTests\.[A-Za-z0-9_]+ [A-Za-z0-9_]+\]' failed" "$1" 2>/dev/null \
    | sed -E "s/^Test Case '-\[ArbosTests\.([A-Za-z0-9_]+) ([A-Za-z0-9_]+)\]' failed.*/\1\/\2/" | sort -u
}

test_sim_udid() {
  xcrun simctl list devices available 2>/dev/null | grep -F "$TEST_SIM_NAME (" | grep -oE '[0-9A-F-]{36}' | head -n 1
}

# run_suite <derived-data dir> <log> → xcodebuild's exit code; the .xctestrun is the one build-for-testing wrote.
run_suite() {
  local dd="$1" log="$2" xctestrun udid
  xctestrun=$(ls -t "$dd"/Build/Products/*.xctestrun 2>/dev/null | head -n 1)
  udid=$(test_sim_udid)
  [[ -n "$xctestrun" && -n "$udid" ]] || { echo "no xctestrun or no simulator '$TEST_SIM_NAME'" >"$log"; return 2; }
  xcrun simctl bootstatus "$udid" -b >/dev/null 2>&1
  # xcodebuild sometimes never exits after the suite has finished (06:23 Oct 26: 222 tests done, the
  # process idle on the main thread 8 min later). Run it in the background and stop it once the log
  # carries the suite verdict (plus a 20 s grace for the result bundle), or after TEST_RUN_MAX_S.
  xcodebuild test-without-building -xctestrun "$xctestrun" \
    -destination "platform=iOS Simulator,id=$udid" -parallel-testing-enabled NO \
    -only-testing:ArbosTests >"$log" 2>&1 &
  local pid=$! t0 done_at=0 now
  t0=$(date +%s)
  while kill -0 "$pid" 2>/dev/null; do
    now=$(date +%s)
    if (( done_at == 0 )) && grep -qE "Test Suite 'All tests' (passed|failed)|\*\* TEST (EXECUTE FAILED|SUCCEEDED) \*\*" "$log" 2>/dev/null; then
      done_at=$now
    fi
    if (( done_at > 0 && now - done_at > 20 )) || (( now - t0 > TEST_RUN_MAX_S )); then
      kill "$pid" 2>/dev/null; sleep 3; kill -9 "$pid" 2>/dev/null
      echo "== tests: xcodebuild stopped by the watchdog ($(( now - t0 )) s; verdict $([ "$done_at" -gt 0 ] && echo present || echo MISSING))" >>"$log"
      break
    fi
    sleep 5
  done
  wait "$pid" 2>/dev/null
  # The verdict line, not xcodebuild's exit code, decides: a watchdog kill after a complete run is a pass/fail by the log.
  grep -qE "Test Suite 'All tests' passed|\*\* TEST SUCCEEDED \*\*" "$log" && return 0
  grep -qE "Test Suite 'All tests' failed|\*\* TEST EXECUTE FAILED \*\*" "$log" && return 65
  return 70
}
TEST_RUN_MAX_S="${TEST_RUN_MAX_S:-900}"

# baseline_for <main sha> → prints the cached failing set of main at that head, computing it once:
# checks out the base in the gate tree, build-for-testing into the Debug derived data, runs the suite.
baseline_for() {
  local base="$1" f="$BASELINE_DIR/$1" cur blog
  mkdir -p "$BASELINE_DIR"
  if [[ -f "$f" ]]; then cat "$f"; return 0; fi
  cur=$(git -C "$ROOT" rev-parse HEAD)
  blog=$(mktemp -t ios-gate-baseline)
  if git -C "$ROOT" checkout -q "$base" 2>/dev/null; then
    if ( cd "$ROOT/apps/ios/Arbos" && xcodebuild -project Arbos.xcodeproj -scheme Arbos -configuration Debug \
          -destination 'generic/platform=iOS Simulator' -derivedDataPath "$HOME/arbos-gate-dd-Debug" \
          CODE_SIGNING_ALLOWED=NO build-for-testing >/dev/null 2>&1 ); then
      run_suite "$HOME/arbos-gate-dd-Debug" "$blog" || true
      failing_tests "$blog" >"$f"
    else
      # Main itself does not compile its tests (or the tree's xcframework does not fit it): no baseline —
      # the caller does not blame the branch; the compile check above already reports main's breakage.
      echo "__unavailable__" >"$f"
    fi
    git -C "$ROOT" checkout -q "$cur" 2>/dev/null
    # Keep the dozen newest baselines; a main head is gated against for an hour or two at most.
    ls -t "$BASELINE_DIR" 2>/dev/null | tail -n +13 | while read -r old; do rm -f "$BASELINE_DIR/$old"; done
  fi
  rm -f "$blog"
  cat "$f" 2>/dev/null
}

run_unit_tests() {
  local log t0 fails base baseline new
  log=$(mktemp -t ios-gate-tests); t0=$(date +%s)
  run_suite "$HOME/arbos-gate-dd-Debug" "$log"
  local trc=$?
  fails=$(failing_tests "$log")
  if [[ "$trc" -eq 0 && -z "$fails" ]]; then
    echo "== tests: $(grep -oE 'Executed [0-9]+ tests' "$log" | tail -n 1 | tr -d '\n') pass ($(( $(date +%s) - t0 )) s)"
    rm -f "$log"; return 0
  fi
  if [[ -z "$fails" ]]; then
    # xcodebuild failed without a single failing test case: simulator / infrastructure trouble, not the branch.
    echo "== tests: could not run (rc=$trc, no test case failed) — not blamed; tail:"
    tail -n 8 "$log" | cut -c1-200
    rm -f "$log"; return 0
  fi
  GATE_BASE_SHA=${GATE_BASE_SHA:-$(git -C "$ROOT" rev-parse origin/main 2>/dev/null || true)}
  base="${GATE_BASE_SHA:-}"
  baseline=""
  [[ -n "$base" ]] && baseline=$(baseline_for "$base")
  if grep -qx "__unavailable__" <<< "$baseline"; then
    echo "== tests: $(printf '%s\n' "$fails" | grep -c .) failing test(s) but main (${base:0:8}) cannot run its own suite — no baseline, not blamed: $(printf '%s' "$fails" | tr '\n' ' ')"
    rm -f "$log"; return 0
  fi
  new=$(comm -23 <(printf '%s\n' "$fails") <(printf '%s\n' "$baseline" | sed '/^$/d' | sort -u))
  if [[ -z "$new" ]]; then
    echo "== tests: $(printf '%s\n' "$fails" | grep -c .) failing test(s) are red on main (${base:0:8}) too — not this branch's regression: $(printf '%s' "$fails" | tr '\n' ' ')"
    rm -f "$log"; return 0
  fi
  echo "== tests: FAIL — failing and NOT red on main (${base:0:8}):"
  printf '%s\n' "$new" | head -20
  grep -E "error: -\[ArbosTests\." "$log" | sed 's#/Users/[^/]*/arbos-gate/##' | head -20 | cut -c1-300
  rm -f "$log"; return 1
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
  # The test-run parser must read xcodebuild's failed test cases as Class/test.
  tlog=$(mktemp -t ios-gate-selftest)
  printf '%s\n' "Test Case '-[ArbosTests.HiddenWorkerRowTests testAWorkerSpawnRowDrawsNothingButATeammateSendKeepsItsLine]' failed (1.443 seconds)." \
                 "Test Case '-[ArbosTests.HiddenWorkerRowTests testWorkerFlagParsesAndDefaultsOff]' passed (0.002 seconds)." > "$tlog"
  if [[ "$(failing_tests "$tlog")" == "HiddenWorkerRowTests/testAWorkerSpawnRowDrawsNothingButATeammateSendKeepsItsLine" ]]; then
    echo "selftest: failing-test parser reads Class/test"
  else
    echo "selftest: failing-test parser WRONG — got: $(failing_tests "$tlog")"; rm -f "$tlog"; exit 1
  fi
  rm -f "$tlog"
  # The app target's failure has to survive the trip, not merely be printed. `forwarded` is the filter
  # from rust-gate.sh `status` — the only lines the poller ever sees — so these assertions fail if the
  # transport is changed out from under them, which is how the cause went missing in the first place.
  forwarded() { grep -E "^== |^(error|warning)(\[|:)|^\s+--> |^Diff in |^test .* FAILED|^failures:|panicked at|: error: |\*\* BUILD FAILED"; }
  blog=$(mktemp -t ios-gate-build)
  # A link failure: the only line the old grep kept names no cause, and the cause was in the tail.
  printf '%s\n' "ld: warning: ignoring duplicate libraries: '-lc++'" \
                 "ld: framework 'ArbosFFI' not found" \
                 "clang: error: linker command failed with exit code 1 (use -v to see invocation)" \
                 "** BUILD FAILED **" >"$blog"
  if build_errors "$blog" | forwarded | grep -qF "ld: framework 'ArbosFFI' not found"; then
    echo "selftest: a link failure's cause reaches the issue, not just 'linker command failed'"
  else
    echo "selftest: WRONG — the cause of a link failure is dropped in transport; the worker is told"
    echo "          only that the linker failed. Got: $(build_errors "$blog" | forwarded | tr '\n' '|')"
    rm -f "$blog"; exit 1
  fi
  # A renamed type reported at 30 use sites: the cascade must not crowd out the error that explains it.
  : >"$blog"
  for i in $(seq 1 30); do
    echo "/Users/x/arbos-gate/apps/ios/Arbos/Views/ChatView.swift:$((100 + i)):24: error: cannot find 'MentionChipSpacing' in scope" >>"$blog"
  done
  echo "/Users/x/arbos-gate/apps/ios/Arbos/Theme/Spacing.swift:14:8: error: 'MentionChipSpacing' was renamed to 'MentionChipMetrics'" >>"$blog"
  out=$(build_errors "$blog" | forwarded)
  if [[ "$(wc -l <<< "$out")" -le 12 ]] && grep -qF "renamed to 'MentionChipMetrics'" <<< "$out" \
     && grep -qF "(30 sites)" <<< "$out"; then
    echo "selftest: a 31-error cascade collapses to its distinct causes and still fits the comment"
  else
    echo "selftest: WRONG — cascade not collapsed ($(wc -l <<< "$out") lines) or the explaining error"
    echo "          was crowded out; the poller posts only the last 25 lines it is given"; rm -f "$blog"; exit 1
  fi
  rm -f "$blog"
  [[ -n "$(test_sim_udid)" ]] && echo "selftest: test simulator '$TEST_SIM_NAME' present ($(test_sim_udid))" || { echo "selftest: test simulator '$TEST_SIM_NAME' MISSING"; exit 1; }
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
# Where the FFI cross builds actually land. build-xcframework.sh reads CARGO_TARGET_DIR and falls back to
# the tree's own target/, so the gate's artefacts follow whatever the caller exported: rust-gate.sh points
# it at the scratch volume (/Volumes/ArbosScratch/ec2-user/arbos-rust-gate-target). Resolve it the same way
# the build does, never by name — the prune below was hard-coded to ~/arbos-gate/target, a directory that
# does not exist under the scratch target dir, so it has never once removed a file. The volume filled and
# the iOS gate died on "No space left on device" at 02:35 on 2026-10-30; the disk was cleared by hand and
# the cause left in place to do it again.
ffi_target_dir() { echo "${CARGO_TARGET_DIR:-${ROOT:-$HOME/arbos-gate}/target}"; }
teardown() {
  teardown_sims
  # Each run leaves a fresh libarbos_ffi-<hash>.a per iOS target under <target dir>/<triple>/release/deps
  # (0.2-0.3 GB each; 77 of them filled 13 GB on 2026-10-22). Keep the newest two per directory, drop the
  # rest. Only the Apple-iOS triples build-xcframework.sh builds: the scratch target dir is shared with the
  # Mac half, whose own artefacts sit under <target dir>/release and must be left alone.
  local deps gone n=0
  for deps in "$(ffi_target_dir)"/*apple-ios*/release/deps; do
    [ -d "$deps" ] || continue
    gone=$(ls -t "$deps"/libarbos_ffi-*.a 2>/dev/null | tail -n +3)
    [ -n "$gone" ] || continue
    n=$(( n + $(printf '%s\n' "$gone" | grep -c .) ))
    printf '%s\n' "$gone" | xargs -r rm -f
  done
  # Not `(( n > 0 )) && echo …`: this runs in the EXIT trap, and a trap must not end on a false test
  # (the same shape cost rust-gate.sh's `status` its verdict once — a trailing grep that matched nothing
  # read as "Mac unreachable").
  if (( n > 0 )); then echo "== prune: dropped $n stale libarbos_ffi-*.a under $(ffi_target_dir)"; fi
  return 0
}
trap teardown EXIT
cd ~/arbos-gate || exit 2
ROOT=$(pwd)
git fetch -q origin "+refs/heads/${branch}:refs/remotes/origin/${branch}" || { echo "fetch of ${branch} failed"; exit 2; }
# The base checks (test-target compile, test baseline) compare against origin/main — fetch it, or the
# gate tree's stale ref (bc640a45 from 2026-10-25 20:xx on 2026-10-26 06:23) is "main" (#1878).
git fetch -q origin "+refs/heads/main:refs/remotes/origin/main" || echo "== warning: fetch of main failed; base checks use the tree's last known main"
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
  if [[ "$crc" -eq 0 ]]; then
    grep -E "BUILD SUCCEEDED|note: (building|ArbosFFI)" "$log" | sed 's#/Users/[^/]*/arbos-gate/##' | head -40
  else
    build_errors "$log"
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
    run_unit_tests || rc=1
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

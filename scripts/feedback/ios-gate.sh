#!/bin/bash
# iOS pre-merge gate, run on the EC2 Mac by the feedback poller over ssh:
#   ios-gate.sh <branch> <sha>
# Fetches the branch into ~/arbos-gate (its own clone, never the audit clone), checks out <sha>
# detached, and builds the iPhone app twice (Release, then Debug; generic simulator, no signing). Prints the
# compiler errors; exit 0 = both compile, 1 = one does not, 2 = infrastructure trouble (fetch, checkout, or a
# rig that cannot run the suite — the poller retries those and records no verdict against the branch).
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

# Run the whole ArbosTests suite (#1868, coordinator 2026-10-26 05:52) on ONE named simulator, parallel
# off (docs/process-rules.md), reusing the .xctestrun that build-for-testing just produced: 217 tests run
# in 4 s, ≈ 1 min wall. A test red on MAIN is not the branch's fault: the failing set of the base main
# head is cached per head (~/arbos-gate/.test-baseline/<sha>) — computed once on main's own tree when a
# new head appears — and only failures outside that baseline fail the half (the #1843 / known-red rule).
#
# Two operator overrides, both one-line files in ~/arbos-gate, because the poller launches this over
# ssh and no environment of yours reaches it. Both are named in the failure message, so whoever meets
# a rig that cannot test at 3am does not have to read this script to find them:
#   .test-sim        the simulator to run on, when the rig has no device under the default name.
#                    Stands until removed. ARBOS_GATE_TEST_SIM does the same for a hand-run gate.
#   .allow-untested  one line of reason. Lets ONE run ship a branch the rig could not test, then is
#                    consumed, so the next branch is gated normally. The reason is printed into the
#                    gate output, which the poller posts on the issue.
SIM_PIN="$HOME/arbos-gate/.test-sim"
ALLOW_UNTESTED="$HOME/arbos-gate/.allow-untested"
TEST_SIM_NAME="${ARBOS_GATE_TEST_SIM:-$(head -n 1 "$SIM_PIN" 2>/dev/null)}"
TEST_SIM_NAME="${TEST_SIM_NAME:-iPhone 17}"
BASELINE_DIR="$HOME/arbos-gate/.test-baseline"
# The target run_suite selects. Named once so the xcodebuild flag and the "0 tests" message cannot drift.
TEST_ONLY=ArbosTests

# xcodebuild reports a test case in one of two shapes and this gate has to read both.
#   Xcode 27 running a target in parallel clones, which is what ArbosTests is:
#     Test case 'PinPolicyTests.testAMemberCannotPin()' passed on 'Clone 1 of iPhone 17 - Arbos (41549)' (0.001 seconds)
#   The older serial reporter, which ArbosUITests still uses:
#     Test Case '-[ArbosTests.PinPolicyTests testAMemberCannotPin]' passed (0.001 seconds).
# The wording, the capitalisation and the brackets all differ, so a parser written for one
# matches nothing in the other. Do NOT key anything on the `Executed N tests` summary: the
# parallel reporter never prints it. In a 10,713-line hosted log of a run that passed, all
# 507 ArbosTests cases were the first shape with no summary of their own, and the only eight
# `Executed N tests` lines in the file belonged to the 33 UI tests. On the gate Mac, where
# run_suite selects ArbosTests alone, there is no such line at all — asserting on it reported
# a six-case run that genuinely passed as a run of nothing
# (store note internal/xcodebuild-test-count-detection.md, 2026-10-30 20:13).
# test_cases <log> <passed|failed> → one "Class/test" per matching case, sorted, unique.
# Both shapes normalise to "Class/test", which is what the known-red baseline files hold.
test_cases() {
  local log=$1 want=$2
  {
    grep -E "^Test case '[A-Za-z0-9_]+\.[A-Za-z0-9_]+\(\)' $want " "$log" 2>/dev/null \
      | sed -E "s/^Test case '([A-Za-z0-9_]+)\.([A-Za-z0-9_]+)\(\)' $want .*/\1\/\2/"
    grep -E "^Test Case '-\[([A-Za-z0-9_]+\.)?[A-Za-z0-9_]+ [A-Za-z0-9_]+\]' $want" "$log" 2>/dev/null \
      | sed -E "s/^Test Case '-\[([A-Za-z0-9_]+\.)?([A-Za-z0-9_]+) ([A-Za-z0-9_]+)\]' $want.*/\2\/\3/"
  } | sort -u
}

# failing_tests <xcodebuild log> → "Class/test" per failing test, sorted.
failing_tests() {
  test_cases "$1" failed
}

# test_run_state <xcodebuild log> → "<state> <passed> <failed>". The point of the state is to
# keep "I read this log and it ran nothing" apart from "I could not read this log", because
# the second is a parser that has fallen behind the toolchain and must not be reported as a
# suite of zero tests — that mistake looks exactly like a test regression.
#   ran        case lines are there, so the counts mean what they say
#   none       the run finished and says in so many words that it selected no test
#   unreadable the run finished, but carries no case line of either known shape: either it
#              selected nothing in a way this toolchain does not spell out, or the per-case
#              wording has changed again and test_cases needs updating. Never silently zero.
#   incomplete no verdict banner at all — the run did not get that far (no simulator, a
#              watchdog kill before the end). Infrastructure, handled by the caller as before.
test_run_state() {
  local log=$1 passed failed
  passed=$(test_cases "$log" passed | grep -c .)
  failed=$(test_cases "$log" failed | grep -c .)
  if (( passed + failed > 0 )); then
    echo "ran $passed $failed"
  elif ! grep -qE '\*\* TEST (SUCCEEDED|FAILED|EXECUTE FAILED) \*\*' "$log" 2>/dev/null; then
    echo "incomplete 0 0"
  elif grep -qE "Executed 0 test|Test Suite 'Selected tests'" "$log" 2>/dev/null; then
    echo "none 0 0"
  else
    echo "unreadable 0 0"
  fi
}

# run_verdict <xcodebuild log> → passed | failed | none. Both the watchdog below and run_suite's
# exit code read this one function, so the banners live in one place.
# `** TEST FAILED **` is the banner a plain test failure prints, and on a unit-only run it is the
# only verdict there is: the parallel reporter writes no `Test Suite 'All tests'` block at all
# (in the hosted log that block spans lines 4759-10065 and covers the UI target alone, closing on
# `Executed 33 tests`, while the 507 unit cases start at line 10066 after a bare `Testing
# started`). While that literal was missing from the alternation, a failing unit run never
# registered as finished and burned the whole TEST_RUN_MAX_S before the watchdog killed it —
# fifteen minutes of a contended Mac, on exactly the runs someone is waiting for.
run_verdict() {
  if grep -qE "Test Suite 'All tests' passed|\*\* TEST SUCCEEDED \*\*" "$1" 2>/dev/null; then
    echo passed
  elif grep -qE "Test Suite 'All tests' failed|\*\* TEST (FAILED|EXECUTE FAILED) \*\*" "$1" 2>/dev/null; then
    echo failed
  else
    echo none
  fi
}

test_sim_udid() {
  xcrun simctl list devices available 2>/dev/null | grep -F "$TEST_SIM_NAME (" | grep -oE '[0-9A-F-]{36}' | head -n 1
}

# run_suite <derived-data dir> <log> → xcodebuild's exit code; the .xctestrun is the one build-for-testing wrote.
run_suite() {
  local dd="$1" log="$2" xctestrun udid
  xctestrun=$(ls -t "$dd"/Build/Products/*.xctestrun 2>/dev/null | head -n 1)
  udid=$(test_sim_udid)
  # Write what the rig actually offers, not only what is missing: the hosted runner hit the same
  # fault on 2026-10-30 (runs 41114348485, 41116429834) and its log named the device list, which is
  # what told us CoreSimulator had no device at all rather than one under a different name.
  if [[ -z "$xctestrun" || -z "$udid" ]]; then
    { echo "no xctestrun or no simulator '$TEST_SIM_NAME'"
      echo "xctestrun: ${xctestrun:-none under $dd/Build/Products}"
      echo "simulators this rig offers (xcrun simctl list devices available):"
      xcrun simctl list devices available 2>&1 | head -n 25
    } >"$log"
    return 2
  fi
  xcrun simctl bootstatus "$udid" -b >/dev/null 2>&1
  # xcodebuild sometimes never exits after the suite has finished (06:23 Oct 26: 222 tests done, the
  # process idle on the main thread 8 min later). Run it in the background and stop it once the log
  # carries the suite verdict (plus a 20 s grace for the result bundle), or after TEST_RUN_MAX_S.
  xcodebuild test-without-building -xctestrun "$xctestrun" \
    -destination "platform=iOS Simulator,id=$udid" -parallel-testing-enabled NO \
    "-only-testing:$TEST_ONLY" >"$log" 2>&1 &
  local pid=$! t0 done_at=0 now
  t0=$(date +%s)
  while kill -0 "$pid" 2>/dev/null; do
    now=$(date +%s)
    if (( done_at == 0 )) && [[ "$(run_verdict "$log")" != none ]]; then
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
  case "$(run_verdict "$log")" in
    passed) return 0 ;;
    failed) return 65 ;;
    *) return 70 ;;
  esac
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
  local log t0 fails state passed base baseline new
  log=$(mktemp -t ios-gate-tests); t0=$(date +%s)
  run_suite "$HOME/arbos-gate-dd-Debug" "$log"
  local trc=$?
  fails=$(failing_tests "$log")
  read -r state passed _ <<< "$(test_run_state "$log")"
  # Three of the four states carry no test case at all, and they are decided together: a branch
  # that no test ran against has not been checked, so none of them may report a pass. Each one
  # still gets its own words, because they are repaired in three different places.
  case "$state" in
    unreadable)
      # A finished run whose cases this script cannot find is a parser problem, and saying so is the
      # whole point of the state: the alternative is calling it zero tests, which reads as a suite
      # that never ran and sends the next person looking at the branch instead of at this file.
      echo "== tests: FAIL — the run finished but no test case line of a shape this gate knows is in"
      echo "   its log, so it cannot tell a suite that passed from one that never ran. Either"
      echo "   -only-testing:$TEST_ONLY selected nothing, or xcodebuild's per-case wording changed"
      echo "   again and test_cases() needs the new shape (see its comment for the two it reads)."
      tail -n 8 "$log" | cut -c1-200
      rm -f "$log"; return 1 ;;
    none)
      # xcodebuild exits 0 and prints "** TEST SUCCEEDED **" when -only-testing matches no test in
      # the .xctestrun, so the verdict line cannot see it (2026-10-30: a rig built the app, ran 0
      # tests and reported success because a new ArbosUITests file was never added to project.pbxproj,
      # so it compiled into no bundle).
      echo "== tests: FAIL — the suite finished but ran no test (rc=$trc); -only-testing:$TEST_ONLY"
      echo "   matched nothing in the .xctestrun. Check that $TEST_ONLY is a target of the Arbos scheme's"
      echo "   Test action and that every test file is a member of it in project.pbxproj."
      tail -n 8 "$log" | cut -c1-200
      rm -f "$log"; return 1 ;;
    incomplete)
      # No verdict banner at all: the suite never started, so nothing was checked. The cause is the
      # rig (no simulator named TEST_SIM_NAME, no .xctestrun, a kill before the first case), never the
      # branch — which is why this returns 2 and not 1. Two is the gate's infrastructure code, like a
      # failed fetch: rust-gate.sh reports `infra`, the poller retries on the next tick, no verdict is
      # recorded and no issue is told its branch failed. What it must never do again is return 0. It
      # did until 2026-10-30, printing "could not run — not blamed" on a pass, which let a branch no
      # test ran against merge as if it had been tested. The hosted runner met the same empty
      # simulator set three times that day (runs 41081201116, 41114348485, 41116429834) and went red
      # for it; the gate waved it through.
      if [[ -f "$ALLOW_UNTESTED" ]]; then
        local why; why=$(head -n 1 "$ALLOW_UNTESTED"); rm -f "$ALLOW_UNTESTED"
        echo "== tests: NOT RUN (rc=$trc) and shipped anyway on the operator's one-shot override"
        echo "   ($ALLOW_UNTESTED, now consumed): ${why:-no reason given}"
        echo "   This branch merges WITHOUT a test run. The next branch is gated normally."
        tail -n 12 "$log" | cut -c1-200
        rm -f "$log"; return 0
      fi
      # Every line of the repair starts with "== ", because that is the only shape rust-gate.sh's
      # `status` forwards to the poller (`grep -E "^== "`). An indented continuation line is dropped
      # on the way out and the operator would have to be on the Mac to read it — which is exactly
      # the person we do not have at 3am.
      echo "== tests: NOT RUN — the suite never reached a verdict (rc=$trc), so this branch was NOT checked."
      echo "== tests: the rig could not test. That is not the branch's fault and it is not a pass: the gate"
      echo "== tests: reports infrastructure, the poller retries, and nothing merges untested meanwhile."
      echo "== rig fix, on the gate Mac (ssh via the relay host, docs/process-rules.md), in this order:"
      echo "== rig fix: 1. xcrun simctl list devices available | grep -F '$TEST_SIM_NAME'"
      echo "== rig fix:    it is listed? nothing to repair, the retry will pass. Stop here."
      echo "== rig fix: 2. no device of any name? CoreSimulator is sick. Restart it:"
      echo "== rig fix:    sudo killall -9 com.apple.CoreSimulator.CoreSimulatorService"
      echo "== rig fix: 3. devices under other names? pin one: echo '<device name>' > $SIM_PIN"
      echo "== rig fix: 4. a hotfix must ship now? allow ONE untested run, consumed by it:"
      echo "== rig fix:    echo '<your name>: <why>' > $ALLOW_UNTESTED"
      echo "== rig fix: 5. none of those? escalate: items stay gating and the merge queue stops draining."
      tail -n 12 "$log" | cut -c1-200 | sed 's/^/== rig: /'
      rm -f "$log"; return 2 ;;
  esac
  if [[ "$trc" -eq 0 && -z "$fails" ]]; then
    echo "== tests: $passed passed ($(( $(date +%s) - t0 )) s)"
    rm -f "$log"; return 0
  fi
  if [[ -z "$fails" ]]; then
    # The suite ran cases and not one of them failed, yet xcodebuild still returned non-zero (a crash
    # after the last case, a watchdog kill once the verdict was in): infrastructure, not the branch.
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
  # The run-state reader is what keeps a passing suite from being reported as a suite that never
  # ran, so it is pinned to lines copied out of real logs: the parallel shape from the gate Mac's
  # Xcode 27, the serial shape, a run that selected nothing, and a finished run in neither shape.
  printf '%s\n' "Test case 'PinPolicyTests.testAdminsPolicyRefusesAPlainMember()' passed on 'Clone 1 of iPhone 17 - Arbos (41549)' (0.001 seconds)" \
                 "Test case 'PinPolicyTests.testAMemberCannotPin()' failed on 'Clone 1 of iPhone 17 - Arbos (41549)' (0.002 seconds)" \
                 "** TEST FAILED **" > "$tlog"
  parallel="$(test_run_state "$tlog") $(failing_tests "$tlog")"
  printf '%s\n' "Test Case '-[ArbosTests.PinPolicyTests testAdminsPolicyRefusesAPlainMember]' passed (0.001 seconds)." \
                 "Test Case '-[ArbosTests.PinPolicyTests testAMemberCannotPin]' failed (0.002 seconds)." \
                 "	 Executed 2 tests, with 1 failure (0 unexpected) in 0.003 (0.004) seconds" \
                 "** TEST FAILED **" > "$tlog"
  serial="$(test_run_state "$tlog") $(failing_tests "$tlog")"
  printf '%s\n' "Test Suite 'Selected tests' passed at 2026-10-30 17:24:31.002." \
                 "	 Executed 0 tests, with 0 failures (0 unexpected) in 0.000 (0.001) seconds" \
                 "** TEST SUCCEEDED **" > "$tlog"
  selected_nothing=$(test_run_state "$tlog")
  printf '%s\n' "Test thing 'PinPolicyTests.whatever' went fine on some future runner" \
                 "** TEST SUCCEEDED **" > "$tlog"
  future=$(test_run_state "$tlog")
  printf '%s\n' "no xctestrun or no simulator 'iPhone 17'" > "$tlog"
  no_run=$(test_run_state "$tlog")
  if [[ "$parallel" == "ran 1 1 PinPolicyTests/testAMemberCannotPin" \
     && "$serial" == "ran 1 1 PinPolicyTests/testAMemberCannotPin" \
     && "$selected_nothing" == "none 0 0" && "$future" == "unreadable 0 0" && "$no_run" == "incomplete 0 0" ]]; then
    echo "selftest: run-state reader counts cases in both shapes and separates none / unreadable / incomplete"
  else
    echo "selftest: run-state reader WRONG — parallel '$parallel', serial '$serial', selected-nothing"
    echo "          '$selected_nothing', future '$future', no-run '$no_run'"; rm -f "$tlog"; exit 1
  fi
  # run_verdict decides both the watchdog's "is it finished" and run_suite's exit code, so a banner
  # missing from it costs the whole TEST_RUN_MAX_S. The first case is the one that was missing:
  # a unit-only run that failed, whose only verdict is "** TEST FAILED **".
  printf '%s\n' "Test case 'PinPolicyTests.testAMemberCannotPin()' failed on 'Clone 1 of iPhone 17 - Arbos (41549)' (0.002 seconds)" \
                 "Test suite 'PinPolicyTests' failed on 'Clone 1 of iPhone 17 - Arbos (41549)'" \
                 "** TEST FAILED **" > "$tlog"
  unit_failed=$(run_verdict "$tlog")
  printf '%s\n' "Test case 'PinPolicyTests.testAMemberCannotPin()' passed on 'Clone 1 of iPhone 17 - Arbos (41549)' (0.001 seconds)" \
                 "** TEST SUCCEEDED **" > "$tlog"
  unit_passed=$(run_verdict "$tlog")
  printf '%s\n' "Test Suite 'All tests' failed at 2026-10-30 17:20:06.520." > "$tlog"
  suite_failed=$(run_verdict "$tlog")
  printf '%s\n' "** TEST EXECUTE FAILED **" > "$tlog"
  execute_failed=$(run_verdict "$tlog")
  printf '%s\n' "Test case 'PinPolicyTests.testAMemberCannotPin()' passed on 'Clone 1 of iPhone 17 - Arbos (41549)' (0.001 seconds)" > "$tlog"
  unfinished=$(run_verdict "$tlog")
  if [[ "$unit_failed" == "failed" && "$unit_passed" == "passed" && "$suite_failed" == "failed" \
     && "$execute_failed" == "failed" && "$unfinished" == "none" ]]; then
    echo "selftest: run_verdict reads a unit-only run's banner, so the watchdog sees it finish"
  else
    echo "selftest: run_verdict WRONG — unit-failed '$unit_failed', unit-passed '$unit_passed', suite-failed"
    echo "          '$suite_failed', execute-failed '$execute_failed', unfinished '$unfinished'"; rm -f "$tlog"; exit 1
  fi
  rm -f "$tlog"
  # The decision itself, not only the reader. Stub the suite with the logs that reach run_unit_tests
  # without a single case line and check each verdict: the rig that cannot test is infrastructure (2),
  # a finished run that selected nothing is the branch's (1), and neither is ever a pass. Until
  # 2026-10-30 the first of the two printed "not blamed" and returned 0 on zero tests.
  # ALLOW_UNTESTED is pointed at a scratch file here: a selftest must not eat an override an operator
  # has armed for the next gate run.
  no_sim() { run_suite() { printf '%s\n' "no xctestrun or no simulator '$TEST_SIM_NAME'" >"$2"; return 2; }; }
  no_sim_rc=0
  ( no_sim; ALLOW_UNTESTED=/nonexistent/no-override-armed
    run_unit_tests >/dev/null 2>&1 ) || no_sim_rc=$?
  nothing_rc=0
  ( run_suite() { printf '%s\n' "Test Suite 'Selected tests' passed at 2026-10-30 17:24:31.002." \
                                "** TEST SUCCEEDED **" >"$2"; return 0; }
    run_unit_tests >/dev/null 2>&1 ) || nothing_rc=$?
  if (( no_sim_rc == 2 && nothing_rc == 1 )); then
    echo "selftest: a run that executed no test never passes — untestable rig 2 (retried), nothing selected 1"
  else
    echo "selftest: WRONG — untestable rig returned $no_sim_rc (want 2), selected-nothing returned"
    echo "          $nothing_rc (want 1); a 0 there is a pass for a branch no test ran against"; exit 1
  fi
  # The operator's escape hatch has to work on the night it is needed, and has to be one-shot.
  allow=$(mktemp -t ios-gate-allow); echo "selftest: pretending to be the operator" >"$allow"
  armed_rc=0
  ( no_sim; ALLOW_UNTESTED="$allow"; run_unit_tests >/dev/null 2>&1 ) || armed_rc=$?
  if (( armed_rc == 0 )) && [[ ! -f "$allow" ]]; then
    echo "selftest: .allow-untested ships one untested run and is consumed by it"
  else
    echo "selftest: WRONG — armed override returned $armed_rc (want 0) and the file is"
    echo "          $([[ -f "$allow" ]] && echo "still there (want consumed)" || echo "gone")"; rm -f "$allow"; exit 1
  fi
  rm -f "$allow"
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
    # 2 keeps its meaning all the way out: infrastructure, retried, never a verdict on the branch.
    run_unit_tests; urc=$?
    case "$urc" in
      0) ;;
      2) rc=2 ;;
      *) rc=1 ;;
    esac
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

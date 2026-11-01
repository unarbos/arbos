#!/bin/bash
# iOS perf baseline rig (bc-85f873aa, 2026-11-01). Release + ARBOS_FIXTURE build on the
# "Arbos PerfBaseline" simulator. Fixture mode only: the relay is http://fixture.invalid.
# Start detached:  ( trap "" HUP PIPE; exec ~/arbos-perf-82aa-w/perf-rig.sh run <plan> ) > ~/arbos-perf-82aa-w/run.log 2>&1 < /dev/null &
export DEVELOPER_DIR=/Applications/Xcode-27.0.0.app/Contents/Developer
W=${W:-~/arbos-perf-82aa-w}; SRC=${SRC:-~/arbos-perf-82aa}; SLOT=/usr/local/bin/arbos-heavy-slot.sh
SLOTS=/Users/Shared/arbos-run/heavy-slots
SIM=${SIM:-717D7018-B970-4621-BC53-61346B36EE94}
DD=$W/dd; BUDGET=${BUDGET:-3300}
mkdir -p $W/logs $W/xcr $W/traces
say() { echo "[$(date -u +%H:%M:%S)] RIG $*"; }

release_all() {
  pkill -TERM -f "derivedDataPath $DD" 2>/dev/null
  pkill -TERM -f "xctrace record --device $SIM" 2>/dev/null
  sleep 3
  pkill -KILL -f "derivedDataPath $DD" 2>/dev/null
  pkill -KILL -f "xctrace record --device $SIM" 2>/dev/null
  xcrun simctl shutdown "$SIM" 2>/dev/null
  [ -n "${S:-}" ] && $SLOT release "$S" >/dev/null 2>&1
  say "released slot=${S:-none}"
}

live_waiters() {
  local n=0 m
  for m in "$1"/*; do [ -f "$m" ] && ps -p "$(basename "$m")" >/dev/null 2>&1 && n=$((n+1)); done
  echo "$n"
}

take_slot() {
  shopt -s nullglob
  for d in "$SLOTS"/*.slot; do
    [ -d "$d" ] || continue
    say "held: $(cat "$d/name" 2>/dev/null)"
    case "$(cat "$d/name" 2>/dev/null)" in mac-release*|tf-archive*) echo "STAND DOWN: release lane holds the Mac"; exit 4 ;; esac
  done
  [ "$(live_waiters "$SLOTS/.release-waiting")" -gt 0 ] && { echo "STAND DOWN: release lane queued"; exit 4; }
  [ "$(live_waiters "$SLOTS/.gate-waiting")" -gt 0 ] && { echo "STAND DOWN: gate half queued"; exit 4; }
  # Queue in the rig FIFO (fair share) rather than polling with --no-wait.
  S=""; ACQ=""
  trap 'kill -TERM ${ACQ:-0} 2>/dev/null; release_all; exit 9' INT TERM
  say "queueing for the slot"
  $SLOT acquire perf-baseline-82aa --pid $$ > $W/slot.out 2>$W/slot.err &
  ACQ=$!; wait $ACQ; RC=$?; ACQ=""
  S=$(grep -E "^/.*\.slot$" $W/slot.out 2>/dev/null | tail -1)
  [ -n "$S" ] || { echo "NO SLOT (rc=$RC): $(tail -2 $W/slot.err)"; exit 4; }
  case "$S" in *uncapped-*) echo "STAND DOWN: uncapped fall-through"; $SLOT release "$S" >/dev/null 2>&1; exit 4 ;; esac
  LEASE=$(cat "$S/lease" 2>/dev/null || echo 3600)
  BUDGET=$(( LEASE - 90 ))
  say "slot=$S lease=${LEASE}s budget=${BUDGET}s"
  trap 'release_all; exit 9' INT TERM
  ( sleep $BUDGET; kill -TERM $$ 2>/dev/null; sleep 20; kill -9 $$ 2>/dev/null ) & WD=$!
}

boot() {
  line=$(xcrun simctl list devices | grep "$SIM") || { echo "simulator $SIM not in this account's set"; exit 2; }
  say "device: $line"
  xcrun simctl boot "$SIM" 2>/dev/null
  xcrun simctl bootstatus "$SIM" -b >/dev/null 2>&1
}

build() {
  say "build-for-testing Release ARBOS_FIXTURE"
  ( cd $SRC/apps/ios/Arbos && nice -n 12 xcodebuild build-for-testing -project Arbos.xcodeproj -scheme ArbosPerf \
      -configuration Release -destination "id=$SIM" -derivedDataPath $DD \
      -jobs 6 ONLY_ACTIVE_ARCH=YES CODE_SIGNING_ALLOWED=NO SWIFT_ACTIVE_COMPILATION_CONDITIONS=ARBOS_FIXTURE ) > $W/logs/build.log 2>&1
  local rc=$?
  grep -E "error: |BUILD SUCCEEDED|TEST BUILD SUCCEEDED|BUILD FAILED" $W/logs/build.log | head -40
  say "build rc=$rc"
  return $rc
}

# test <name> <tag> [extra env...]: one test case, its xcodebuild log and the app's log.
runtest() {
  local t=$1 tag=$2; shift 2
  local T0; T0=$(date "+%Y-%m-%d %H:%M:%S")
  say "test $t tag=$tag $* load=[$(sysctl -n vm.loadavg)] other_xcodebuild=$(pgrep -f xcodebuild | grep -vc $$)"
  ( cd $SRC/apps/ios/Arbos && env TEST_RUNNER_ARBOS_PERF_BASELINE=1 TEST_RUNNER_ARBOS_PERF_ITERATIONS=${ITER:-5} "$@" nice -n 12 xcodebuild test-without-building \
      -project Arbos.xcodeproj -scheme ArbosPerf -configuration Release -destination "id=$SIM" -derivedDataPath $DD \
      -only-testing:ArbosUITests/PerfBaselineUITests/$t -parallel-testing-enabled NO \
      -collect-test-diagnostics never -resultBundlePath $W/xcr/$tag-$t.xcresult ) > $W/logs/$tag-$t.xcb.log 2>&1
  local rc=$?
  sleep 2
  xcrun simctl spawn "$SIM" log show --style compact --start "$T0" --predicate \
    '(subsystem == "com.unarbos.arbos.ios" AND (category == "perf-frames" OR category == "launch")) OR eventMessage BEGINSWITH "perfphase"' \
    > $W/logs/$tag-$t.app.log 2>&1
  say "test $t rc=$rc $(grep -cE ' late | second ' $W/logs/$tag-$t.app.log) probe lines; $(grep -E 'Test Case .*(passed|failed)' $W/logs/$tag-$t.xcb.log | tail -1)"
}

# trace <template> <test> <secs> <tag> [env...]: run one test and attach xctrace to the app it launches.
trace() {
  local tpl="$1" t=$2 secs=$3 tag=$4; shift 4
  local slug; slug=$(echo "$tpl" | tr ' ' '_')
  local out="$W/traces/$tag-$t-$slug.trace"
  rm -rf "$out"
  runtest $t $tag "$@" &
  local RT=$! pid="" i
  for i in $(seq 1 150); do
    pid=$(xcrun simctl spawn "$SIM" launchctl list 2>/dev/null | awk '/UIKitApplication:com.unarbos.arbos.ios\[/{print $1}' | grep -E '^[0-9]+$' | head -1)
    [ -n "$pid" ] && break; sleep 1
  done
  say "trace '$tpl' $t attach pid=${pid:-none}"
  if [ -n "$pid" ]; then
    xcrun xctrace record --device "$SIM" --template "$tpl" ${XINST:-} --attach "$pid" --time-limit ${secs}s --output "$out" > $W/logs/$tag-$t-$slug.xctrace.log 2>&1
    say "trace rc=$? size=$(du -sh "$out" 2>/dev/null | cut -f1) $(tail -1 $W/logs/$tag-$t-$slug.xctrace.log)"
  fi
  wait $RT
}

# simlaunch: five launches by simctl (no XCTest in the loop), then the app's own launch lines.
simlaunch() {
  local T0; T0=$(date "+%Y-%m-%d %H:%M:%S") i
  say "simlaunch load=[$(sysctl -n vm.loadavg)]"
  for i in 1 2 3 4 5 6; do
    SIMCTL_CHILD_ARBOS_FIXTURE=1 SIMCTL_CHILD_ARBOS_PERF_FRAMES=1 SIMCTL_CHILD_ARBOS_FIXTURE_HOME_ROWS=60 \
      xcrun simctl launch --terminate-running-process "$SIM" com.unarbos.arbos.ios >/dev/null 2>&1
    sleep 7
  done
  xcrun simctl terminate "$SIM" com.unarbos.arbos.ios 2>/dev/null
  sleep 2
  xcrun simctl spawn "$SIM" log show --style compact --start "$T0" --predicate \
    'subsystem == "com.unarbos.arbos.ios" AND (category == "perf-frames" OR category == "launch")' > $W/logs/sl-simlaunch.app.log 2>&1
  say "simlaunch lines=$(grep -c app-init $W/logs/sl-simlaunch.app.log)"
}

applaunch() {
  local out="$W/traces/tr-app-launch.trace"; rm -rf "$out"
  xcrun xctrace record --device "$SIM" --template "App Launch" --time-limit 20s --output "$out" \
    --env ARBOS_FIXTURE=1 --env ARBOS_FIXTURE_HOME_ROWS=60 \
    --launch -- "$DD/Build/Products/Release-iphonesimulator/Arbos.app" > $W/logs/tr-app-launch.xctrace.log 2>&1
  say "app-launch trace rc=$? size=$(du -sh "$out" 2>/dev/null | cut -f1) $(tail -1 $W/logs/tr-app-launch.xctrace.log)"
  xcrun simctl terminate "$SIM" com.unarbos.arbos.ios 2>/dev/null
}

# openprof <tag> <secs> KEY=VAL...: launch the app by simctl with the in-app open driver, attach
# Time Profiler (+ os_signpost for the open-chat intervals), then export the tables as XML.
openprof() {
  local tag=$1 secs=$2; shift 2
  local out="$W/traces/$tag.trace" kv envs=()
  rm -rf "$out"
  for kv in ARBOS_FIXTURE=1 ARBOS_FIXTURE_HOME_ROWS=60 "$@"; do envs+=("SIMCTL_CHILD_$kv"); done
  xcrun simctl terminate "$SIM" com.unarbos.arbos.ios 2>/dev/null; sleep 2
  say "openprof $tag load=[$(sysctl -n vm.loadavg)] $*"
  local pid try
  for try in 1 2 3; do
    pid=$(env "${envs[@]}" xcrun simctl launch --terminate-running-process "$SIM" com.unarbos.arbos.ios 2>/dev/null | awk -F': ' '{print $2}')
    sleep 1
    [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null && break
    say "openprof $tag launch $try: pid=${pid:-none} gone before attach; relaunching"
    pid=""; sleep 3
  done
  say "openprof $tag pid=${pid:-none}"
  [ -n "$pid" ] || return 1
  xcrun xctrace record --device "$SIM" --template "Time Profiler" --instrument os_signpost \
    --attach "$pid" --time-limit ${secs}s --output "$out" > $W/logs/$tag.xctrace.log 2>&1
  say "openprof $tag rc=$? size=$(du -sh "$out" 2>/dev/null | cut -f1) $(tail -1 $W/logs/$tag.xctrace.log)"
  xcrun simctl terminate "$SIM" com.unarbos.arbos.ios 2>/dev/null
}

# coldlaunch <n>: launches by simctl, alternating warm-up on / off (the order flips each round),
# then the app's launch milestones; the pid → arm map goes to cl-pids.txt. CL_ROWS: padded DMs
# added under the fixture's own rows. Even CL_ROWS=6 overflows the screen, so `first-content` (a
# sentinel after the last row) does not fire; the root view's `probe on` line is the first frame
# of the populated list (the fixture loads in `AppModel.init`).
coldlaunch() {
  local T0 i arm pid; T0=$(date "+%Y-%m-%d %H:%M:%S")
  : > $W/logs/cl-pids.txt
  say "coldlaunch load=[$(sysctl -n vm.loadavg)]"
  for i in $(seq 1 ${1:-6}); do
    for arm in $([ $((i % 2)) = 1 ] && echo "on off" || echo "off on"); do
      pid=$(env SIMCTL_CHILD_ARBOS_FIXTURE=1 SIMCTL_CHILD_ARBOS_PERF_FRAMES=1 SIMCTL_CHILD_ARBOS_FIXTURE_HOME_ROWS=${CL_ROWS:-60} \
        $([ $arm = off ] && echo SIMCTL_CHILD_ARBOS_NO_WARMUP=1) \
        xcrun simctl launch --terminate-running-process "$SIM" com.unarbos.arbos.ios 2>/dev/null | awk -F': ' '{print $2}')
      echo "$arm $i $pid" >> $W/logs/cl-pids.txt
      sleep 7
    done
  done
  xcrun simctl terminate "$SIM" com.unarbos.arbos.ios 2>/dev/null; sleep 2
  xcrun simctl spawn "$SIM" log show --style compact --start "$T0" --predicate \
    'subsystem == "com.unarbos.arbos.ios" AND (category == "perf-frames" OR category == "launch")' > $W/logs/cl-launch.app.log 2>&1
  say "coldlaunch lines=$(grep -c app-init $W/logs/cl-launch.app.log)"
}

# probebuild: the side-effect counter for the app just built, hooking every uniffi_ function
# and the system calls it imports (scripts/ios-perf/probe).
probebuild() {
  local app="$DD/Build/Products/Release-iphonesimulator/Arbos.app/Arbos"
  mkdir -p $W/probe && cp $SRC/scripts/ios-perf/probe/{gen.py,probe.c} $W/probe/ || return 1
  xcrun nm -u "$app" | grep -E '^_uniffi_' > $W/probe/imports.txt
  ( cd $W/probe && python3 gen.py imports.txt . && xcrun clang -dynamiclib -target arm64-apple-ios17.0-simulator \
      -isysroot "$(xcrun --sdk iphonesimulator --show-sdk-path)" -O1 probe.c hooks.s -o probe.dylib \
      -undefined dynamic_lookup -framework Foundation -lobjc 2>&1 | grep -v dynamic_lookup )
  say "probebuild $(ls -la $W/probe/probe.dylib 2>&1 | awk '{print $5}') bytes"
}

# proberun <tag> [KEY=VAL...]: one launch with the side-effect counter injected (probe/probe.dylib),
# its log, and the mtimes of every file the app's containers and the simulator keychain hold.
proberun() {
  local tag=$1 T0 kv envs=() pid d; shift
  T0=$(date "+%Y-%m-%d %H:%M:%S")
  for kv in ARBOS_FIXTURE=1 ARBOS_FIXTURE_HOME_ROWS=60 "DYLD_INSERT_LIBRARIES=$W/probe/probe.dylib" "$@"; do envs+=("SIMCTL_CHILD_$kv"); done
  pid=$(env "${envs[@]}" xcrun simctl launch --terminate-running-process "$SIM" com.unarbos.arbos.ios 2>/dev/null | awk -F': ' '{print $2}')
  say "proberun $tag pid=${pid:-none} $*"
  sleep 20
  xcrun simctl terminate "$SIM" com.unarbos.arbos.ios 2>/dev/null; sleep 2
  xcrun simctl spawn "$SIM" log show --style compact --start "$T0" --predicate \
    'subsystem == "com.unarbos.arbos.perfprobe" OR (subsystem == "com.unarbos.arbos.ios" AND category == "launch")' > $W/logs/$tag.probe.log 2>&1
  { for d in "$(xcrun simctl get_app_container "$SIM" com.unarbos.arbos.ios data 2>/dev/null)" \
        $(xcrun simctl get_app_container "$SIM" com.unarbos.arbos.ios groups 2>/dev/null | awk '{print $2}') \
        ~/Library/Developer/CoreSimulator/Devices/$SIM/data/Library/Keychains; do
      [ -d "$d" ] && find "$d" -type f -exec stat -f "%Fm %z %N" {} + 2>/dev/null
    done; } > $W/logs/$tag.files.txt
  say "proberun $tag $(grep -c 'probe count' $W/logs/$tag.probe.log) count lines; $(grep 'probe summary' $W/logs/$tag.probe.log | tail -1 | sed 's/.*probe summary//')"
}

# xtwarm: the first xctrace attach after a simulator boot fails ("Cannot find process for
# provided pid"); spend it on a throwaway launch so the next openprof gets a trace.
xtwarm() {
  local pid
  pid=$(SIMCTL_CHILD_ARBOS_FIXTURE=1 SIMCTL_CHILD_ARBOS_FIXTURE_HOME_ROWS=60 xcrun simctl launch --terminate-running-process "$SIM" com.unarbos.arbos.ios 2>/dev/null | awk -F': ' '{print $2}')
  sleep 2
  rm -rf $W/xtwarm.trace
  xcrun xctrace record --device "$SIM" --template "Time Profiler" --attach "$pid" --time-limit 3s --output $W/xtwarm.trace > $W/logs/xtwarm.xctrace.log 2>&1
  say "xtwarm pid=${pid:-none} rc=$? $(tail -1 $W/logs/xtwarm.xctrace.log)"
  rm -rf $W/xtwarm.trace
  xcrun simctl terminate "$SIM" com.unarbos.arbos.ios 2>/dev/null; sleep 2
}

# exportall: time-profile and os-signpost tables of every trace not yet exported (gzipped XML;
# symbolise with sim_symbol_addrs.py + sim-symbolicate.sh).
exportall() {
  local t n
  for t in $W/traces/*.trace; do
    n=${t%.trace}
    [ -f $n.tp.xml.gz ] && continue
    nice -n 12 xcrun xctrace export --input "$t" --xpath '/trace-toc/run[@number="1"]/data/table[@schema="time-profile"]' > $n.tp.xml 2>/dev/null
    nice -n 12 xcrun xctrace export --input "$t" --xpath '/trace-toc/run[@number="1"]/data/table[@schema="os-signpost"]' > $n.sp.xml 2>/dev/null
    gzip -f $n.tp.xml $n.sp.xml
    say "exported $(basename $n) $(du -h $n.tp.xml.gz | cut -f1)"
  done
}

ALL="testLaunchToChatList testOpenLongGroup testOpenSmallDM testSwitchChats testScrollChatList testScrollLongChat testComposerTypeAndSend testSettingsSheet testMessageMenu testStreamingAnswer"

case "${1:-}" in
  run)
    plan=${2:-pass1}
    take_slot; boot
    case "$plan" in
      buildpass)
        build || { kill $WD 2>/dev/null; release_all; exit 3; }
        for t in $ALL; do [ "$t" = testLaunchToChatList ] && continue; runtest $t p1; done ;;
      pass)
        for t in $ALL; do runtest $t ${3:-p2}; done ;;
      traces)
        runtest testLaunchToChatList p1
        simlaunch
        applaunch
        trace "Animation Hitches" testScrollLongChat 100 tr
        trace "SwiftUI" testStreamingAnswer 80 tr
        trace "Time Profiler" testStreamingAnswer 80 tr
        trace "Time Profiler" testOpenLongGroup 70 tr ;;
      glass)
        for t in testScrollChatList testScrollLongChat testStreamingAnswer; do runtest $t glass TEST_RUNNER_ARBOS_PERF_SURFACE=glass; done
        trace "Animation Hitches" testScrollLongChat 100 trglass TEST_RUNNER_ARBOS_PERF_SURFACE=glass ;;
      openprof)
        build || { kill $WD 2>/dev/null; release_all; exit 3; }
        xcrun simctl install "$SIM" "$DD/Build/Products/Release-iphonesimulator/Arbos.app"
        openprof open-dm 130 ARBOS_PERF_OPENS=fx-dm-unconst:22
        openprof open-group 85 ARBOS_FIXTURE_LONG_ROWS=2200 ARBOS_FIXTURE_LONG_RICH=1 ARBOS_PERF_OPENS=fx-group-launch:14 ;;
      # No build: the app from `openprof` is still installed. DM first, then the group, in
      # turn: open 0 is the first push after launch, open 1 the group's first open (not the
      # first push), the rest repeats of each.
      openmix)
        [ -d "$W/traces/open-dm.trace" ] || openprof open-dm 130 ARBOS_PERF_OPENS=fx-dm-unconst:22
        openprof open-mix 85 ARBOS_FIXTURE_LONG_ROWS=2200 ARBOS_FIXTURE_LONG_RICH=1 ARBOS_PERF_OPENS=fx-dm-unconst,fx-group-launch:14
        # The same group opens driven by XCUITest (accessibility on in the app), profiled.
        ITER=5 XINST="--instrument os_signpost" trace "Time Profiler" testOpenLongGroup 70 axg ;;
      # First-open warm-up A/B in one binary (ARBOS_NO_WARMUP=1 turns it off): cold launches
      # alternating on / off, the side-effect counter over the warm-up window, then profiled
      # opens, DM first then the long group, three launches per arm.
      warmab)
        build || { kill $WD 2>/dev/null; release_all; exit 3; }
        xcrun simctl install "$SIM" "$DD/Build/Products/Release-iphonesimulator/Arbos.app"
        xtwarm
        coldlaunch 6
        probebuild && { proberun probe-on-1; proberun probe-off-1 ARBOS_NO_WARMUP=1; proberun probe-on-2; }
        for i in 1 2 3; do
          openprof wu-on-$i 48 ARBOS_FIXTURE_LONG_ROWS=2200 ARBOS_FIXTURE_LONG_RICH=1 ARBOS_PERF_OPENS=fx-dm-unconst,fx-group-launch:6
          openprof wu-off-$i 48 ARBOS_FIXTURE_LONG_ROWS=2200 ARBOS_FIXTURE_LONG_RICH=1 ARBOS_PERF_OPENS=fx-dm-unconst,fx-group-launch:6 ARBOS_NO_WARMUP=1
        done ;;
      # No build: cold launches only, on the app the last plan installed.
      coldab)
        SIMCTL_CHILD_ARBOS_FIXTURE=1 xcrun simctl launch "$SIM" com.unarbos.arbos.ios >/dev/null 2>&1; sleep 8
        xcrun simctl terminate "$SIM" com.unarbos.arbos.ios 2>/dev/null
        coldlaunch ${3:-8}
        [ -n "${WU_EXTRA:-}" ] && openprof $WU_EXTRA 48 ARBOS_FIXTURE_LONG_ROWS=2200 ARBOS_FIXTURE_LONG_RICH=1 ARBOS_PERF_OPENS=fx-dm-unconst,fx-group-launch:6
        xcrun simctl shutdown "$SIM" 2>/dev/null
        exportall ;;
      # No build: one launch with the side-effect counter (network detail lines), on the app
      # the last plan installed.
      netprobe)
        SIMCTL_CHILD_ARBOS_FIXTURE=1 xcrun simctl launch "$SIM" com.unarbos.arbos.ios >/dev/null 2>&1; sleep 8
        xcrun simctl terminate "$SIM" com.unarbos.arbos.ios 2>/dev/null
        set -- ${NET_TAGS:-net-1 net-2}
        proberun $1
        proberun $2 ARBOS_NO_WARMUP=1
        [ "${NET_THEN_COLD:-}" = 1 ] && {
          coldlaunch ${CL_N:-8}
          [ -n "${WU_EXTRA:-}" ] && openprof $WU_EXTRA 48 ARBOS_FIXTURE_LONG_ROWS=2200 ARBOS_FIXTURE_LONG_RICH=1 ARBOS_PERF_OPENS=fx-dm-unconst,fx-group-launch:6
          xcrun simctl shutdown "$SIM" 2>/dev/null
          exportall
        } ;;
      # No build: the missing on-arm open profile, then the counter for the launch-time connect().
      onextra)
        xtwarm
        openprof ${WU_EXTRA:-wu-on-4} 48 ARBOS_FIXTURE_LONG_ROWS=2200 ARBOS_FIXTURE_LONG_RICH=1 ARBOS_PERF_OPENS=fx-dm-unconst,fx-group-launch:6
        proberun net-3
        proberun net-4 ARBOS_NO_WARMUP=1
        xcrun simctl shutdown "$SIM" 2>/dev/null
        exportall ;;
      only)
        tag=$3; shift 3; for t in "$@"; do runtest $t $tag; done ;;
      *) echo "plan?"; ;;
    esac
    kill $WD 2>/dev/null
    release_all ;;
  *) echo "usage: $0 run pass1|pass23|glass|only <tests>"; exit 1 ;;
esac

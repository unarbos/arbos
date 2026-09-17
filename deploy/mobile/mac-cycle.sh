#!/bin/bash
# One loop cycle on the Mac: build the iOS app for the simulator from a
# branch, boot an iPhone 15 Pro, run the local kernel, install and launch
# the app, take the stills. Output under ~/mobile-out/<cycle>/.
#   mac-cycle.sh <cycle> <branch> [extra launch args…]
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.9/bin:$HOME/.local/bin:$PATH"
CYCLE=${1:?cycle}; BRANCH=${2:?branch}; shift 2
OUT="$HOME/mobile-out/$CYCLE"; mkdir -p "$OUT"
REPO="$HOME/arbos"
cd "$REPO" && git fetch -q origin "$BRANCH" && git checkout -q -B "$BRANCH" "origin/$BRANCH" && git log --oneline -1 | tee "$OUT/build-sha.txt"

# Simulator: iPhone 15 Pro on the newest iOS runtime.
RUNTIME=$(xcrun simctl list runtimes -j | python3 -c 'import json,sys; rs=[r for r in json.load(sys.stdin)["runtimes"] if r["platform"]=="iOS" and r["isAvailable"]]; rs.sort(key=lambda r:[int(x) for x in r["version"].split(".")]); print(rs[-1]["identifier"])')
UDID=$(xcrun simctl list devices -j | python3 -c 'import json,sys; d=json.load(sys.stdin)["devices"]; print(next((x["udid"] for k,v in d.items() for x in v if x["name"]=="Arbos iPhone 15 Pro"), ""))')
if [ -z "$UDID" ]; then
  UDID=$(xcrun simctl create "Arbos iPhone 15 Pro" com.apple.CoreSimulator.SimDeviceType.iPhone-15-Pro "$RUNTIME")
fi
echo "sim $UDID on $RUNTIME" | tee "$OUT/sim.txt"
xcrun simctl boot "$UDID" 2>/dev/null || true
xcrun simctl bootstatus "$UDID" -b >/dev/null
xcrun simctl ui "$UDID" appearance dark

# Build.
cd "$REPO/ios"
[ -x scripts/gen-secrets.sh ] && [ -n "${OP_SERVICE_ACCOUNT_TOKEN:-}" ] && sh scripts/gen-secrets.sh 2>/dev/null || true
DERIVED="$HOME/mobile-derived"
xcodebuild -project Arbos.xcodeproj -scheme Arbos -configuration Debug -sdk iphonesimulator \
  -destination "id=$UDID" -derivedDataPath "$DERIVED" build 2>&1 \
  | tee "$OUT/xcodebuild.log" | grep -E "error:|warning: unre|BUILD (SUCCEEDED|FAILED)" | head -40
APP=$(find "$DERIVED/Build/Products/Debug-iphonesimulator" -maxdepth 1 -name "Arbos.app" | head -1)
[ -n "$APP" ] || { echo "no app built"; exit 1; }
xcrun simctl install "$UDID" "$APP"
BUNDLE=com.unarbos.arbos.ios
xcrun simctl terminate "$UDID" $BUNDLE 2>/dev/null || true
xcrun simctl launch --console-pty "$UDID" $BUNDLE "$@" > "$OUT/app-console.log" 2>&1 &
sleep 4
xcrun simctl io "$UDID" screenshot "$OUT/01-launch.png" >/dev/null
echo "launched; console in $OUT/app-console.log"

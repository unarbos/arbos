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
# A failed checkout used to be survivable. Scratch edits left in the tree
# made `git checkout` refuse, the `&&` chain stopped, and the script carried
# on to build, launch and report — naming a branch it was not on. Two cycles
# in a row built the previous tree and said BUILD SUCCEEDED. The tree the
# loop measures must be the branch the loop names, so make it so and stop
# if it cannot.
cd "$REPO" || exit 1
git fetch -q origin "$BRANCH" || { echo "cannot fetch $BRANCH"; exit 1; }
# Was anything discarded? It matters for the *binary*, not only the tree.
# Cycle 151: a build from a clean checkout of `main` behaved like code from
# before #711 — the separator it removed was back, six runs of six — and a
# manual rebuild from the identical commit was clean, six of six. The tree
# was right and the app was not.
#
# The cause is this loop's own habit: scp a modified source onto the Mac,
# build, then `git reset --hard` it away. The restored file can land with an
# older timestamp than the object built from the scratch version, and the
# incremental build keeps the object. Deleting the built products when the
# reset actually threw something away costs one full compile in that case
# and nothing the rest of the time.
DISCARDED=$(git status --porcelain | wc -l | tr -d ' ')
git reset -q --hard && git clean -qfd
if [ "$DISCARDED" != 0 ]; then
  echo "cleared $DISCARDED local change(s) — rebuilding the app from scratch so"
  echo "the binary cannot be older than the tree"
  rm -rf "$HOME/mobile-derived/Build/Products" 2>/dev/null
fi
git checkout -q -B "$BRANCH" "origin/$BRANCH" || { echo "cannot check out $BRANCH"; exit 1; }
HEAD_SHA=$(git rev-parse HEAD)
[ "$HEAD_SHA" = "$(git rev-parse "origin/$BRANCH")" ] || { echo "not on origin/$BRANCH after checkout"; exit 1; }
git log --oneline -1 | tee "$OUT/build-sha.txt"

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

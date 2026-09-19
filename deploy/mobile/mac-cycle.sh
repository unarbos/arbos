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
git reset -q --hard && git clean -qfd
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
BUNDLE=com.unarbos.arbos.ios
xcrun simctl install "$UDID" "$APP" || { echo "the app did not install — every"; \
  echo "run after this would have measured whatever was already on the device"; exit 1; }

# Is the app on the device actually this code?
#
# Cycle 151 built a clean checkout of `main` and then measured behaviour from
# before a fix that checkout contained — six runs of six — while a rebuild
# from the identical commit read clean. I blamed timestamps surviving a
# `git reset --hard` and wrote a remedy for that. Cycle 153 tried to
# reproduce it and could not: put a visible marker in a source file, build,
# reset, rebuild with nothing deleted, and the marker was gone, correctly.
# So the cause is still unknown.
#
# A remedy for a cause you cannot reproduce is a guess. A check for the thing
# that actually went wrong is not. Whatever makes the device disagree with the
# tree — an install that failed unnoticed, a build that did not take — leaves
# an app older than the newest source, so that is what is asked.
INSTALLED=$(xcrun simctl get_app_container "$UDID" $BUNDLE app 2>/dev/null)
if [ -n "$INSTALLED" ]; then
  # `find -newermt @<epoch>` does not parse on BSD find and silently matches
  # nothing, which reads as a pass. A reference file carrying the app's own
  # date is compared instead.
  REF=$(mktemp) && touch -r "$INSTALLED/Arbos" "$REF"
  NEWER=$(find "$REPO/ios" -name '*.swift' -newer "$REF" -print -quit 2>/dev/null)
  rm -f "$REF"
  if [ -n "$NEWER" ]; then
    echo "the app on the device is older than $(basename "$NEWER")."
    echo "Nothing measured after this would be about the code that was checked out."
    exit 1
  fi
  echo "the app on the device is newer than every source file"
else
  echo "cannot find the installed app to date it — not claiming it is current"
fi
xcrun simctl terminate "$UDID" $BUNDLE 2>/dev/null || true
xcrun simctl launch --console-pty "$UDID" $BUNDLE "$@" > "$OUT/app-console.log" 2>&1 &
sleep 4
xcrun simctl io "$UDID" screenshot "$OUT/01-launch.png" >/dev/null
echo "launched; console in $OUT/app-console.log"

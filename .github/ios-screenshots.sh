#!/usr/bin/env bash
#
# Photograph the iPhone app on a booted simulator, in both of bittensor's themes,
# and check the things only a running app can tell you.
#
# The app's one screen is a Metal render of a projected polytope. A build that
# compiles says nothing about whether that figure drew, whether the bundled font
# resolved, or whether the figure holds still off a call and turns on one — and
# none of it can be seen on the Linux box the code is written on. So this launches
# the real app and takes two frames 1.2s apart in each state:
#
#   off a call  the two frames must be identical. Anything else means the figure
#               is still drifting when it should be parked.
#   on a call   they must differ. Identical frames here would mean either that the
#               rotation never started or that Metal drew nothing at all, which is
#               also how a blank orb would show up.
#
# Then the Keychain, for the same reason: an API key that the Keychain accepts and
# forgets reads back fine in the launch that wrote it, so only a second process can
# catch it. The app lost a key that way once.
#
# Needs UDID and APP_ID in the environment, and the built .app under
# $RUNNER_TEMP/DerivedData, signed with .github/ios-simulator.entitlements.
set -euo pipefail

app="$RUNNER_TEMP/DerivedData/Build/Products/Debug-iphonesimulator/Arbos.app"
out="${SHOTS_DIR:-$RUNNER_TEMP/shots}"
mkdir -p "$out"

xcrun simctl install "$UDID" "$app"

# A dummy key and address stand in for the Keychain and for a real server, so the
# screens the app spends its life on can be reached. Both are DEBUG-only launch
# arguments and neither leaves the simulator; the address is deliberately
# unroutable.
configured=(-previewKey sk-screenshot-not-a-real-key -selfHostedURL wss://screenshot.invalid/ws)

# shoot <name> <settle seconds> [extra launch args...]
shoot() {
  local name=$1 settle=$2
  shift 2
  xcrun simctl terminate "$UDID" "$APP_ID" >/dev/null 2>&1 || true
  xcrun simctl launch "$UDID" "$APP_ID" "${configured[@]}" "$@" >/dev/null
  # Long enough for the launch animation to finish and for the alpha chase to
  # settle, so a frame pair is only comparing the figure.
  sleep "$settle"
  xcrun simctl io "$UDID" screenshot --type=png "$out/$name-a.png" >/dev/null 2>&1
  sleep 1.2
  xcrun simctl io "$UDID" screenshot --type=png "$out/$name-b.png" >/dev/null 2>&1
  echo "  $name"
}

states=(resting oncall reply settings)

for mode in light dark; do
  echo "$mode:"
  xcrun simctl ui "$UDID" appearance "$mode"
  shoot "$mode-resting" 6
  # `listening` is mid-utterance, so the caption is the words being spoken;
  # `speaking` is the reply that replaced them.
  shoot "$mode-oncall" 6 -previewPhase listening
  shoot "$mode-reply" 6 -previewPhase speaking
  shoot "$mode-settings" 5 -previewSettings 1
done

# Last, because it puts a key in the Keychain and the screens above are
# photographed with nothing in it. Two launches: one writes the key through the
# same call Settings makes, the next has to find it. One launch cannot tell the
# difference, since a write reads back fine in the process that made it.
launch_quietly() {
  xcrun simctl terminate "$UDID" "$APP_ID" >/dev/null 2>&1 || true
  xcrun simctl launch "$UDID" "$APP_ID" "$@" >/dev/null
  sleep 3
}
launch_quietly -previewSaveKey sk-keychain-round-trip
launch_quietly

xcrun simctl terminate "$UDID" "$APP_ID" >/dev/null 2>&1 || true
xcrun simctl spawn "$UDID" log show --last 10m --style compact \
  --predicate 'process == "Arbos"' > "$out/app.log" 2>/dev/null || true

fail=0

for mode in light dark; do
  if cmp -s "$out/$mode-resting-a.png" "$out/$mode-resting-b.png"; then
    echo "$mode: the figure held still off the call"
  else
    echo "::error title=The figure moves when it should be parked::$mode: two frames 1.2s apart differ with no call on the line. E8OrbView only advances the projection while phase.inCall."
    fail=1
  fi

  if cmp -s "$out/$mode-oncall-a.png" "$out/$mode-oncall-b.png"; then
    echo "::error title=The figure does not turn during a call::$mode: two frames 1.2s apart are identical while the phase is listening. Either the rotation never started, or Metal drew nothing and the orb is blank."
    fail=1
  else
    echo "$mode: the figure turned during the call"
  fi
done

# The page is white, on a phone set to either appearance. A patch of bare page
# below the figure and above the caption, which every screen leaves empty, has to
# come back one colour and that colour has to be #ffffff. Probed rather than
# compared against the light shot, because two launches of a screen with a turning
# figure never land on the same frame.
page=(90 1900 120 100)
for mode in light dark; do
  for state in "${states[@]}"; do
    read -r count r g b <<<"$(python3 .github/ios-shot-probe.py "$out/$mode-$state-a.png" "${page[@]}" | head -1)"
    if [ "$count" = 12000 ] && [ "$r$g$b" = "255255255" ]; then
      echo "$mode-$state: the page is white"
    else
      echo "::error title=The page is not white on $mode-$state::The bare patch at ${page[0]},${page[1]} came back $r $g $b over $count of 12000 pixels. The app is pinned to the light appearance with INFOPLIST_KEY_UIUserInterfaceStyle; something here is reading the system appearance anyway."
      fail=1
    fi
  done
done

# Settings has no figure on it, so it is the one screen that can be held to being
# the same picture whichever way the phone is set — which catches dark text or a
# dark keyboard, things a patch of bare page would not. Every screen with the
# figure on it is out: the projection starts from a random basis, so each launch
# parks it at a different pose and no two launches agree.
if cmp -s "$out/light-settings-a.png" "$out/dark-settings-a.png"; then
  echo "settings: identical on a dark phone"
else
  echo "::error title=Settings follows the phone::The sheet differs between a light and a dark simulator even though the app is pinned to Light. Text or the keyboard is still reading the system appearance."
  fail=1
fi

# The orb logs and gives up rather than taking the process down, so a failure here
# is otherwise invisible: the app just runs without its one screen.
if grep -q 'E8 orb:' "$out/app.log" 2>/dev/null; then
  echo "::error title=The orb could not start::Metal reported a failure; see app.log in the artifact."
  grep 'E8 orb:' "$out/app.log" | head -5
  fail=1
fi

# Likewise the theme: a font that does not resolve falls back silently, so it says
# so in the log and this is the only place that reads it.
if grep -q 'is not in the bundle' "$out/app.log" 2>/dev/null; then
  echo "::error title=A bundled font did not resolve::The theme fell back to the system monospace; see app.log in the artifact."
  grep 'is not in the bundle' "$out/app.log" | head -5
  fail=1
fi

# -34018 is errSecMissingEntitlement. It means the app was signed without
# `application-identifier`, so no Keychain call in it can work and the two
# assertions below would fail for a reason that has nothing to do with the app.
if grep -q 'Keychain read failed for .*: -34018' "$out/app.log" 2>/dev/null; then
  echo "::error title=The app has no Keychain entitlement::Every Keychain call failed with -34018, so this run proves nothing about the key. Check that the build signed with .github/ios-simulator.entitlements."
  fail=1
fi

if grep -q 'Keychain check: wrote, stored=yes readback=match' "$out/app.log" 2>/dev/null; then
  echo "the Keychain took the key"
else
  echo "::error title=The Keychain would not take the key::saveOpenAIKey could not write the key, or could not read back what it wrote. See 'Keychain check' in app.log."
  grep 'Keychain check' "$out/app.log" | tail -5
  fail=1
fi

# The one that matters: a different process has to find the key. A write that
# reads back inside the launch that made it still looks saved and comes back
# empty next time, which is how the key went missing before.
if [ "$(grep 'Keychain check:' "$out/app.log" | tail -1 | sed 's/.*Keychain check: //')" = "read a key" ]; then
  echo "the key survived a relaunch"
else
  echo "::error title=The key did not survive a relaunch::A later launch of the app read nothing back. Typing a key into Settings would not stick."
  grep 'Keychain check' "$out/app.log" | tail -5
  fail=1
fi

ls -la "$out"
exit "$fail"

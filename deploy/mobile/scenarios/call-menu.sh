#!/bin/bash
# The call's menu: what it offers, and whether the toggle names the right way.
#
#   call-menu.sh <cycle>
#
# Nothing had ever opened it. Three items live there — `Back to chat`,
# the speaker toggle, and `Hang up` — and the last two only while a call is
# up.
#
# The toggle is the one worth a check. Its label names the route you are
# **not** on: "Use speaker" while on the headset, "Use headset" while on the
# speaker. A label like that is one `!` away from telling every caller to
# switch to the thing they are already using, and nothing on screen would
# look wrong — the button is there, it is spelled correctly, and it works.
# Only tapping it and reading it again can tell.
#
# The route itself is not checked here and cannot be: the coverage row
# "call — AirPods / speaker route, screen off, CallKit" needs a real phone.
# What the menu *says* is a different claim and does not.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/call-menu"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
CLIP=${CLIP:-$HOME/mobile-clips/small.wav}
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
items() { ui dump | grep -oE "Button +(Back to chat|Use speaker|Use headset|Hang up)" | sed 's/Button *//' | tr '\n' ';'; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 -previewCall 1 -injectWav "$CLIP" >/dev/null 2>&1
sleep 12

FAULTS=0
echo "== the menu, mid-call =="
ui tap "Call menu" >/dev/null 2>&1 || { echo "  no Call menu button"; exit 1; }
sleep 2
FIRST=$(items)
xcrun simctl io "$UDID" screenshot "$OUT/01-menu.png" >/dev/null 2>&1
echo "  offers: ${FIRST:-nothing}"
case "$FIRST" in
  *"Back to chat"*) ;;
  *) echo "  FAULT: no way back to the chat"; FAULTS=$((FAULTS + 1));;
esac
TOGGLE=$(echo "$FIRST" | grep -oE "Use (speaker|headset)" | head -1)
if [ -z "$TOGGLE" ]; then
  echo "  no route toggle — is the call up? (it only appears mid-call)"
  echo "  Nothing below is about the toggle."
else
  echo
  echo "== the toggle names the other route =="
  echo "  before: $TOGGLE"
  ui tap "$TOGGLE" >/dev/null 2>&1
  sleep 2
  ui tap "Call menu" >/dev/null 2>&1
  sleep 2
  AFTER=$(items | grep -oE "Use (speaker|headset)" | head -1)
  echo "  after:  ${AFTER:-nothing}"
  if [ -z "$AFTER" ]; then
    echo "  FAULT: the toggle vanished after being used"
    FAULTS=$((FAULTS + 1))
  elif [ "$AFTER" = "$TOGGLE" ]; then
    echo "  FAULT: it still offers '$AFTER' — the label does not follow the route,"
    echo "         so it is telling the caller to switch to what they are on"
    FAULTS=$((FAULTS + 1))
  else
    echo "  it flipped, so the label follows the route"
  fi
fi

echo
echo "== back to the chat =="
ui dump | grep -qE "Button +Back to chat" || { ui tap "Call menu" >/dev/null 2>&1; sleep 2; }
ui tap "Back to chat" >/dev/null 2>&1
sleep 3
xcrun simctl io "$UDID" screenshot "$OUT/02-back.png" >/dev/null 2>&1
if ui field >/dev/null 2>&1; then
  echo "  landed in a chat, composer and all"
else
  echo "  FAULT: 'Back to chat' did not reach a chat"
  FAULTS=$((FAULTS + 1))
fi

echo
if [ "$FAULTS" = 0 ]; then
  echo "VERDICT: the call's menu offers a way back and a route toggle that names"
  echo "         the route you are not on, and Back to chat lands in the chat"
else
  echo "VERDICT: $FAULTS fault(s) in the call's menu — named above"
fi
echo "stills in $OUT"

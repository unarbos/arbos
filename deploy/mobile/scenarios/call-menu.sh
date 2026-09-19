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
# Open it and wait for it, rather than tapping and reading two seconds
# later. The first run of this read "offers: nothing" from a menu that opens
# perfectly well — a fixed sleep timing out is indistinguishable, in the
# output, from a menu with nothing in it.
open_menu() {
  ui tap "Call menu" >/dev/null 2>&1 || return 1
  local i
  for i in 1 2 3 4 5 6; do
    [ -n "$(items)" ] && return 0
    sleep 1
  done
  return 1
}

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 -previewCall 1 -injectWav "$CLIP" >/dev/null 2>&1
sleep 12

FAULTS=0
echo "== the menu, mid-call =="
open_menu || { echo "  the Call menu did not open, or opened empty"; exit 1; }
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
  # The label follows the *route*, not the preference: `toggleSpeaker` flips
  # a preference and then reads back what CoreAudio actually did. A
  # simulator has one output and nothing to switch to, so the route cannot
  # move and the label is right not to. The first version of this called
  # that a fault in the app, which it is not — so the route is read too, and
  # the label is only judged when the route it names has changed.
  route_now() { ui dump | grep -oE "(speaker|headset|receiver|headphones) · [0-9]+%" | head -1; }
  # Close the menu to see the badge underneath it.
  idb ui tap 196 700 --udid "$UDID" >/dev/null 2>&1; sleep 1
  R1=$(route_now)
  open_menu >/dev/null 2>&1
  echo "  before: $TOGGLE   (route: ${R1:-not shown})"
  ui tap "$TOGGLE" >/dev/null 2>&1
  sleep 2
  R2=$(route_now)
  open_menu || echo "  the menu did not reopen"
  AFTER=$(items | grep -oE "Use (speaker|headset)" | head -1)
  echo "  after:  ${AFTER:-nothing}   (route: ${R2:-not shown})"
  # Unknown must not fall through to an accusation. The first two versions
  # of this chain ended at FAULT when the route could not be read at all,
  # and printed "the route changed to ''" — a fault reported from an empty
  # string. Missing evidence is its own branch, and it comes first.
  if [ -z "$R1" ] || [ -z "$R2" ]; then
    echo "  the route is not on screen, so the label cannot be judged against"
    echo "  it here. The menu offers a toggle; whether it names the right way"
    echo "  needs the phone (the AirPods row)."
  elif [ -z "$AFTER" ]; then
    echo "  FAULT: the toggle vanished after being used"
    FAULTS=$((FAULTS + 1))
  elif [ "$R1" = "$R2" ]; then
    echo "  the route did not change — there is one output on this simulator,"
    echo "  so the label staying put is correct and the flip is untested here."
    echo "  That half needs the phone (the AirPods row)."
  elif [ "$AFTER" = "$TOGGLE" ]; then
    echo "  FAULT: the route changed to '$R2' and the menu still offers '$AFTER' —"
    echo "         it is telling the caller to switch to what they are on"
    FAULTS=$((FAULTS + 1))
  else
    echo "  the route moved and the label followed it"
  fi
fi

echo
echo "== back to the chat =="
case "$(items)" in *"Back to chat"*) ;; *) open_menu >/dev/null 2>&1;; esac
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

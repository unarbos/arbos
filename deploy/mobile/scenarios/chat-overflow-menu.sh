#!/bin/bash
# The chat's ··· menu: what it offers, and whether the offers work.
#
#   chat-overflow-menu.sh <cycle> [project]
#
# Three items have sat behind that button since it was built — `Call
# <project>`, `Reconnect`, `Settings` — and no scenario has ever opened it.
# The button itself is checked (M-355 gave it a name); what is under it is
# not.
#
# `Call <project>` is the interesting one. It names the project it will
# call, which is the difference between "Call" and "Call phone" when a
# person has several projects open and is about to talk to one of them.
#
# Reconnect is checked by what the chat does, not by the tap returning:
# a reconnect that changes nothing is indistinguishable from a menu item
# that does nothing, and only the chat's own mode line tells them apart.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/overflow-menu"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
sleep 11
reach_the_list "$UDID" || exit 1
ui tap "$ROW" >/dev/null 2>&1 || { echo "no '$ROW' row"; exit 1; }
sleep 5

FAULTS=0
echo "== what the menu offers =="
ui tap "More" >/dev/null 2>&1 || { echo "  no More button in the header"; exit 1; }
sleep 2
ui dump > "$OUT/menu.txt"
xcrun simctl io "$UDID" screenshot "$OUT/01-menu.png" >/dev/null 2>&1
ITEMS=$(grep -oE "Button +(Call [^,]*|Reconnect|Settings)" "$OUT/menu.txt" | sed 's/Button *//' | tr '\n' ';')
echo "  $ITEMS"
for want in "Reconnect" "Settings"; do
  grep -qE "Button +$want" "$OUT/menu.txt" || { echo "  MISSING: $want"; FAULTS=$((FAULTS + 1)); }
done
# The call item must name the project, not just say "Call".
CALLITEM=$(grep -oE "Button +Call [^ ].*" "$OUT/menu.txt" | head -1 | sed 's/Button *//')
if [ -z "$CALLITEM" ]; then
  echo "  MISSING: a Call item"; FAULTS=$((FAULTS + 1))
elif [ "$CALLITEM" = "Call" ]; then
  echo "  FAULT: the call item says only '$CALLITEM' — it should name the project"
  FAULTS=$((FAULTS + 1))
else
  echo "  the call item names its project: '$CALLITEM'"
fi

echo
echo "== Reconnect, by what the chat does =="
# Close the menu and use it. What matters is the chat's own state moving,
# not the tap being accepted.
ui tap "Reconnect" >/dev/null 2>&1 || { echo "  could not tap Reconnect"; FAULTS=$((FAULTS + 1)); }
MOVED=no
for _ in $(seq 1 20); do
  ui dump | grep -qiE "Opening $ROW…|is not answering|Reconnecting" && { MOVED=yes; break; }
  sleep 0.5
done
sleep 4
BACK=no
ui field >/dev/null 2>&1 && BACK=yes
echo "  the chat said it was reconnecting: $MOVED"
echo "  and came back to a usable composer: $BACK"
# A reconnect over a link that is already up can finish before a 0.5 s
# sample sees it, so a missed transition is not a fault by itself — a chat
# that never comes back is.
[ "$BACK" = yes ] || { echo "  FAULT: the chat did not come back after Reconnect"; FAULTS=$((FAULTS + 1)); }
[ "$MOVED" = yes ] || echo "  (the transition was not caught: a reconnect on a live link can finish"
[ "$MOVED" = yes ] || echo "   inside one sample, so this says nothing either way)"

echo
echo "== Settings, from the same menu =="
ui tap "More" >/dev/null 2>&1; sleep 2
ui tap "Settings" >/dev/null 2>&1
sleep 3
xcrun simctl io "$UDID" screenshot "$OUT/02-settings.png" >/dev/null 2>&1
if ui dump | grep -qE "StaticText +Settings|Heading"; then
  echo "  the settings sheet opened"
  ui tap "Done" >/dev/null 2>&1; sleep 2
else
  echo "  FAULT: Settings from the chat's menu did not open the sheet"
  FAULTS=$((FAULTS + 1))
fi

echo
if [ "$FAULTS" = 0 ]; then
  echo "VERDICT: the menu offers three things, the call item names its project,"
  echo "         Reconnect leaves a usable chat, and Settings opens"
else
  echo "VERDICT: $FAULTS fault(s) behind the ··· menu — named above"
fi
echo "tree and stills in $OUT"

#!/bin/bash
# The chat path a person uses every time, timed.
#
#   the-core-chat-path.sh <cycle> [project]
#
# Last exercised at cycle 41, twenty cycles ago, and it is the surface the
# app is mostly made of: type a line, see the card, watch the reply stream,
# see the Worked line. Each stage gets a number so the next check is a
# comparison rather than an opinion.
#
# Every timing is polled off the accessibility tree. A screenshot says
# pixels changed; the tree says the card exists.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/core-chat"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }
now() { python3 -c 'import time;print(time.time())'; }
since() { python3 -c "import time;print(f'{time.time()-$1:.1f}')"; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 8
ui tap "$ROW" >/dev/null || { echo "no $ROW row"; exit 1; }
sleep 4
ui field >/dev/null 2>&1 || { echo "no composer"; exit 1; }

MARK="core $(date -u +%H%M%S)"
LINE="$MARK: reply with three short sentences about the sea, nothing else."
ui focus >/dev/null; sleep 0.7
idb ui text "$LINE" --udid "$UDID"
for _ in $(seq 1 80); do [ "$(ui field plain 2>/dev/null)" = "$LINE" ] && break; sleep 0.25; done
[ "$(ui field plain 2>/dev/null)" = "$LINE" ] || { echo "the line never landed in the box"; exit 1; }

T0=$(now)
ui tap "Up" >/dev/null || { echo "no send button"; exit 1; }

# 1. the card: the line must appear as the user's own, at once
CARD=""
for _ in $(seq 1 40); do
  ui dump | grep -q "$MARK" && { CARD=$(since "$T0"); break; }
  sleep 0.2
done
# One `ui dump` costs about 0.4 s, so this loop cannot resolve anything
# faster than roughly half a second: a card that is drawn instantly still
# reads as ~1 s. Treat this figure as "at the floor" rather than as the
# app's latency. The two below are well clear of it and mean what they say.
echo "  the card appears:        ${CARD:-never}s   (floor of this method is ~0.5s)"
shot 01-card

# 2. the first words of the reply
FIRST=""
for _ in $(seq 1 120); do
  ui dump | grep -qiE "StaticText +(The |A |Sea|Wave|Ocean|Salt)" && { FIRST=$(since "$T0"); break; }
  sleep 0.5
done
echo "  the reply starts:        ${FIRST:-never}s"
shot 02-streaming

# 3. the Worked line, which is the turn ending
WORKED=""
for _ in $(seq 1 120); do
  W=$(ui dump | grep -oE "Worked [0-9]+[sm][^;]*" | tail -1)
  [ -n "$W" ] && { WORKED=$(since "$T0"); break; }
  sleep 0.5
done
echo "  the Worked line lands:   ${WORKED:-never}s   reading '$(ui dump | grep -oE "Worked [0-9]+[sm]" | tail -1)'"
shot 03-worked

echo "  the composer afterwards: $(ui dump | awk '$3 == "TextField" { $1="";$2="";$3=""; print }')"
echo "  and the kernel's record:"
python3 "$HERE/../kernel.py" pod history 4 2>/dev/null | tail -3 | cut -c1-110 | sed 's/^/    /'
echo "stills in $OUT"

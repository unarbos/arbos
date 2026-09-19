#!/bin/bash
# COVERS: project chat — send, prompt card, streaming, Worked line
# Stop: does the square end the turn, or only the screen?
#
#   stop-a-turn.sh <cycle> [project]
#
# The journey's J5 has said "Stop not exercised by this step (the control
# exists)" since it was written, and nothing else touches it. The control
# was added because the kernel's own stall line tells people to use it —
# "Stop ends the turn" — so a Stop that only quietens the phone would be
# the app repeating a promise the kernel made and not keeping it.
#
# The turn is therefore judged on **the kernel's record**, not on the square
# going away. A composer that stops looking busy while the kernel works on
# is the exact failure worth catching, and from the screen alone the two are
# identical.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}; TARGET=${3:-pod}
OUT="$HOME/mobile-out/$CYCLE/stop-a-turn"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
total() { python3 "$HERE/../kernel.py" "$TARGET" total 2>/dev/null | tail -1; }
tail_lines() { python3 "$HERE/../kernel.py" "$TARGET" history "${1:-6}" 2>/dev/null; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
sleep 11
reach_the_list "$UDID" || exit 1
ui tap "$ROW" >/dev/null 2>&1 || { echo "no '$ROW' row"; exit 1; }
sleep 5
ui field >/dev/null 2>&1 || { echo "no composer in '$ROW'"; exit 1; }

MARK="stop $(date -u +%H%M%S)"
LINE="$MARK: run the bash command sleep 75 and nothing else, then reply done."
BEFORE=$(total)
case $BEFORE in ''|*[!0-9]*) echo "cannot read $TARGET's transcript — stopping rather than guessing"; exit 1;; esac

echo "== send something long enough to stop =="
# Written yesterday, and it typed and sent blind — the fault every other
# scenario here had already fixed in its own body, which is precisely why it
# came back in a new file. `type_line` refuses what idb drops and reads the
# field back, in one place.
type_line "$UDID" "$LINE" || {
  echo
  echo "VERDICT: none — the line never reached the composer, so no turn was"
  echo "         started and nothing could be stopped"
  exit 1
}
ui tap "Send" >/dev/null 2>&1
SAW_STOP=no
for _ in $(seq 1 30); do
  case "$(ui dump)" in *"Button       Stop"*|*"Button  Stop"*) SAW_STOP=yes; break;; esac
  sleep 1
done
echo "  the send disc became Stop: $SAW_STOP"
xcrun simctl io "$UDID" screenshot "$OUT/01-running.png" >/dev/null 2>&1
if [ "$SAW_STOP" != yes ]; then
  echo
  echo "VERDICT: none — no Stop appeared, so there was nothing to press and"
  echo "         nothing below would be about stopping a turn"
  exit 1
fi

echo
echo "== press it =="
ui tap "Stop" >/dev/null 2>&1
GONE=no
for _ in $(seq 1 20); do
  case "$(ui dump)" in *"Button       Stop"*|*"Button  Stop"*) ;; *) GONE=yes; break;; esac
  sleep 1
done
echo "  the square went away: $GONE"
sleep 6
xcrun simctl io "$UDID" screenshot "$OUT/02-stopped.png" >/dev/null 2>&1

echo
echo "== what the kernel did =="
# The turn must be over there too. `sleep 75` is long enough that a kernel
# still working is unmistakable: check twice, half a minute apart, and a
# transcript that keeps growing is a turn that did not stop.
A=$(total); sleep 30; C=$(total)
echo "  transcript right after Stop: $A"
echo "  and thirty seconds later:    $C"
tail_lines 4 | sed 's/^/    /'
ENDED=no
case "$(tail_lines 6)" in *turn_complete*) ENDED=yes;; esac
echo "  the turn is closed in the record: $ENDED"

echo
if [ "$GONE" = yes ] && [ "$A" = "$C" ] && [ "$ENDED" = yes ]; then
  echo "VERDICT: Stop ends the turn — the square goes, the kernel's transcript"
  echo "         stops growing, and the turn is closed in its record"
elif [ "$GONE" = yes ] && [ "$A" != "$C" ]; then
  echo "VERDICT: the square went but the kernel kept going — $A → $C in thirty"
  echo "         seconds. The phone is repeating a promise the kernel made"
  echo "         ('Stop ends the turn') and not keeping it"
elif [ "$GONE" != yes ]; then
  echo "VERDICT: the square stayed after being pressed — nothing visibly stopped"
else
  echo "VERDICT: the transcript stopped growing but no turn_complete is in the"
  echo "         record — stopped, or stalled and indistinguishable from here"
fi
echo "stills in $OUT"

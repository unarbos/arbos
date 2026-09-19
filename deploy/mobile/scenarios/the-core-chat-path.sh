#!/bin/bash
# COVERS: project chat — send, prompt card, streaming, Worked line
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
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }
now() { python3 -c 'import time;print(time.time())'; }
since() { python3 -c "import time;print(f'{time.time()-$1:.1f}')"; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 8
reach_the_list "$UDID" || exit 1
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
ui tap "Send" >/dev/null || { echo "no send button"; exit 1; }

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

# 2. the first words of the reply, and how it arrives
#
# One pass, not two. The first version measured the first word, and only
# then began sampling the reply's length — by which time a three-sentence
# answer is already whole, so it could never see the growth it was looking
# for. Watching from the send means the same loop gives both.
#
# Measured as the transcript's total text, not as "the last line matching a
# word the reply might start with". That first attempt picked whichever line
# matched last — including the *previous* turn's reply — and reported
# lengths of 243 then 123. A length that goes **down** is not a reply
# growing, and it is the only reason the flaw was visible at all.
#
# A total over every row only grows while a turn runs, whatever the model
# happens to say, so it needs no guess about the reply's first word.
FIRST=""
LENGTHS=""
BASE=""
chars_in() { echo "$1" | grep -E "StaticText" | awk '{ n += length($0) } END { print n+0 }'; }
for _ in $(seq 1 200); do
  # One dump, used for all three readings. Taking a second one for the busy
  # comparison halved the sampling rate and missed the Stop phase of a
  # 4.7-second turn entirely.
  DUMP=$(ui dump)
  N=$(chars_in "$DUMP")
  [ -n "$BASE" ] || BASE=$N
  if [ "$N" -gt "$BASE" ]; then
    [ -n "$FIRST" ] || FIRST=$(since "$T0")
    case " $LENGTHS " in *" $N "*) ;; *) LENGTHS="$LENGTHS $N";; esac
  fi
  # Stop when the turn ends rather than after a fixed count: a reply still
  # growing must not be cut off by the sampler.
  # While the turn runs, three things on this screen claim to know whether
  # the project is busy, and cycle 187 caught them disagreeing: the composer
  # showed Stop and the transcript showed "Working" while the pill read
  # "✓ Agents 48" — a tick, which reads as everything finished.
  #
  # The pill counts running *workers* (`chat.running`), and a root turn with
  # no sub-agents is not one. Correct by its own definition, and still a
  # screen saying two things. Recorded here, not judged: whether the pill
  # should follow the root turn is a design decision, filed rather than
  # guessed at.
  if [ -z "${BUSY_SEEN:-}" ] && echo "$DUMP" | grep -qE "Button +Stop$"; then
    BUSY_SEEN=$(echo "$DUMP" | grep -oE "(Agents|Working) [0-9]+" | head -1)
    BUSY_SEEN=${BUSY_SEEN:-no pill}
  fi
  echo "$DUMP" | grep -qE "Worked [0-9]+[sm]" && break
  sleep 0.3
done
echo "  the reply starts:        ${FIRST:-never}s"
shot 02-streaming

# Streaming is the row's third word, and this file has taken a still called
# `02-streaming` for eighty cycles without measuring it. A reply that
# arrives whole and one that grows word by word pass every timing here
# identically; only the caller sees the difference, for the whole length of
# the answer.
STEPS=$(echo $LENGTHS | wc -w | tr -d ' ')
SPAN=$(echo $LENGTHS | awk '{print $1 " → " $NF}')
[ -n "${BUSY_SEEN:-}" ] && case "$BUSY_SEEN" in
  Working*) echo "  while the composer said Stop, the pill said: $BUSY_SEEN  (they agree)";;
  *)        echo "  while the composer said Stop, the pill said: $BUSY_SEEN  — the pill counts"
            echo "                           running workers, so a root turn alone leaves its tick showing";;
esac
echo "  the transcript grows in: ${STEPS:-0} step(s)   ${SPAN:-—} characters on screen"

# 2b. streaming — the row's third word, and this file has taken a still
# called `02-streaming` for eighty cycles without ever measuring it. A reply
# that arrives in one lump and one that grows word by word both pass every
# timing above; only the caller can see the difference, and they see it for
# the whole length of the answer.
#
# The measure is how many *different* lengths the reply is caught at. One
# means it appeared whole; several mean it grew.

# 3. the Worked line, which is the turn ending
WORKED=""
for _ in $(seq 1 120); do
  W=$(ui dump | grep -oE "Worked [0-9]+[sm][^;]*" | tail -1)
  [ -n "$W" ] && { WORKED=$(since "$T0"); break; }
  sleep 0.5
done
echo "  the Worked line lands:   ${WORKED:-never}s   reading '$(ui dump | grep -oE "Worked [0-9]+[sm]" | tail -1)'"
if [ "${STEPS:-0}" -le 1 ]; then
  echo "  NOTE: the reply was only ever caught at one length, so this run cannot"
  echo "        tell streaming from a reply that arrived whole — a short answer"
  echo "        can finish between two samples"
fi
shot 03-worked

AFTER=$(ui dump | awk '$3 == "TextField" { $1="";$2="";$3=""; sub(/^ +/, ""); print }' | head -1)
echo "  the composer afterwards: ${AFTER:-nothing}"
# Printed since this file was written, and claimed in the verdict, and never
# compared. A send that left the typed line sitting in the box would have
# read "and the composer cleared" all the same. It is cleared when it holds
# a placeholder rather than the words that were sent.
cleared_verdict() { # <what the composer holds> <the mark that was sent>
  case "$1" in
    *"$2"*) echo no;;
    ""|*"Follow up"*|*"Plan, ask, build"*|*"Answer"*|*"Message "*) echo yes;;
    *) echo no;;
  esac
}
# Prove the judgement can say no before trusting it to say yes. A branch
# that has never fired is a branch nobody has read.
if [ "$(cleared_verdict "Follow up…" "$MARK")" != yes ] \
   || [ "$(cleared_verdict "$MARK: reply with three short sentences" "$MARK")" != no ]; then
  echo "  the cleared-composer test cannot tell the two apart — not judging it"
  CLEARED=unknown
else
  CLEARED=$(cleared_verdict "$AFTER" "$MARK")
fi
echo "  and the kernel's record:"
python3 "$HERE/../kernel.py" pod history 4 2>/dev/null | tail -3 | cut -c1-110 | sed 's/^/    /'
echo "stills in $OUT"

# The path in one line per claim: the card, the reply, the Worked line.
echo
MISSING=""
[ -z "${CARD:-}" ]   && MISSING="$MISSING card"
[ -z "${FIRST:-}" ]  && MISSING="$MISSING reply"
[ -z "${WORKED:-}" ] && MISSING="$MISSING Worked-line"
if [ -n "$MISSING" ]; then
  echo "VERDICT: the turn never produced:$MISSING"
elif [ "$CLEARED" = unknown ]; then
  echo "VERDICT: send → card ${CARD}s → reply ${FIRST}s → Worked ${WORKED}s; the"
  echo "         composer was not judged — see the line above"
elif [ "$CLEARED" != yes ]; then
  echo "VERDICT: send → card ${CARD}s → reply ${FIRST}s → Worked ${WORKED}s, but the"
  echo "         composer still holds '$AFTER' — the line was sent and not cleared"
else
  echo "VERDICT: send → card ${CARD}s → reply ${FIRST}s → Worked ${WORKED}s, and the composer cleared"
fi

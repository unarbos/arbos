#!/bin/bash
# Speaking over Arbos while it talks: does it stop, and how fast?
#
#   barge-in.sh <cycle> [runs]
#
# The row was measured at cycle 77 and never committed, so it has been a
# memory for 47 cycles — the exact trap M-270 named when the mic rig had to
# be rebuilt from scratch for the same reason.
#
# The app already reports both halves, so nothing here times anything by
# hand:
#
#   barge_in_speech_started  — from the barge clip starting to the app
#                              stopping its own playback and telling the
#                              gateway to interrupt. This is what a person
#                              feels: how long Arbos keeps talking over them.
#   barge_in_response_done   — to the gateway confirming `interrupted`.
#
# Both are emitted from `CallViewModel`, so a run that prints neither has
# not barged in — it has failed to, and says so rather than passing quietly.
#
# `-injectWav` is the opening question and `-bargeWav` is fired while the
# reply plays. The opening clip must provoke a reply long enough to be
# interrupted; `small.wav` asks for three sentences, which is plenty.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; RUNS=${2:-3}
OUT="$HOME/mobile-out/$CYCLE/barge-in"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ASK=${ASK:-$HOME/mobile-clips/small.wav}
BARGE=${BARGE:-$HOME/mobile-clips/barge.wav}
for f in "$ASK" "$BARGE"; do
  [ -f "$f" ] || { echo "no clip at $f"; exit 1; }
done
echo "asking with $(basename "$ASK"), barging with $(basename "$BARGE")"
echo

STARTED=0; DONE=0; NEITHER=0
for i in $(seq 1 "$RUNS"); do
  LOG="$OUT/run-$i.log"
  xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
  xcrun simctl launch --console-pty "$UDID" $B -noAskNotifications 1 -previewCall 1 \
    -injectWav "$ASK" -bargeWav "$BARGE" > "$LOG" 2>&1 &
  LAUNCH=$!
  sleep 45
  kill $LAUNCH 2>/dev/null
  xcrun simctl terminate "$UDID" $B 2>/dev/null

  S=$(grep -oE "metric barge_in_speech_started [0-9]+" "$LOG" | tail -1 | awk '{print $3}')
  D=$(grep -oE "metric barge_in_response_done [0-9]+" "$LOG" | tail -1 | awk '{print $3}')
  if [ -n "$S" ]; then
    STARTED=$((STARTED + 1))
    echo "run $i: it stopped talking after ${S} ms${D:+, the gateway confirmed at ${D} ms}"
    [ -n "$D" ] && DONE=$((DONE + 1))
  else
    NEITHER=$((NEITHER + 1))
    # Say which of the two failures it was. A reply that never played cannot
    # be interrupted, and calling that "barge-in is broken" is a fault filed
    # against the wrong thing.
    if grep -q "response.done .*playing=true" "$LOG"; then
      echo "run $i: a reply played and nothing interrupted it — barge-in did not fire"
    else
      echo "run $i: no reply played, so there was nothing to barge into — this run says nothing"
    fi
  fi
done

echo
echo "runs that stopped the reply:      $STARTED of $RUNS"
echo "runs the gateway confirmed:       $DONE of $RUNS"
if [ "$STARTED" = "$RUNS" ]; then
  MS=$(grep -hoE "metric barge_in_speech_started [0-9]+" "$OUT"/run-*.log | awk '{s+=$3; n++} END {if (n) printf "%d", s/n}')
  echo "VERDICT: speaking over it stops it, every run — ${MS} ms on average"
  echo "         (cycle 77 measured 181 ms)"
elif [ "$STARTED" = 0 ] && [ "$NEITHER" = "$RUNS" ]; then
  echo "VERDICT: none — no run got a reply to interrupt, so this says nothing"
  echo "         about barge-in either way"
else
  echo "VERDICT: it stopped the reply in $STARTED of $RUNS — not every time, which is"
  echo "         worse than never, because the caller cannot learn what to expect"
fi
echo "logs in $OUT"

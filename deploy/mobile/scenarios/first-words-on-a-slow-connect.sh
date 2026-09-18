#!/bin/bash
# Does a slow connect still cost the caller their opening words?
#
#   first-words-on-a-slow-connect.sh <cycle> [runs]
#
# The history this settles. At cycle 66 the capture path was driven with a
# clip and counted at both ends: a **2633 ms** connect lost the first 13
# frames, while connects of 1514 ms and 1936 ms lost none. The cause was the
# pre-socket hold, two seconds of audio, oldest dropped first (M-246). Cycle
# 67 widened it to six (M-248) and four runs after the change lost nothing —
# but none of those four connected slowly enough to be the case in question,
# so the report said plainly that the failing reading had not been seen to
# turn green.
#
# This is the missing measurement, and it needs no new instrumentation: the
# app already prints `metric connect <ms>` and `metric mic_frames clip=<n>
# sent=<n>`. What it needed was to be run often enough to catch a slow
# connect, which is why this takes a run count.
#
# Cycles 66 and 67 did all of this with ad-hoc commands that were never
# committed, which is M-133 over again — the measurement existed for one
# evening and then only in a report.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; RUNS=${2:-6}
OUT="$HOME/mobile-out/$CYCLE/first-words"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
CLIP=${CLIP:-$HOME/mobile-clips/acceptance.wav}
[ -f "$CLIP" ] || { echo "no clip at $CLIP"; exit 1; }

echo "run  connect     clip    sent    lost"
SLOW_AND_CLEAN=0; SLOW=0
for i in $(seq 1 "$RUNS"); do
  LOG="$OUT/run-$i.log"
  xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
  # `-previewCall 1` opens straight into the call; `-micWav` plays the clip
  # down the capture path from the first moment, which is the only way to
  # exercise audio spoken before the socket exists.
  xcrun simctl launch --console-pty "$UDID" $B -noAskNotifications 1 -previewCall 1 -micWav "$CLIP" > "$LOG" 2>&1 &
  LAUNCH=$!
  sleep 30
  kill $LAUNCH 2>/dev/null
  xcrun simctl terminate "$UDID" $B 2>/dev/null

  MS=$(grep -oE "metric connect [0-9]+ms" "$LOG" | tail -1 | grep -oE "[0-9]+" | head -1)
  LAST=$(grep -oE "metric mic_frames clip=[0-9]+ sent=[0-9]+" "$LOG" | tail -1)
  CLIPN=$(echo "$LAST" | grep -oE "clip=[0-9]+" | cut -d= -f2)
  SENTN=$(echo "$LAST" | grep -oE "sent=[0-9]+" | cut -d= -f2)
  if [ -z "$MS" ] || [ -z "$CLIPN" ]; then
    printf "%-4s %-11s %s\n" "$i" "${MS:-—}" "no counters — the call did not run this time"
    continue
  fi
  LOST=$((CLIPN - SENTN))
  printf "%-4s %-11s %-7s %-7s %s\n" "$i" "${MS}ms" "$CLIPN" "$SENTN" "$LOST"
  # Two seconds was the old window; anything above it is a run that would
  # have lost frames before #557, and so is the case worth seeing pass.
  if [ "$MS" -gt 2000 ]; then
    SLOW=$((SLOW + 1))
    [ "$LOST" -eq 0 ] && SLOW_AND_CLEAN=$((SLOW_AND_CLEAN + 1))
  fi
done

echo
echo "runs above the old two-second window: $SLOW, of which lost nothing: $SLOW_AND_CLEAN"
if [ "$SLOW" -eq 0 ]; then
  echo "VERDICT: no slow connect occurred, so this run says nothing about the case in question."
  echo "         Run it again — connect time is not something the rig controls."
elif [ "$SLOW_AND_CLEAN" -eq "$SLOW" ]; then
  echo "VERDICT: every slow connect kept all its frames — the case M-246 caught is seen passing."
else
  echo "VERDICT: a slow connect still lost frames. The hold is not covering it."
fi
echo "logs in $OUT"

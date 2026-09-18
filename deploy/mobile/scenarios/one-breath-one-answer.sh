#!/bin/bash
# A question with a breath in the middle: one transcript, or two?
#
#   one-breath-one-answer.sh <cycle> [runs]
#
# M-146, open since cycle 43 and reproduced cleanly at 68 (M-251): the
# standard ask, cut in half by a silence, came back as **two** transcripts
# and **two** spoken answers, both `response.done reason=completed` with
# audio played. The caller hears two replies to one question, the second
# arriving over the first.
#
# The phone was ruled out at 68 by counting: 900 of 900 frames captured and
# sent, no loss at the socket, so the stream the gateway received was
# continuous and the split is its own segmentation of it. Filed at
# `internal/features-inbox/2026-09-18-one-question-two-answers-across-a-pause.md`.
#
# This is the re-check. It exists as a file because cycle 68 did it with
# ad-hoc commands and cycle 72 had to rebuild the mic rig from scratch for
# the same reason (M-270): a measurement that is not committed is a memory.
#
# What it counts, per run: final transcripts, and `response.done` frames
# that actually played audio. One of each is the pass.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; RUNS=${2:-3}
OUT="$HOME/mobile-out/$CYCLE/one-breath"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
CLIP=${CLIP:-$HOME/mobile-clips/pause.wav}
[ -f "$CLIP" ] || { echo "no clip at $CLIP"; exit 1; }
# How many separate things the clip actually asks. `pause.wav` is one
# question with a breath in it; `acceptance.wav` is two utterances, and
# against that a count of two transcripts is right rather than a split.
# Without this the scenario read a correct two-utterance run as "the
# transcript split, 3 of 3".
SAYS=${SAYS:-1}
echo "clip: $(basename "$CLIP") — expecting $SAYS utterance(s) and $SAYS answer(s) per run"

SPLIT=0; WHOLE=0; NOTKERNEL=0
# What the kernel itself recorded before any of this. Counting the gateway's
# frames says how many answers were *spoken*; only the kernel's own record
# says whether the kernel is the one that answered. With the gateway serving
# its own model those numbers come apart — the caller hears a reply and the
# project never knows a question was asked.
TARGET=${TARGET:-pod}
ktotal() { python3 "$HERE/../kernel.py" "$TARGET" history 1 2>/dev/null | grep -oE "^ *[0-9]+" | tr -d ' '; }

for i in $(seq 1 "$RUNS"); do
  LOG="$OUT/run-$i.log"
  BEFORE=$(ktotal)
  xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
  xcrun simctl launch --console-pty "$UDID" $B -noAskNotifications 1 -previewCall 1 -micWav "$CLIP" > "$LOG" 2>&1 &
  LAUNCH=$!
  sleep 40
  kill $LAUNCH 2>/dev/null
  xcrun simctl terminate "$UDID" $B 2>/dev/null

  TRANSCRIPTS=$(grep -c "^transcript:" "$LOG" | tr -d ' ')
  ANSWERS=$(grep -c "response.done .*playing=true" "$LOG" | tr -d ' ')
  FRAMES=$(grep -oE "metric mic_frames clip=[0-9]+ sent=[0-9]+" "$LOG" | tail -1)
  echo "run $i: $TRANSCRIPTS transcript(s), $ANSWERS answer(s) with audio   ${FRAMES:-no frame counts}"
  grep "^transcript:" "$LOG" | sed 's/^/     /'
  # The phone's half, every run: if frames were lost at the socket then the
  # gateway heard a broken stream and the split is not its fault.
  if [ -n "$FRAMES" ]; then
    C=$(echo "$FRAMES" | grep -oE "clip=[0-9]+" | cut -d= -f2)
    S=$(echo "$FRAMES" | grep -oE "sent=[0-9]+" | cut -d= -f2)
    [ "$C" = "$S" ] || echo "     NOTE: $((C - S)) frames lost at the socket — this run says nothing about the gateway"
  fi
  # Did the kernel answer, or did the gateway answer for it? Read the lines
  # the kernel added during this run: one `user`, one `assistant`, one
  # `turn_complete` is the shape of a single question answered by the
  # project. Nothing new at all means the caller heard a reply the project
  # never knew about.
  AFTER=$(ktotal)
  if [ -n "$BEFORE" ] && [ -n "$AFTER" ] && [ "$AFTER" -gt "$BEFORE" ]; then
    NEW=$(python3 "$HERE/../kernel.py" "$TARGET" history $(( (AFTER - BEFORE) + 2 )) 2>/dev/null \
      | awk -v b="$BEFORE" '$1+0 > b')
    KU=$(echo "$NEW" | grep -c " user  " | tr -d ' ')
    KA=$(echo "$NEW" | grep -c " assistant  " | tr -d ' ')
    echo "     kernel recorded: $KU question(s), $KA answer(s)"
    echo "$NEW" | grep " assistant  " | cut -c1-110 | sed 's/^/       /'
    { [ "$KU" = "$SAYS" ] && [ "$KA" = "$SAYS" ]; } || NOTKERNEL=$((NOTKERNEL + 1))
  else
    echo "     kernel recorded: nothing — the caller heard a reply the project never saw"
    NOTKERNEL=$((NOTKERNEL + 1))
  fi

  # Two separate faults, and after #562 they no longer travel together: the
  # transcript can be whole while the question is still answered twice.
  [ "$TRANSCRIPTS" -gt "$SAYS" ] && SPLIT=$((SPLIT + 1))
  [ "$ANSWERS" -gt "$SAYS" ] && WHOLE=$((WHOLE + 1))
done

echo
echo "runs with more than $SAYS transcript(s): $SPLIT of $RUNS"
echo "runs with more than $SAYS answer(s):    $WHOLE of $RUNS"
echo "runs the kernel did not answer:      $NOTKERNEL of $RUNS"
if [ "$SPLIT" = 0 ] && [ "$WHOLE" = 0 ] && [ "$NOTKERNEL" = 0 ]; then
  echo "VERDICT: one breath, one transcript, one kernel answer. M-146 does not reproduce."
elif [ "$SPLIT" = 0 ] && [ "$WHOLE" = 0 ]; then
  echo "VERDICT: one transcript and one spoken answer, but the kernel did not answer"
  echo "         $NOTKERNEL of $RUNS run(s) — the caller heard a reply the project never saw."
elif [ "$SPLIT" = 0 ]; then
  echo "VERDICT: the transcript is whole now — the breath no longer splits it — but the"
  echo "         question is still answered more than once. Half of M-146."
elif [ "$WHOLE" = 0 ]; then
  echo "VERDICT: the transcript still splits, though each half is answered once."
else
  echo "VERDICT: both still happen."
fi
echo "logs in $OUT"

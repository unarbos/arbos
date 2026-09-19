#!/bin/bash
# A recording shrunk for review, without losing what review is for.
#
#   review-demo.sh <recording.mp4> [out.mp4]
#
# Raw simulator footage is 30–40 MB a minute and too large to hand to a
# reviewer. Every cycle that films something has shrunk it by hand with an
# ffmpeg line typed from memory, and cycle 174 typed 400 px.
#
# At 400 px the separator dot in a list row — one pixel of ink at that scale —
# disappears completely. The reviewer read `Idle  home`, called the gap a
# missing character, and I spent the first part of a cycle checking a fault
# that was an artefact of my own downscaling (M-539).
#
# 540 px keeps it. That is the width this uses, and the reason it is written
# down here rather than remembered.
#
# What a film is good for is motion: clipping, stalls, jumps, a spinner that
# never stops, a transition that flickers. Fine typography belongs to a
# still, at full size. Ask a reviewer for the first and not the second.
set -uo pipefail
# ffmpeg lives in Homebrew and a non-login ssh shell does not have that
# directory on PATH — which is the only way this loop ever runs. Without this
# line the tool built to make recordings reviewable answers "no ffmpeg on this
# machine" on a machine that has it. Cycle 160 learned this about Python and
# the lesson did not travel; check-tools.sh now enforces it.
export PATH="/opt/homebrew/bin:$PATH"
IN=${1:?a recording to shrink}
OUT=${2:-${IN%.mp4}-demo.mp4}
WIDTH=${REVIEW_WIDTH:-540}

[ -f "$IN" ] || { echo "no recording at $IN"; exit 1; }
command -v ffmpeg >/dev/null || { echo "no ffmpeg on this machine"; exit 1; }

ffmpeg -loglevel error -i "$IN" -vf "scale=$WIDTH:-2,fps=10" -an "$OUT" -y || {
  echo "ffmpeg could not read $IN"; exit 1; }

# wc -c, not stat. `stat -f` is "format" on BSD and "file system" on GNU, so
# the BSD form succeeds on Linux and prints block counts — the fallback never
# runs, and the line reads as though the file were sixty million bytes.
before=$(wc -c < "$IN" | tr -d ' ')
after=$(wc -c < "$OUT" | tr -d ' ')
printf '%s\n' "  $IN  $before bytes"
printf '%s\n' "  $OUT  $after bytes, ${WIDTH}px wide"
echo
echo "  Ask the reviewer about motion: clipping, stalls, jumps, a spinner that"
echo "  never stops. Not about fine typography — at this width a separator dot"
echo "  is already gone, and cycle 174 lost half a cycle to exactly that."

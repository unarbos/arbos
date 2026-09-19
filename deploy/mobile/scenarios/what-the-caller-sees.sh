#!/bin/bash
# COVERS: call — first word and transcription
# COVERS: call — voice first, orb, colours
#
# What is on the screen while the answer is being waited for?
#
#   what-the-caller-sees.sh <cycle>
#
# `mac-voice.sh` measures this turn in milliseconds and reads none of it off
# the screen: it parses the console and takes seven stills nobody opens. So
# the numbers are known — cycle 193 measured the kernel answering in 8386 ms —
# and what a person looks at for those eight seconds has never been recorded.
#
# The call screen is wordless on purpose. The orb's colour carries the state
# and one line of text carries the last thing said. This samples both, as
# fast as the accessibility tree can be read, from before the clip plays
# until after the reply lands, and prints the order things happened in.
set -uo pipefail
export PATH="/opt/homebrew/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/caller"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
CLIPS="$HOME/mobile-clips"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

[ -f "$CLIPS/ask.wav" ] || { echo "no ask.wav in $CLIPS — run mac-voice.sh once to make the clips"; exit 1; }

xcrun simctl terminate "$UDID" $B 2>/dev/null
sleep 1
xcrun simctl launch --console-pty "$UDID" $B -previewCall 1 -injectWav "$CLIPS/ask.wav" \
  > "$OUT/console.log" 2>&1 &
START=$(python3 -c 'import time;print(time.time())')

# One dump per sample, and nothing else: a screenshot here would cost a
# second each time and blur the order of what is being timed.
SAMPLES="$OUT/timeline.txt"; : > "$SAMPLES"
for _ in $(seq 1 60); do
  NOW=$(python3 -c "import time;print(f'{time.time()-$START:.1f}')")
  D=$(ui dump 2>/dev/null)
  PHASE=$(echo "$D" | grep -oE 'Call \| [A-Za-z ]+' | head -1 | sed 's/Call | //')
  [ -z "$PHASE" ] && PHASE=$(echo "$D" | grep -A1 "Call" | grep -oE "Listening|Speaking|Thinking|Connecting|Idle" | head -1)
  LINE=$(echo "$D" | grep -E "StaticText" | awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }' \
    | grep -vE "^(Call|Mute|End call|Call menu|Idle|Listening|Thinking|Speaking|)$" | head -1)
  echo "$NOW|${PHASE:-?}|${LINE:-}" >> "$SAMPLES"
  awk -F'|' -v n="$NOW" 'END{}' /dev/null
done

xcrun simctl io "$UDID" screenshot "$OUT/at-the-end.png" >/dev/null 2>&1

echo "the call, as the screen told it:"
echo
python3 - "$SAMPLES" <<'PY'
import sys
rows = [l.rstrip("\n").split("|", 2) for l in open(sys.argv[1]) if l.strip()]
rows = [(float(t), p, x) for t, p, x in rows]
if not rows:
    print("  nothing sampled")
    raise SystemExit
# Only the changes: sixty samples of the same screen say nothing.
last = None
for t, phase, text in rows:
    now = (phase, text)
    if now == last:
        continue
    last = now
    shown = text if text else "— no words —"
    print(f"  {t:5.1f}s  {phase:<11} {shown[:64]}")
print()
span = rows[-1][0]
silent = [t for t, p, x in rows if not x]
phases = []
for _, p, _ in rows:
    if not phases or phases[-1] != p:
        phases.append(p)
print(f"  sampled {len(rows)} times over {span:.0f}s")
print(f"  phases in order: {' → '.join(phases)}")
if silent:
    print(f"  wordless for {len(silent)} of {len(rows)} samples,"
          f" first at {silent[0]:.1f}s, last at {silent[-1]:.1f}s")
PY

echo
echo "console said:"
grep -E "metric (connect|reply_first_audio)|^transcript:" "$OUT/console.log" | head -6 | sed 's/^/  /'
echo
echo "still and timeline in $OUT"
xcrun simctl terminate "$UDID" $B 2>/dev/null

#!/bin/bash
# COVERS: call — the microphone path
# Voice measurements on the simulator (runs on the Mac). Two clips made
# with `say` stand in for the mic; the app's DEBUG metrics print to the
# console: reply_first_audio (speech end → first reply audio),
# barge_in_speech_started / barge_in_response_done (barge → playback cut).
#   mac-voice.sh <cycle>
set -uo pipefail
export PATH="/opt/homebrew/bin:$PATH"
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE"; mkdir -p "$OUT/voice"
CLIPS="$HOME/mobile-clips"; mkdir -p "$CLIPS"
mk() { [ -f "$CLIPS/$1.wav" ] || { say -v Samantha -o "$CLIPS/$1.aiff" "$2" && ffmpeg -loglevel error -y -i "$CLIPS/$1.aiff" -ar 24000 -ac 1 -sample_fmt s16 "$CLIPS/$1.wav"; }; }
# This clip asks a question the *kernel* must answer, so the reply_first_audio
# it produces is not comparable with a greeting's. Cycle 170 measured four
# runs at 11.0, 11.1, 12.4 and 13.1 s and nearly filed a tenfold regression
# against cycle 167's 1249 ms — until one run of call-text-in-chat printed
# both numbers, 1244 ms for its small talk and 12223 ms for its delegated
# question, on the same build. The gateway answers a greeting itself; a
# question about the project waits for the kernel to think.
mk ask "Hello Arbos, what are we working on right now? Give me one sentence."
mk barge "Stop, wait, one more thing."
UDID=$(cut -d' ' -f2 "$OUT/../$CYCLE/sim.txt" 2>/dev/null || xcrun simctl list devices booted -j | python3 -c 'import json,sys; d=json.load(sys.stdin)["devices"]; print(next(x["udid"] for v in d.values() for x in v))')
BUNDLE=com.unarbos.arbos.ios
xcrun simctl terminate "$UDID" $BUNDLE 2>/dev/null || true
xcrun simctl launch --console-pty "$UDID" $BUNDLE -previewCall 1 -injectWav "$CLIPS/ask.wav" -bargeWav "$CLIPS/barge.wav" > "$OUT/voice/console.log" 2>&1 &
PID=$!
sleep 1
xcrun simctl io "$UDID" recordVideo --codec h264 --force "$OUT/voice/call.mp4" >/dev/null 2>&1 &
RECPID=$!
prev=1
for t in 3 5 7 9 11 14 20; do sleep $(( t - prev )); prev=$t; xcrun simctl io "$UDID" screenshot "$OUT/voice/call-t$t.png" >/dev/null 2>&1; done
sleep 8
kill -INT $RECPID 2>/dev/null; sleep 3
kill $PID 2>/dev/null
ffmpeg -v error -y -i "$OUT/voice/call.mp4" -vf "scale=786:-2,fps=30" -c:v libx264 -crf 24 -preset veryfast -pix_fmt yuv420p -an "$OUT/voice/call-live.mp4"
grep -E "^metric|phase|transcript:|reply:" "$OUT/voice/console.log" | head -60 | tee "$OUT/voice/metrics.txt"

echo
# One run produces two of these, and the note used to sit under the wrong
# one. The clip asks a question the kernel must answer; the barge that
# follows is small talk the gateway answers itself. Cycle 184 measured
# 6422 ms and 1240 ms in the same run, and a reader seeing only the second
# with a note about the first would call the next delegated reading a
# tenfold regression — which is the confusion cycle 170 wrote the note to
# prevent.
#
# So pair each number with the question it followed.
echo "what each reply timing answered:"
python3 - "$OUT/voice/console.log" <<'PY'
import re, sys
said = None
for line in open(sys.argv[1], errors="ignore"):
    if line.startswith("transcript:"):
        said = line.split(":", 1)[1].strip()
    m = re.match(r"metric reply_first_audio (\d+)ms", line)
    if m and said:
        kind = "the kernel had to answer" if len(said) > 40 else "the gateway answered it itself"
        print(f"  {m.group(1):>6} ms   after \"{said[:52]}\"")
        print(f"            {kind}")
        said = None
PY
echo
echo "The two are never compared. A greeting answered by the gateway and a"
echo "question waiting on the kernel are different measurements wearing one"
echo "metric name."

# Whether barge-in happened at all. The clip plays at a fixed offset, so a
# short reply is over before the interruption arrives and the run measures
# nothing — it prints `barge_in_skipped` among a hundred console lines and
# the summary says only what the reply timings mean. Cycle 195 read a run as
# healthy that had not exercised barge-in once, on a row the ledger then
# aged as covered.
echo
BARGE=$(grep -oE "metric barge_in_(speech_started|response_done) [0-9]+ms" "$OUT/voice/console.log" | head -2 | tr '\n' ' ')
if [ -n "$BARGE" ]; then
  echo "barge-in was exercised: $BARGE"
else
  WHY=$(grep -oE "metric barge_in_[a-z_]+ .*" "$OUT/voice/console.log" | head -1)
  echo "barge-in was NOT exercised this run — ${WHY:-no barge metric at all}."
  echo "Nothing here says anything about interrupting a reply. The clip plays"
  echo "at a fixed offset, so a reply shorter than that offset is over before"
  echo "the interruption arrives."
fi

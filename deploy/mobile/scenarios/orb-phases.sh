#!/bin/bash
# What the orb goes through during a call, in order and with timings.
#
#   orb-phases.sh <cycle> [clip]
#
# This row has never had a check of its own — only timings borrowed from
# journey runs — because until #590 the orb's state existed *only* as a
# colour. Nothing in the accessibility tree said whether the app was
# listening, thinking or speaking, so there was nothing to read.
#
# #590 gave the orb a value carrying `Phase.label`, for VoiceOver's sake.
# The side effect is that the state is now machine-readable, and the
# sequence a caller actually experiences can be recorded rather than
# inferred from a screenshot of a coloured circle.
#
# Expected, roughly: idle → connecting → listening → thinking → speaking →
# listening. What matters is that it passes through them in that order and
# does not flap — cycle 43's fault (M-145) was the orb dropping back to
# listening mid-turn and jumping forward again.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}
CLIP=${2:-$HOME/mobile-clips/pause.wav}
# How many things the clip asks. Each turn legitimately walks thinking →
# speaking → listening, so a two-utterance clip revisits all three and is
# not flapping. Run against acceptance.wav (two utterances) the first
# version called a perfectly ordinary two-turn call a fault — the same
# "one question baked in" error as M-301, for the third time.
SAYS=${SAYS:-1}
OUT="$HOME/mobile-out/$CYCLE/orb-phases"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
[ -f "$CLIP" ] || { echo "no clip at $CLIP"; exit 1; }

phase() {
  idb ui describe-all --udid "$UDID" 2>/dev/null | python3 -c '
import json, sys
try:
    els = json.load(sys.stdin)
except Exception:
    sys.exit()
for e in els:
    if (e.get("AXLabel") or "") == "Call":
        print((e.get("AXValue") or "").strip())
        break
'
}

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 -previewCall 1 -micWav "$CLIP" >"$OUT/console.log" 2>&1 &
LAUNCH=$!

echo "time  phase"
LAST=""; START=$(python3 -c 'import time;print(time.time())')
SEQ=""
for _ in $(seq 1 120); do
  P=$(phase)
  if [ -n "$P" ] && [ "$P" != "$LAST" ]; then
    T=$(python3 -c "import time;print(f'{time.time() - $START:5.1f}')")
    printf "%s  %s\n" "$T" "$P"
    SEQ="$SEQ $P"
    LAST=$P
  fi
  sleep 0.4
done
kill $LAUNCH 2>/dev/null
xcrun simctl terminate "$UDID" $B 2>/dev/null

echo
echo "the sequence:$SEQ"
echo
# Flapping is the fault worth catching: a phase that is entered, left and
# entered again inside one turn is what M-145 described, and what a caller
# sees as the orb twitching.
python3 - "$SEQ" "$SAYS" <<'PY'
import sys
seq = sys.argv[1].split()
if not seq:
    print("VERDICT: the orb never reported a phase — nothing to read")
    raise SystemExit
says = int(sys.argv[2])
runs = []
for p in seq:
    if not runs or runs[-1] != p:
        runs.append(p)
spoke = runs.count("speaking")
print(f"phases seen: {len(set(runs))} distinct, {len(runs)} changes; "
      f"spoke {spoke} time(s) for {says} question(s)")
if "speaking" not in runs:
    print("VERDICT: it never reached speaking — the call did not get an answer out")
elif spoke > says:
    print(f"VERDICT: {spoke} spoken answers to {says} question(s) — either a reply in "
          f"parts or the double answer of M-146")
else:
    print("VERDICT: one pass through the phases per question, in order, no flapping")
PY
echo "console in $OUT"

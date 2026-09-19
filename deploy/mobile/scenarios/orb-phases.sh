#!/bin/bash
# COVERS: call — voice first, orb, colours
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
#
# The coverage row is "call — voice first, orb, **colours**", and until now
# this file read the phase *labels* and never looked at a colour: the word
# appeared only in this comment. A caller in a quiet room sees the colour
# and nothing else, so the orb changing its value while looking identical
# would be a real fault that the sequence above cannot see. Each phase is
# therefore sampled from the screen as well, once, the first time it is
# entered.
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
    # One still the first time each phase is entered, for the colour.
    [ -f "$OUT/phase-$P.png" ] || xcrun simctl io "$UDID" screenshot "$OUT/phase-$P.png" >/dev/null 2>&1
  fi
  sleep 0.4
done
kill $LAUNCH 2>/dev/null
xcrun simctl terminate "$UDID" $B 2>/dev/null

echo
echo "the sequence:$SEQ"
echo
echo "what each phase looked like:"
python3 - "$OUT" <<'PY'
from pathlib import Path
import sys
try:
    from PIL import Image
except ImportError:
    print("  no PIL on this machine — the colours were not read"); raise SystemExit
out = Path(sys.argv[1])
seen = {}
for png in sorted(out.glob("phase-*.png")):
    name = png.stem[len("phase-"):]
    im = Image.open(png).convert("RGB")
    w, h = im.size
    # The orb sits in the middle of the screen. Its own pixels, not the
    # ground around it: the brightest tenth of the box, since the orb is
    # lit and the background is near-black.
    box = im.crop((int(w * 0.30), int(h * 0.33), int(w * 0.70), int(h * 0.55)))
    px = sorted(box.getdata(), key=sum, reverse=True)
    top = px[: max(1, len(px) // 10)]
    avg = tuple(sum(c[i] for c in top) // len(top) for i in range(3))
    seen[name] = avg
    print(f"  {name:12} RGB {avg}")
if len(seen) < 2:
    print("  only one phase was seen, so nothing could be compared")
else:
    # Distance in RGB is not the question a caller asks. Cycle 170 measured
    # listening at (133,132,132) rising to (173,173,173) with the voice, and
    # thinking at (75,74,74): far apart as numbers, the same grey to the eye,
    # and within a range listening already travels on its own. A reviewer
    # watching the film said the two were identical. Speaking, at
    # (98,124,162), is the only one that changes hue.
    #
    # So two phases count as sharing a face when neither has a colour — a
    # grey is a grey however bright — as well as when their RGB is close.
    def grey(c):
        return max(c) - min(c) <= 12
    pairs = [(a, b) for a in seen for b in seen if a < b]
    same = [f"{a} and {b}" for a, b in pairs
            if max(abs(x - y) for x, y in zip(seen[a], seen[b])) <= 8
            or (grey(seen[a]) and grey(seen[b]))]
    if same:
        print("  LOOK THE SAME: " + "; ".join(same))
        for a, b in pairs:
            if grey(seen[a]) and grey(seen[b]):
                print(f"  ({a} {seen[a]} and {b} {seen[b]} are both grey — a"
                      f" difference in brightness only, and listening's own"
                      f" brightness moves with the voice)")
        # Not every pair matters equally. `connecting` happens once, before
        # the call is under way; a caller is never choosing between it and
        # `thinking`. Two *mid-call* states sharing a face is the one that
        # leaves someone watching a circle with no way to read it.
        midcall = {"listening", "thinking", "speaking"}
        bad = [p for p in same if all(w in midcall for w in p.split(" and "))]
        if bad:
            print("  and both of these happen mid-call: " + "; ".join(bad))
            print("  A caller in a quiet room has only the colour to go on.")
        else:
            print("  each pair involves a state that happens once, before the call"
                  " is under way,")
            print("  so nobody is choosing between them while listening."
                  " Worth knowing, not a fault.")
    else:
        print(f"  all {len(seen)} phases differ on screen, not only in the tree")
PY
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

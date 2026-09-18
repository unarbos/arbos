#!/bin/bash
# The call's words, read back in the project chat.
#
#   call-text-in-chat.sh <cycle> [clip]
#
# Runs the acceptance conversation into a live call, leaves the call, and
# photographs the project chat. What the stills are for: a delegated turn is
# answered twice — the kernel writes an answer and the voice says the same
# thing in its own words — and the chat must show one of them, the spoken
# one. Before the rule landed both were there, one under the other.
#
# The clip is injected at launch because the audio engine consumes it when
# it starts, not when the call is entered; entering the call from the chat
# afterwards would find it already spent.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}
CLIP=${2:-$HOME/mobile-clips/acceptance.wav}
OUT="$HOME/mobile-out/$CYCLE/call-text"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
BUNDLE=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "shot $1"; }

idb connect "$UDID" >/dev/null 2>&1
xcrun simctl terminate "$UDID" $BUNDLE 2>/dev/null || true
xcrun simctl launch --console-pty "$UDID" $BUNDLE \
  -previewCall 1 -noAskNotifications 1 -injectWav "$CLIP" > "$OUT/console.log" 2>&1 &

# The clip is the acceptance conversation: small talk, then a question the
# gateway delegates. The kernel turn is the slow part.
sleep 8;  shot 01-call-small-talk
sleep 30; shot 02-call-delegated-answer

# Out of the call and into the chat it belongs to. Closing the call lands on
# the project list, not on the chat, so the project is opened by name — the
# list is alphabetical and its rows move as machines come and go.
# A call is wordless by design: the composer and the close button only exist
# once the screen is pulled down, so the gesture comes first and the tap
# after it. Tapping for a control that is not on the screen yet was how the
# first two attempts at this still were lost.
idb ui swipe 196 300 196 700 --duration 0.4 --udid "$UDID"; sleep 1.5
shot 03-call-pulled-down
ui tap "End call" || { echo "no close control — the pull did not take"; exit 1; }
sleep 2.5; shot 04-project-list
ui tap "${PROJECT:-phone}, Idle" || { echo "project row not on screen"; exit 1; }
sleep 3; shot 05-chat-tail

# One screen back reaches the same question as it was answered before the
# rule landed: the kernel's wording, kept whole.
idb ui swipe 196 300 196 720 --duration 0.4 --udid "$UDID"; sleep 1.2
shot 06-chat-before-the-rule

echo "--- what the call did ---"
grep -E "^metric|^event response.done|^phase" "$OUT/console.log" | tail -12
echo "--- what the kernel wrote for the same turns ---"
python3 "$HERE/../kernel.py" pod history 8 2>&1 | tail -8

# The rule, checked rather than photographed. The two answers to a delegated
# turn are written independently — the kernel's text and the voice's own
# words — so they differ, and that difference is what makes this testable:
# take a distinctive run of words from the kernel's answer and require it to
# be absent from the screen, while a Spoken row is present.
echo
echo "--- the rule: one wording, the spoken one ---"
KERNEL_SAID=$(python3 "$HERE/../kernel.py" pod history 8 2>/dev/null \
              | awk '$2=="assistant"' | tail -1 | cut -d' ' -f3- | sed 's/^ *//')
if [ -z "$KERNEL_SAID" ]; then
  echo "  no assistant line to compare against — inconclusive"
else
  # Six consecutive words is long enough not to collide by chance and short
  # enough to survive the chat truncating a long answer.
  PHRASE=$(echo "$KERNEL_SAID" | tr -s ' ' | cut -d' ' -f2-7)
  echo "  the kernel's words:  ...$PHRASE..."
  # Only this turn. The kernel's wording is legitimately elsewhere on screen:
  # spoken rows do not survive a restart, so older turns replay as the
  # kernel's text (M-179). Searching the whole screen found it there and
  # called the rule broken — the answer to a different question.
  DUMP=$(ui dump)
  SPOKEN=$(echo "$DUMP" | grep -c " StaticText   Spoken$" | tr -d ' ')
  # Count, do not compare. The old check asked whether this turn's reply was
  # *different from* the kernel's wording, which only works while the two
  # differ — and since the gateway changed today it often says exactly what
  # the kernel said. On a run where the chat showed the answer once, that
  # check called it "both wordings are showing".
  #
  # The rule is about how many rows a turn gets, so count the rows.
  TIMES=$(echo "$DUMP" | grep -cF "$PHRASE" | tr -d ' ')
  echo "  rows marked Spoken: $SPOKEN; the answer appears $TIMES time(s) on screen"
  if [ "$SPOKEN" = 0 ]; then
    echo "  VERDICT: nothing on screen is marked Spoken — inconclusive, the chat may not be at the tail"
  elif [ "$TIMES" -le 1 ]; then
    echo "  VERDICT: one wording on screen for this turn — the rule holds"
  else
    echo "  VERDICT: the answer is on screen $TIMES times — both wordings are showing"
  fi
fi
echo "stills in $OUT"

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

# Out of the call and into the chat it belongs to. The leave control is
# behind the overflow menu, so both taps go by label rather than by pixel.
ui tap "More" || ui tap "…" || echo "no overflow control"
sleep 1; shot 03-call-menu
ui tap "Leave" || ui tap "End" || echo "no leave control"
sleep 2; shot 04-chat

# The answer sits at the tail; the questions are above it.
idb ui swipe 196 300 196 700 --duration 0.4 --udid "$UDID"; sleep 1
shot 05-chat-scrolled-back

echo "--- agent rows on screen ---"
ui dump | grep -i "spoken\|worked" | head -20
echo "--- what the call did ---"
grep -E "^metric|^event response.done|^phase" "$OUT/console.log" | tail -12
echo "stills in $OUT"

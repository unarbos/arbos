#!/bin/bash
# A worker's chat: reached from the sheet, reached from its line, and left.
#
#   worker-chat-open-and-back.sh <cycle> [project]
#
# The row has been carried since cycle 32 on "the sheet lists workers and a
# worker's chat opens with its name in the header". Two ways in are claimed
# and only one has ever been driven: the sheet. A worker's `Done …` line in
# the transcript is meant to open the same chat, and that half has been
# taken on trust.
#
# Names are tapped as the app states them (`Back`, not a symbol's rendering)
# — M-314: a script tapping a name the app never says is coupled to a
# missing label and breaks the moment one is added.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/worker-chat"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }
# Which screen we are on, from the tree: the projects list, a project chat
# (composer + More), or a worker's chat (no composer — M-154's choice).
where() {
  local d; d=$(ui dump)
  if echo "$d" | grep -q "StaticText +Projects"; then echo list
  elif echo "$d" | grep -qE " TextField "; then echo "project-chat"
  elif echo "$d" | grep -qE "Button +Back"; then echo "worker-chat:$(echo "$d" | awk '$3=="StaticText" && $2<120 {print $4; exit}')"
  else echo unknown; fi
}

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 9
ui tap "$ROW" >/dev/null || { echo "no $ROW row"; exit 1; }
sleep 5
shot 01-the-project-chat

echo "== in by the sheet =="
# Idle the pill reads `Agents 19`; while anything runs it reads
# `⠇, Working 2` — the same animated spinner that prefixes the worker lines.
# A pattern anchored to the word alone finds it only half the time, and
# reported "no pill in this chat" on a chat that plainly had one.
PILL=$(ui dump | grep -E "Button +([⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏], )?(Agents|Working) [0-9]+" | head -1)
[ -n "$PILL" ] || { echo "  no pill in this chat"; exit 1; }
idb ui tap "$(echo "$PILL" | awk '{print $1}')" "$(echo "$PILL" | awk '{print $2}')" --udid "$UDID"
sleep 3
FIRST=$(ui dump | grep -E ", (Done|Working)$" | head -1 | awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); sub(/,.*/, ""); print }')
echo "  first row on the sheet: ${FIRST:-none}"
[ -n "$FIRST" ] || { echo "  the sheet did not open"; exit 1; }
ui tap "$FIRST" >/dev/null || { echo "  could not open that worker"; exit 1; }
sleep 4
shot 02-worker-chat-from-the-sheet
echo "  landed on: $(where)"
echo "  it holds $(ui dump | grep -c ' StaticText ') text rows, and a composer? $(ui dump | grep -cE ' TextField ')"

echo
echo "== back out =="
ui tap "Back" >/dev/null || echo "  no Back"
sleep 3
shot 03-back-from-the-worker
echo "  back landed on: $(where)"

echo
echo "== in by the worker's own line =="
# Only a *running* worker has a line that is a control: `workerLines` in
# MainChatView draws a Button per running worker and nothing for the rest.
# A finished worker leaves `<name> · Turn ended. Last words: …` in the
# transcript, which is kernel text and not a tap target — by design, not by
# omission. A first version of this tapped that text, stayed where it was,
# and was one line away from reporting an app fault.
# A single trivial request is not reliably delegated — cycles 90 and 91 both
# watched the root do the sleep itself. The plural form is what worked at
# cycle 88, so ask for two and take whichever line appears. Reusing a proven
# wording rather than inventing a third.
NAP=$(( 90 + RANDOM % 20 ))
TAG=r$(date -u +%H%M%S)
WLINE="Start two workers at once, each waiting for none of the other. Their goals are exactly $TAG one and $TAG two. Each runs the bash command sleep $NAP and nothing else, then replies done."
ui focus >/dev/null 2>&1
sleep 0.7
idb ui text "$WLINE" --udid "$UDID"
LANDED=no
for _ in $(seq 1 80); do
  [ "$(ui field plain 2>/dev/null)" = "$WLINE" ] && { LANDED=yes; break; }
  sleep 0.25
done
if [ "$LANDED" = no ]; then
  echo "  the line never landed in the box — not sending, and this half stays untested"
  echo "  the box holds: $(ui field plain 2>/dev/null | cut -c1-60)"
else
  ui tap "Send" >/dev/null 2>&1 || ui tap "Up" >/dev/null 2>&1
  echo "  asked for two workers sleeping ${NAP}s ($TAG); waiting for a running line"
fi
RUNNING=""
[ "$LANDED" = yes ] && for _ in $(seq 1 20); do
  sleep 4
  # The label begins with the braille spinner, then an optional count, then
  # "Working": `⠙, 2 Working p091543 one · Running sleep 80`. Requiring a
  # digit straight after "Button" matched none of them, and three cycles
  # reported the app drawing no line while it drew one every second.
  RAWLINE=$(ui dump | grep -E "Button +[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏], ([0-9]+ )?Working " | head -1)
  RUNNING=$(echo "$RAWLINE" | awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }')
  [ -n "$RUNNING" ] && break
done
if [ -z "$RUNNING" ]; then
  # Before blaming the app for not drawing a line, ask whether there was
  # anything to draw. Whether the root delegates at all is the model's
  # decision, not the phone's, and it has declined more than once tonight.
  KIDS=$(python3 "$HERE/../kernel.py" pod history 12 2>/dev/null | grep -c "$TAG" || true)
  if [ "${KIDS:-0}" = 0 ]; then
    echo "  the kernel has no child for this request: the root answered it itself."
    echo "  VERDICT: no worker ran, so there was no line to draw. Untested, and not the app's doing."
  else
    echo "  VERDICT: a worker ran and no line appeared — that is the app's to answer"
  fi
else
  echo "  the line: $RUNNING"
  shot 04-the-running-line
  # Tap the frame the dump gave, not a label. Two reasons: the spinner at
  # the front is animated, so an exact label read a moment ago is already
  # stale; and the worker's name appears on more than one element, so
  # matching by name can land somewhere that is not the button. Tapping by
  # label failed here for a whole cycle and read as the line refusing to
  # open — by frame it opens first time.
  LX=$(echo "$RAWLINE" | awk '{print $1}'); LY=$(echo "$RAWLINE" | awk '{print $2}')
  echo "  tapping its frame at $LX,$LY"
  idb ui tap "$LX" "$LY" --udid "$UDID" >/dev/null 2>&1
  sleep 4
  shot 05-worker-chat-from-its-line
  WHERE=$(where)
  echo "  landed on: $WHERE"
  case "$WHERE" in
    worker-chat*) echo "  VERDICT: a running worker's line opens its chat, as the sheet does";;
    *)            echo "  VERDICT: tapping the running line did not open a worker's chat — it left us on $WHERE";;
  esac
fi
echo
echo "stills in $OUT"

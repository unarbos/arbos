#!/bin/bash
# Does an attached photo actually reach the model?
#
#   photo-reaches-the-model.sh <cycle> [project]
#
# The attachments row has said "Not sent this cycle" since cycle 37, and the
# journey's P2 step could not answer it either: until cycle 56 the picker
# never closed, so nothing was attached — and the model answered the question
# anyway, plausibly, about a photo it could not see. A step that scores on
# "did a reply come back" cannot tell those apart.
#
# So this asks about a picture with content worth naming. The first photo in
# the simulator's library is a close-up of magenta flowers with yellow
# centres. If the reply names flowers or their colour, the bytes arrived; if
# it hedges, or talks about the words instead, they did not.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/photo"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
hist() { python3 "$HERE/../kernel.py" pod history 40 2>/dev/null; }

BEFORE=$(hist | tail -1 | awk '{print $1}')
echo "transcript at ${BEFORE:-?}"

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 8
ui tap "$ROW" >/dev/null || { echo "no $ROW row"; exit 1; }
sleep 4

echo "attaching the first photo in the library"
ui tap "Add" >/dev/null || { echo "no attachment button"; exit 1; }
sleep 1.5
ui tap "Photo Library" >/dev/null || { echo "no Photo Library"; exit 1; }
sleep 4
tap_shot 78 470 "$UDID"; sleep 1     # the magenta flowers, top-left of the grid
tap_shot 426 157 "$UDID"; sleep 3    # the tick, which closes the picker
ui field >/dev/null 2>&1 || { echo "the picker would not close — nothing was attached"; exit 1; }
shot 01-chip-attached
echo "  chip on the composer: $(ui dump | awk '$2 > 690 && $2 < 780' | grep -cE 'Image|Close')"

LINE="What is in the photo I just attached? Name the subject and its colour in one short sentence. If no image reached you, say exactly: no image reached me."
ui focus >/dev/null; sleep 0.7
idb ui text "$LINE" --udid "$UDID"
for _ in $(seq 1 80); do [ "$(ui field plain 2>/dev/null)" = "$LINE" ] && break; sleep 0.25; done
ui tap "Send" >/dev/null || { echo "no send button"; exit 1; }
echo "sent; waiting for the reply"
for _ in $(seq 1 24); do
  sleep 5
  R=$(hist | awk -v a="${BEFORE:-0}" '$1+0 > a+0' | grep -E "^ *[0-9]+ assistant" | tail -1)
  [ -n "$R" ] && break
done
sleep 3; shot 02-reply

echo
echo "--- what the model said ---"
echo "  ${R:-nothing came back}" | cut -c1-400
echo
if [ -z "${R:-}" ]; then
  echo "VERDICT: no reply — nothing proven"
elif echo "$R" | grep -qiE "no image reached me|didn.t (arrive|reach)|did not (arrive|reach)|can.t see|cannot see|no (image|photo|attachment)"; then
  echo "VERDICT: the model says no image reached it — the photo did NOT get through"
elif echo "$R" | grep -qiE "flower|blossom|petal|magenta|pink|bloom"; then
  echo "VERDICT: the model named the picture — the photo DID get through"
else
  echo "VERDICT: the reply names neither the subject nor a refusal; read it above and judge"
fi
echo "stills in $OUT"

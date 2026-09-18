#!/bin/bash
# The other half of the attachments row: files, not photos.
#
#   attach-a-file.sh <cycle> [project]
#
# The coverage row is "attachments (`+`), photos, **files**". Every
# measurement under it is about photos — `chip-and-send-arrow.sh`,
# `photo-reaches-the-model.sh`, the journey's P2. No scenario had ever
# opened the Files picker.
#
# **Attaching a file cannot be driven on this simulator**, and that is the
# useful part of what follows. The picker opens on `Recents`, which is
# empty, and `Browse` shows "On My iPhone is Empty": the app does not
# declare `UIFileSharingEnabled`, so its own Documents folder is not a place
# the picker can see, and a file dropped there — this scenario tried — does
# not appear. Making it appear means changing the app to suit the harness,
# which is the wrong way round, so it is written down instead of worked
# around.
#
# What can be driven is checked here, and it is not nothing: the `+` menu
# offers Files at all, the picker opens, and — the part with history — the
# way *out* is clean. In journey run 32 a picker left open swallowed the
# photo line and the whole call step after it.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/attach-a-file"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
chips() { ui dump | grep -cE "Button +Remove "; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
sleep 11
reach_the_list "$UDID" || exit 1
ui tap "$ROW" >/dev/null 2>&1 || { echo "no '$ROW' row"; exit 1; }
sleep 5
BEFORE_CHIPS=$(chips)
BEFORE_FIELD=$(ui field plain 2>/dev/null)

FAULTS=0
echo "== the + menu =="
ui tap "Add" >/dev/null 2>&1 || { echo "  no attachment button"; exit 1; }
sleep 2
MENU=$(ui dump | grep -oE "Button +(Files|Photo Library)" | awk '{print $2, $3}' | tr '\n' ' ')
echo "  offers: ${MENU:-nothing}"
case "$MENU" in
  *Files*) ;;
  *) echo "  FAULT: no Files in the attachment menu"; FAULTS=$((FAULTS + 1));;
esac

echo
echo "== the picker =="
ui tap "Files" >/dev/null 2>&1
sleep 5
xcrun simctl io "$UDID" screenshot "$OUT/01-picker.png" >/dev/null 2>&1
# The picker is another process, so `describe-all` sees nothing inside it.
# Its being invisible is how we know it is up: the app's own controls go.
if ui dump | grep -qE " TextField |Button +Add"; then
  echo "  the picker did not open — the composer is still on screen"
  FAULTS=$((FAULTS + 1))
else
  echo "  open (another process, so the tree cannot see inside it)"
fi

echo
echo "== the way out =="
# Close is the X at the picker's top right, in points on this device.
idb ui tap 355 97 --udid "$UDID" >/dev/null 2>&1
sleep 3
for _ in 1 2 3; do
  ui dump | grep -qE " TextField " && break
  idb ui swipe 196 300 196 850 --duration 0.4 --udid "$UDID" >/dev/null 2>&1
  sleep 2
done
xcrun simctl io "$UDID" screenshot "$OUT/02-back.png" >/dev/null 2>&1
if ui dump | grep -qE " TextField "; then
  echo "  the composer is back"
else
  echo "  FAULT: the picker would not close — nothing after this could be driven"
  FAULTS=$((FAULTS + 1))
fi
AFTER_CHIPS=$(chips)
AFTER_FIELD=$(ui field plain 2>/dev/null)
echo "  chips: $BEFORE_CHIPS before, $AFTER_CHIPS after"
[ "$AFTER_CHIPS" = "$BEFORE_CHIPS" ] || { echo "  FAULT: cancelling left an attachment behind"; FAULTS=$((FAULTS + 1)); }
[ "$AFTER_FIELD" = "$BEFORE_FIELD" ] || { echo "  FAULT: the composer's text changed across the picker"; FAULTS=$((FAULTS + 1)); }

echo
if [ "$FAULTS" = 0 ]; then
  echo "VERDICT: the + menu offers Files, the picker opens and closes, and"
  echo "         cancelling leaves the composer as it was."
else
  echo "VERDICT: $FAULTS fault(s) on the file path — named above."
fi
echo "         Attaching a file is NOT exercised: the picker has nothing to"
echo "         offer on this simulator (Recents empty, 'On My iPhone is"
echo "         Empty'), because the app does not expose its Documents to"
echo "         Files. That half of the row stays unmeasured, and says so."
echo "stills in $OUT"

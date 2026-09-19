#!/bin/bash
# COVERS: projects list — faces, rows, sections
#
# Does the projects list know a project is working?
#
#   does-the-list-know.sh <cycle> [project]
#
# The list is the screen a person scans to see what is busy. Cycle 187 found
# the pill inside a chat showing a tick while that chat's own composer showed
# Stop; this asks the same question one screen out.
#
# It matters more here. Inside a chat you have the composer and the running
# line to tell you; on the list the state word is all there is.
#
# The reading is only worth anything if the turn is still running at the
# moment the row is read, so that is checked from the chat side immediately
# before and immediately after — not assumed from a send a few seconds
# earlier.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/list-knows"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }
composer() { ui dump | grep -oE "Button +(Stop|Send|Microphone)$" | head -1 | awk '{print $2}'; }
state_of() { ui dump | grep -E "Button +$ROW, " | head -1 | sed -E "s/.*Button +$ROW, ([^,]+).*/\\1/"; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 11
reach_the_list "$UDID" || exit 1
BEFORE=$(state_of)
echo "before anything runs, the row says: ${BEFORE:-no row}"
shot 01-before

ui tap "$ROW" >/dev/null 2>&1 || { echo "no $ROW row"; exit 1; }
sleep 5
# Long enough that it is still going while the list is read and looked at.
type_line "$UDID" "Count slowly from one to forty, one number on each line, and say done at the end." || exit 1
ui tap "Send" >/dev/null 2>&1
for _ in $(seq 1 20); do [ "$(composer)" = Stop ] && break; sleep 1; done
[ "$(composer)" = Stop ] || { echo "the turn never started — nothing to ask about"; exit 1; }
echo "the turn is running: the composer reads Stop"

ui tap "Back" >/dev/null 2>&1; sleep 2
reach_the_list "$UDID" || exit 1
DURING=$(state_of)
WORKING_ROWS=$(ui dump | grep -cE "Button +[a-z0-9-]+, Working")
shot 02-while-it-works
echo "while its turn runs, the row says: ${DURING:-no row}"
echo "rows anywhere on the list reading Working: $WORKING_ROWS"

# Back in, to show the turn was still going the whole time the list was read.
ui tap "$ROW" >/dev/null 2>&1; sleep 3
STILL=$(composer)
echo "back in the chat, the composer reads: ${STILL:-nothing}"

echo
if [ "$STILL" != Stop ]; then
  echo "VERDICT: cannot say — the turn ended while the list was being read, so the"
  echo "         row saying '${DURING:-nothing}' may simply have been right"
elif [ "$DURING" = Working ]; then
  echo "VERDICT: the list knows — the row read 'Working' while the turn ran"
else
  echo "VERDICT: the list does not know. The turn was running before and after the"
  echo "         row was read, and the row said '${DURING:-nothing}'. On this screen"
  echo "         the state word is the only thing there is to tell you."
  echo "         Filed as a question, not a fault: a project's state comes from the"
  echo "         hub roster, which a turn in flight has not reached."
fi
echo "stills in $OUT"

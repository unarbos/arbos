#!/bin/bash
# COVERS: background minutes/hours → resume; what a returning user sees first
# Put the phone down in a chat. Pick it up later. Where are you?
#
#   what-a-returning-user-sees.sh <cycle> [project] [pause seconds] [second project]
#
# With a second project named: leave the first for the list, open the
# second, then the reclaim. The place he comes back to must be the second.
# (#615 recorded settings.kernelTarget at push time, which still named the
# first project — the chat's own switch runs after the push — so a cold
# start put him back in the one he had left.)
#
# The coverage row has been half answered since cycle 27. The resume half is
# done — 7 and 12 minutes backgrounded, the chat exactly as it was, a line
# sent straight afterwards answered (M-167). The other half, "hours", has
# stood as "needs Jacob's phone", and so has never been looked at here at
# all.
#
# But hours is not one case, it is two, and only the first needs his phone:
#
#   still suspended  — iOS kept the process; this is M-167's case, longer.
#   reclaimed        — iOS took the memory back, so returning is a cold
#                      launch. Nothing about that needs hours to reproduce;
#                      it needs the process gone, which is one command.
#
# The second is the one a person actually meets after a night, and it asks a
# question the first does not: does the app put them back in the chat they
# left, or on the list?
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}; PAUSE=${3:-120}; ROW2=${4:-}
OUT="$HOME/mobile-out/$CYCLE/returning-user"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }
# Where the app is, in one word, read off the tree rather than a screenshot.
# The chat has a Back button and a composer; the list has Projects and no
# Back.
where() {
  local d; d=$(ui dump)
  if echo "$d" | grep -qE "Button +Back"; then echo "chat:$(echo "$d" | awk '$3=="StaticText" && $2<120 {print $4; exit}')"
  elif echo "$d" | grep -qE "StaticText +Projects"; then echo "list"
  else echo "unknown"; fi
}
tail3() { ui dump | grep " StaticText " | tail -3 | md5; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 9
# The app comes back to the chat that was in front now, so the list may not
# be the first screen. Step back to it before starting, or the run cannot
# find the row it means to open.
reach_the_list "$UDID" || exit 1
ui tap "$ROW" >/dev/null || { echo "no $ROW row"; exit 1; }
sleep 5
shot 01-where-he-left-it
BEFORE_WHERE=$(where); BEFORE_TAIL=$(tail3)
echo "left it at:        $BEFORE_WHERE"

echo
echo "--- still suspended: Home, wait ${PAUSE}s, come back ---"
idb ui button HOME --udid "$UDID" >/dev/null 2>&1
sleep "$PAUSE"
xcrun simctl launch "$UDID" $B >/dev/null 2>&1
sleep 6
shot 02-back-after-a-pause
AFTER_WHERE=$(where); AFTER_TAIL=$(tail3)
echo "came back to:      $AFTER_WHERE"
[ "$AFTER_WHERE" = "$BEFORE_WHERE" ] && echo "  the same screen" || echo "  A DIFFERENT SCREEN"
[ "$AFTER_TAIL" = "$BEFORE_TAIL" ] && echo "  the same last three lines" || echo "  the last three lines changed"

echo
echo "--- reclaimed: the process is gone, as it would be after a night ---"
xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 2
xcrun simctl launch "$UDID" $B >/dev/null 2>&1
# Long enough for the project's identity to arrive. Read at 9 s the restored
# chat still carried its fallback title and the run called it a different
# project; the header settles to the right name a few seconds later.
sleep 15
shot 03-back-after-being-reclaimed
COLD_WHERE=$(where)
echo "came back to:      $COLD_WHERE"
case "$COLD_WHERE" in
  "$BEFORE_WHERE") echo "  VERDICT: put back in the chat he left, even though the app had been killed";;
  list)            echo "  VERDICT: the list. He left in a chat and returns to the list — whether that is right is a";
                   echo "           judgement, but it should be a decided one rather than a default";;
  *)               echo "  VERDICT: somewhere else entirely: $COLD_WHERE";;
esac

if [ -n "$ROW2" ]; then
  echo
  echo "--- left $ROW for the list, opened $ROW2, reclaimed: which one is he in? ---"
  ui tap "Back" >/dev/null 2>&1; sleep 2
  ui tap "$ROW2" >/dev/null || { echo "no $ROW2 row"; exit 1; }
  sleep 5
  SECOND_WHERE=$(where)
  echo "left it at:        $SECOND_WHERE"
  xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 2
  xcrun simctl launch "$UDID" $B >/dev/null 2>&1
  sleep 15
  shot 04-back-in-the-second-project
  SECOND_COLD=$(where)
  echo "came back to:      $SECOND_COLD"
  if [ "$SECOND_COLD" = "$SECOND_WHERE" ]; then echo "  VERDICT: the second project, the one he was in"
  elif [ "$SECOND_COLD" = "$BEFORE_WHERE" ]; then echo "  VERDICT: FAULT — the first project, the one he had left"
  else echo "  VERDICT: somewhere else: $SECOND_COLD"; fi
fi
echo
echo "still in $OUT"

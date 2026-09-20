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
echo "--- the project moved on while he was away ---"
# Every case above leaves the project idle, so "the same last three lines" is
# the right answer and the check cannot tell a chat that reconnected from one
# that is merely still showing what it showed before. The case a person
# actually meets after a night is the other one: work finished while the
# phone was in a pocket. Does the chat he returns to know about it?
reach_the_list "$UDID" || exit 1
ui tap "$ROW" >/dev/null 2>&1 || { echo "  no $ROW row"; exit 1; }
sleep 5
TAG=b$(date -u +%H%M%S)
if type_line "$UDID" "Start one worker whose goal is exactly $TAG late, which sleeps 60 seconds and then says the word finished. Wait for it and tell me when it is done."; then
  ui tap "Send" >/dev/null 2>&1
  sleep 12
  # Not a count of rows on screen. Coming back raises the keyboard and the
  # view scrolls, so the count fell from 7 to 6 on a run where the work had
  # plainly finished — the same mistake the tool fold made at cycle 162. What
  # proves the chat caught up is a line that was not there before: the turn's
  # "Worked <time>". While the worker runs there is none.
  BEFORE_ROWS=$(ui dump | grep -cE "StaticText")
  BEFORE_END=$(ui dump | grep -cE "Worked [0-9]+[a-z]|Turn ended")
  # Printed as context and labelled as context. The comment above has said
  # since cycle 182 that this count is not the evidence, but the output put
  # it immediately above the verdict where it reads as though it were — and
  # cycle 201 came back to *fewer* lines than it left with, 7 down to 4, on a
  # run that had plainly caught up. A reader then has to distrust either the
  # number or the verdict, and both are fine.
  echo "  left with:         $BEFORE_END ending line(s) — the count that matters"
  echo "                     ($BEFORE_ROWS line(s) on screen, which moves with the"
  echo "                      keyboard and the scroll and proves nothing)"
  shot 05-left-it-working
  idb ui button HOME --udid "$UDID"
  sleep 90
  xcrun simctl launch "$UDID" $B >/dev/null 2>&1
  sleep 8
  shot 06-came-back-to-finished-work
  # Wait for the ending rather than assuming ninety seconds is enough. A run
  # at cycle 182 came back to a turn still going and the check called it
  # "either the worker is slow or the chat did not catch up" — unable to say
  # which. The kernel could: the turn had completed at 2m 9s, because it
  # compacted 682 turns of history in the middle of it.
  AFTER_END=""
  for _ in $(seq 1 24); do
    AFTER_END=$(ui dump | grep -oE "Worked [0-9]+[a-z].*|Turn ended" | head -1)
    [ -n "$AFTER_END" ] && break
    sleep 5
  done
  AFTER_ROWS=$(ui dump | grep -cE "StaticText")
  echo "  came back to:      ${AFTER_END:-no ending line} — the count that matters"
  echo "                     ($AFTER_ROWS line(s) on screen)"
  if [ -n "$AFTER_END" ] && [ "$BEFORE_END" = 0 ]; then
    echo "  VERDICT: the chat caught up while the phone was away — he left a turn"
    echo "           running and came back to '$AFTER_END'"
  elif [ -n "$AFTER_END" ]; then
    echo "  VERDICT: cannot say — the turn already had an ending line before he left,"
    echo "           so coming back to one proves nothing about catching up"
  else
    # Ask the one witness that is not the screen. If the kernel has finished
    # and the chat has not said so, that is the app; if the kernel is still
    # going, the worker is simply slow and this run says nothing about
    # catching up.
    # `phone` is the pod's own kernel folded into the roster (M-121), so its
    # record is readable as `pod`. Any other project would need its
    # machine/project target, which this scenario is not given.
    if [ "$ROW" != phone ]; then
      echo "  VERDICT: cannot say — no ending on screen after two minutes, and this"
      echo "           check only knows how to ask the kernel behind 'phone'"
      KDONE=skip
    else
      KDONE=$(python3 "$HERE/../kernel.py" pod history 6 2>/dev/null | grep -c "turn_complete")
    fi
    if [ "$KDONE" = skip ]; then
      :
    elif [ "${KDONE:-0}" -gt 0 ]; then
      echo "  VERDICT: the kernel finished the turn and the chat never showed its"
      echo "           ending in two minutes of watching — the chat did not catch up"
    else
      echo "  VERDICT: cannot say — the kernel is still working after two minutes, so"
      echo "           there was no ending for the chat to miss. The worker outran its"
      echo "           own 60s sleep (a history compaction will do it)"
    fi
  fi
else
  echo "  could not ask for a worker, so this half did not run"
fi
echo
echo "still in $OUT"

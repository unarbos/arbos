#!/bin/bash
# COVERS: project chat — send, prompt card, streaming, Worked line
# Tool calls fold into one line, and the line opens.
#
#   tool-fold.sh <cycle> [project]
#
# This came from Jacob, build 956: "I don't want to see these tool calls on
# the phone unless maybe I expand" (F13). Consecutive calls became one dim
# line — "4 tool calls · 12s", failures counted — that opens on a tap.
#
# Nothing has checked it since. `fold37.sh` is cycle-37 scratch: it reads
# `~/mobile-out/cycle-37` and takes its tools from `$HOME`, so it cannot run
# today and would not be measuring this if it did.
#
# Two claims, and the second is the one with a person behind it: the calls
# fold, **and the fold opens**. A fold that cannot be opened is not tidier
# than the old list, it is the same information hidden.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/tool-fold"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
rows() { ui dump | grep -cE "StaticText"; }
# Counting every StaticText on screen cannot see a fold open, because opening
# one pushes the transcript up and takes as many lines off the top as it adds
# below. Cycle 161 read "4 → 4" that way and called it a fold that refused to
# open. The lines themselves are compared instead: what is on screen when it
# is open that was not there when it was closed.
lines() { ui dump | awk '$3 == "StaticText" { $1=""; $2=""; $3=""; sub(/^ +/, ""); print }' | sort -u; }
foldline() { ui dump | grep -oE "[0-9]+ tool calls?[^\"]*" | tail -1; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
sleep 11
reach_the_list "$UDID" || exit 1
ui tap "$ROW" >/dev/null 2>&1 || { echo "no '$ROW' row"; exit 1; }
sleep 5
ui field >/dev/null 2>&1 || { echo "no composer in '$ROW'"; exit 1; }

MARK="fold $(date -u +%H%M%S)"
echo "== give it several tool calls to fold =="
# Plain ASCII: a line with an em dash types nothing at all (cycle 149).
LINE="$MARK: run these three bash commands one after another and nothing else: echo one, echo two, echo three. Then reply done."
# Read the field back before sending. `idb ui text` returns before its
# characters arrive, and the first run of this typed into an unfocused
# composer, sent nothing, and reported "no fold line appeared" — a verdict
# about the app from a turn that never left the phone. M-162 is exactly
# this, and the loop has a file about it.
if ! type_line "$UDID" "$LINE"; then
  echo
  echo "VERDICT: none — nothing was asked, so nothing folded and this says"
  echo "         nothing about folding"
  exit 1
fi
ui tap "Send" >/dev/null 2>&1

FOLD=""
for _ in $(seq 1 60); do
  FOLD=$(foldline)
  [ -n "$FOLD" ] && break
  sleep 2
done
xcrun simctl io "$UDID" screenshot "$OUT/01-folded.png" >/dev/null 2>&1
if [ -z "$FOLD" ]; then
  echo "  no fold line appeared"
  echo
  echo "VERDICT: none — the turn made no folded tool calls, so there was"
  echo "         nothing to open and this says nothing about folding"
  exit 1
fi
echo "  the fold reads: $FOLD"
# Wait for the turn to finish before counting anything. The first working
# run compared rows across the toggle while the reply was still arriving and
# reported "closing it left 1 row behind" — that row was the model's answer
# landing, not the fold failing to close. Counts are only comparable over a
# transcript that has stopped moving.
for _ in $(seq 1 60); do
  case "$(ui dump)" in *"Worked "*) break;; esac
  sleep 2
done
sleep 2
CLOSED=$(rows)
lines > "$OUT/closed.txt"
echo "  text rows while closed: $CLOSED"

echo
echo "== open it =="
# Tapped by the words it shows, not by a position: the fold moves as the
# transcript grows underneath it.
ui tap "$(echo "$FOLD" | grep -oE "^[0-9]+ tool calls?")" >/dev/null 2>&1
sleep 3
OPEN=$(rows)
lines > "$OUT/open.txt"
xcrun simctl io "$UDID" screenshot "$OUT/02-open.png" >/dev/null 2>&1
REVEALED=$(comm -13 "$OUT/closed.txt" "$OUT/open.txt" | grep -c .)
echo "  text rows while open:   $OPEN"
echo "  lines opening revealed: $REVEALED"
comm -13 "$OUT/closed.txt" "$OUT/open.txt" | cut -c1-64 | sed 's/^/      /' 

echo
echo "== close it again =="
ui tap "$(echo "$FOLD" | grep -oE "^[0-9]+ tool calls?")" >/dev/null 2>&1
sleep 3
AGAIN=$(rows)
lines > "$OUT/again.txt"
LEFT=$(comm -13 "$OUT/closed.txt" "$OUT/again.txt" | grep -c .)
echo "  text rows closed again: $AGAIN"
echo "  lines still showing:    $LEFT (was $REVEALED when open)"

echo
FAULTS=0
# A fold of one call has nothing under it to reveal: the summary line *is*
# the call, and an opened fold shows labels rather than output (F14). So the
# open/close comparison only means something with two or more, and saying
# otherwise turns the model's choice of how many commands to run into a
# fault in the app.
N=$(echo "$FOLD" | grep -oE "^[0-9]+")
if [ "${N:-1}" -lt 2 ]; then
  echo "  the model made $N tool call this run, so the fold has nothing to"
  echo "  reveal and opening it is untested. Run again for two or more."
  echo
  echo "VERDICT: the calls folded into '$FOLD'; opening was not exercised"
  echo "stills in $OUT"
  exit 0
fi
[ "${REVEALED:-0}" -gt 0 ] || { echo "  FAULT: opening the fold revealed no line that was not already there"; FAULTS=$((FAULTS + 1)); }
[ "${LEFT:-0}" = 0 ] || { echo "  FAULT: closing it left $LEFT of those $REVEALED line(s) on screen"; FAULTS=$((FAULTS + 1)); }
if [ "$FAULTS" = 0 ]; then
  echo "VERDICT: '$FOLD' hides $REVEALED line(s) and gives them back on a tap, and"
  echo "         takes them away again when it closes"
else
  echo "VERDICT: $FAULTS fault(s) in the fold — named above"
fi
echo "stills in $OUT"

#!/bin/bash
# COVERS: the harness — typing into the composer
#
# Does the rig's own typing do what it claims?
#
#   typing-into-the-composer.sh <cycle> [project]
#
# Every scenario that sends anything goes through `type_line`, and the row it
# covers has never had a check of its own. It has earned one: more silent
# passes here have come from typing than from any other single thing.
#
#   * `idb ui text` types *nothing at all* for a line holding any non-ASCII
#     character, and says nothing about it (M-464). A scenario that typed an
#     em-dash sent an empty composer and read the reply to a question it
#     never asked.
#   * iOS turns a typed " into a curly quote, so a read-back against the
#     string you asked for never matches (M-185).
#
# `type_line` was written to close both. This asks whether it still does, in
# the three ways it can fail: a plain line must arrive whole, a long line must
# arrive whole, and a line it cannot type must be refused rather than
# half-typed. The third is the one that matters — a helper that refuses
# loudly is the only reason the other scenarios can trust a read-back.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/typing"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }
field() { ui field plain 2>/dev/null; }

open_the_chat() {
  xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
  xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 10
  reach_the_list "$UDID" || return 1
  ui tap "$ROW" >/dev/null || { echo "  no $ROW row"; return 1; }
  sleep 5
}
open_the_chat || exit 1

FAULTS=0
say() { printf '  %-13s %s\n' "$1" "$2"; }

echo "== a plain line =="
PLAIN="the quick brown fox jumps over the lazy dog"
if type_line "$UDID" "$PLAIN"; then
  GOT=$(field)
  case "$GOT" in
    *"$PLAIN"*) say "arrived" "whole";;
    *) say "WRONG" "asked for '$PLAIN', the field holds '${GOT:-nothing}'"; FAULTS=$((FAULTS + 1));;
  esac
else
  say "WRONG" "refused a line that is entirely ASCII"; FAULTS=$((FAULTS + 1))
fi
shot 01-plain
open_the_chat || exit 1

echo
echo "== a long line =="
# Long enough to wrap the composer several times. A single `idb ui text` has
# been seen to land partially, which is what the read-back loop is for.
LONG="This line is deliberately long so the composer has to grow and the read back has to wait for every last word of it to arrive before anything is compared, which is the whole point of the helper that types it."
if type_line "$UDID" "$LONG"; then
  GOT=$(field)
  if [ "${#GOT}" -ge "${#LONG}" ] && case "$GOT" in *"every last word of it"*) true;; *) false;; esac; then
    say "arrived" "${#GOT} characters, ending intact"
  else
    say "WRONG" "asked for ${#LONG} characters, the field holds ${#GOT}"; FAULTS=$((FAULTS + 1))
  fi
else
  say "WRONG" "refused a long line that is entirely ASCII"; FAULTS=$((FAULTS + 1))
fi
shot 02-long
open_the_chat || exit 1

echo
echo "== a line it cannot type =="
# An em-dash. idb drops the whole line for this and reports success.
BAD="this line has an em dash — right here"
BEFORE=$(field)
# Refusing is not enough to prove the guard is there. With the guard removed
# this still refuses, because idb types nothing and the read-back loop gives
# up — so the check passed a sabotaged helper. The two are told apart by how
# long they take: the guard answers before typing anything, the read-back
# spends three attempts of six waits each first.
T0=$(date +%s.%N)
if type_line "$UDID" "$BAD" 2>/dev/null; then
  say "WRONG" "claimed to type a line with a character idb drops"; FAULTS=$((FAULTS + 1))
else
  TOOK=$(python3 -c "import time;print(f'{time.time()-$T0:.1f}')")
  if [ "$(python3 -c "print(1 if $TOOK < 3 else 0)")" = 1 ]; then
    say "refused" "in ${TOOK}s, before typing anything"
  else
    say "WRONG" "refused, but only after ${TOOK}s — that is the read-back giving up,"
    say "" "not the guard. The line was handed to idb, which drops it silently."
    FAULTS=$((FAULTS + 1))
  fi
fi
AFTER=$(field)
if [ "$AFTER" = "$BEFORE" ]; then
  say "and left" "the composer as it found it"
else
  say "WRONG" "refused but the composer changed: '${BEFORE:-empty}' → '${AFTER:-empty}'"
  FAULTS=$((FAULTS + 1))
fi
shot 03-refused

# Leave nothing behind for the next scenario to trip over.
xcrun simctl terminate "$UDID" $B 2>/dev/null

echo
echo "stills in $OUT"
if [ "$FAULTS" = 0 ]; then
  echo "VERDICT: the rig types what it is given, whole, and refuses what it cannot"
  echo "         type instead of sending an empty line in its place"
else
  echo "VERDICT: $FAULTS fault(s) above. Every scenario that sends anything goes"
  echo "         through this, so read them before reading any other verdict"
  exit 1
fi

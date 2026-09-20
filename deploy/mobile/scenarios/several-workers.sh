#!/bin/bash
# COVERS: project chat — several workers at once, archived children
# Several workers at once, and what the chat and the sheet say about them.
#
#   several-workers.sh <cycle> [project]
#
# Cycle 37 established the shape: four workers asked for together, the Agents
# pill climbing, a Done line each in the transcript, and the sheet listing
# them all by name — kept across going back and reopening. This re-measures
# it, and counts rather than describes.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"   # page_up: scroll in points
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/workers"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }
pill() { ui dump | grep -oE "Agents [0-9]+|Working [0-9]+" | tr '\n' ' '; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 8
reach_the_list "$UDID" || exit 1
ui tap "$ROW" >/dev/null || { echo "no $ROW row"; exit 1; }
sleep 4
echo "pill before: $(pill)"
shot 01-before

# Every run used to ask for the same four goals, so the kernel named the
# workers the same way and the sheet filled with repeats — which is what made
# it uncountable by label (M-305). The tag goes at the *front* of each goal
# so it survives the sheet truncating a long name at the end.
TAG=w$(date -u +%H%M%S)
# No quotation marks and no dashes in this line. iOS turns a typed " into a
# curly quote, the read-back never matches, and the scenario sends whatever
# is in the box after its 20-second wait — which is how this run sent a
# garbled instruction and got no workers at all (M-185, in a new place).
LINE="Start four workers at once, each doing one trivial thing and waiting for none of the others. Their goals are exactly $TAG rivers, $TAG mountains, $TAG sea and $TAG sky. Each says one sentence about its word. Tell me when all four are done."
ui focus >/dev/null || { echo "no composer"; exit 1; }
sleep 0.7
idb ui text "$LINE" --udid "$UDID"
for _ in $(seq 1 80); do [ "$(ui field plain 2>/dev/null)" = "$LINE" ] && break; sleep 0.25; done
ui tap "Send" >/dev/null || { echo "no send button"; exit 1; }

echo "watching the pill for two minutes"
HIGH=0
for t in $(seq 1 40); do
  sleep 3
  n=$(ui dump | grep -oE "Agents [0-9]+" | grep -oE "[0-9]+" | tail -1)
  [ -n "${n:-}" ] && [ "$n" -gt "$HIGH" ] && { HIGH=$n; echo "  $((t*3))s: Agents $n"; }
  ui dump | grep -qE "Worked [0-9]" && [ "$t" -gt 8 ] && break
done
sleep 4; shot 02-after-the-work
echo "pill after: $(pill)   highest seen: $HIGH"
echo "Done lines in the transcript: $(ui dump | grep -cE 'Done [a-z]')"

echo
echo "== the sheet =="
ui tap "Agents $HIGH" >/dev/null 2>&1 || ui tap "Agents" >/dev/null 2>&1 || echo "  could not open the sheet"
sleep 2; shot 03-workers-sheet
# The labels read "<goal>, Done" — the state is last, with no space after
# it, so a grep for "Done " counts none of them and reports an empty sheet
# over a full one.
# Paged to the end, not counted off one screen. The sheet scrolls, only
# rendered rows reach the tree, and a single dump gave 12 while the pill
# said 22 — the fault M-287 withdrew a finding over. That fix went into the
# two scenarios written the same hour and not into this one, which had it
# already.
SHEET=$OUT/sheet-rows.txt
HOW=$(collect_rows "$UDID" ', (Done|Working)$' "$SHEET")
echo "  rows in the sheet: $(wc -l < "$SHEET" | tr -d ' ')  (paging $HOW)"
# The four this run made, told apart from every earlier run's by the tag.
MINE=$(grep -c "$TAG" "$SHEET" | tr -d ' ')
echo "  of them, this run's ($TAG): $MINE of 4"
grep "$TAG" "$SHEET" | sed 's/^/     /'
[ "$MINE" = 4 ] && echo "  VERDICT: all four of this run's workers are on the sheet" \
                || echo "  VERDICT: $MINE of this run's four reached the sheet"
ui dump | grep -E ', (Done|Working)$' | head -8 | sed 's/^/    /'

echo
echo "== back and reopen =="
# Looked for before it is tapped. Written as `tap … || swipe` this asked for
# a button that is deliberately not expected, so every run wrote a refusal
# into refused-taps.log — and a log with a false entry every time is a log
# nobody reads, which is precisely what it was added at cycle 195 to avoid.
if ui dump | grep -q "End call"; then
  ui tap "End call" >/dev/null 2>&1
else
  idb ui swipe 196 300 196 800 --duration 0.3 --udid "$UDID"
fi
sleep 2
# One Back is not "on the list": the sheet or the call may still be up, and
# after the sheet a single Back lands on the chat. The first tap of this
# scenario was given reach_the_list at cycle 116 and this one was not —
# check-tools looks at the first use in a file, so a scenario that returns
# to the list halfway through fails the same way later and the check stays
# quiet. This run said `nothing matching 'phone' on screen`.
reach_the_list "$UDID" || { echo "  could not get back to the list"; exit 1; }
ui tap "$ROW" >/dev/null || { echo "  '$ROW' is not on the list"; exit 1; }
sleep 4
AFTER=$(pill)
echo "  pill after reopening: $AFTER"
shot 04-reopened
echo "stills in $OUT"
echo
# The row's claim is that the workers are still there after leaving and
# coming back, so say whether they were.
BEFORE_N=$(echo "$HIGH" | grep -oE "[0-9]+" | head -1)
# `Agents N` only. HIGH is an Agents count, and the pill reads `Working N`
# while anything runs — the running ones, a subset. Taking the first number
# from either form would compare a subset with a whole and call a busy
# project a lost one, which is the mistake cycle 154 spent a cycle on in the
# workers sheet.
AFTER_N=$(echo "$AFTER" | grep -oE "Agents [0-9]+" | grep -oE "[0-9]+" | head -1)
if [ -z "$AFTER_N" ] && echo "$AFTER" | grep -q "Working"; then
  echo "VERDICT: cannot say — the pill reads '$AFTER' after reopening, which counts"
  echo "         only what is running. Comparing that with $HIGH agents would be a"
  echo "         subset against a whole."
elif [ -z "$AFTER_N" ]; then
  echo "VERDICT: no pill after reopening — the workers did not survive the trip,"
  echo "         or the chat did not finish opening"
elif [ -n "$BEFORE_N" ] && [ "$AFTER_N" -ge "$BEFORE_N" ]; then
  echo "VERDICT: $MINE of 4 named in the sheet, and the pill reads $AFTER after"
  echo "         going back and reopening — the workers are kept"
else
  echo "VERDICT: the pill read $HIGH at its highest and $AFTER after reopening —"
  echo "         fewer than were there, so the trip lost some"
fi

echo
echo "== what a finished worker keeps =="
# The row this covers is "several workers at once, archived children", and
# only the first half has ever been driven. The second was filed as an open
# question at cycle 32 and left: a Done worker's chat read "Nothing on record
# yet." even locally, because the kernel's history answered total 0 for an
# archived agent.
PILL=$(ui dump | grep -E "Button +([^,]+, )?(Agents|Working) [0-9]+" | head -1 | awk '{print $1, $2}')
if [ -n "$PILL" ]; then
  idb ui tap $PILL --udid "$UDID"; sleep 4
  # One of this run's own workers, if the sheet is showing one. Taking the
  # first Done row meant always opening the same ancient worker — "count
  # slowly one to forty", from some cycle long past — so the check proved
  # that a months-old record survives and said nothing about the four
  # workers it had just watched finish.
  # Page towards them. The newest workers sit at the end of the sheet
  # (M-482), so this run's four are never on the first screen — falling back
  # without looking meant the check always opened the oldest worker there is.
  # Only rows a finger could actually reach. `describe-all` returns the whole
  # scroll view, so a row paged above the top is still in the dump with a
  # negative y — this run saw -188 and -119 — and picking by tree order took
  # one of those. `ui tap` then refused it, quietly, because the error went
  # to /dev/null, and the check read the screen that was already there and
  # reported landing on the wrong worker. The screen is 852 points tall.
  mine_row() {
    python3 "$HERE/../ui.py" "$UDID" dump 2>/dev/null \
      | grep -E "Button +$TAG.*, Done" \
      | awk '$2 + 0 > 60 && $2 + 0 < 800' | head -1 \
      | awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }'
  }
  DONE_ROW=$(mine_row)
  PAGED=0
  while [ -z "$DONE_ROW" ] && [ "$PAGED" -lt 8 ]; do
    idb ui swipe 196 700 196 250 --duration 0.3 --udid "$UDID" >/dev/null 2>&1
    sleep 1.5
    PAGED=$((PAGED + 1))
    DONE_ROW=$(mine_row)
  done
  [ "$PAGED" = 0 ] || echo "  paged $PAGED screen(s) to reach this run's workers"
  # Let the sheet stop moving before tapping it. A swipe leaves momentum, and
  # tapping a label read mid-glide lands where that row *was*: cycle 185
  # tapped one worker and opened another's chat on one run, and nothing at
  # all on the next — the sheet was still under the finger both times.
  if [ "$PAGED" -gt 0 ]; then
    sleep 2
    DONE_ROW=$(mine_row)
    [ -n "$DONE_ROW" ] || echo "  the row moved away while the sheet settled"
  fi
  MINE=yes
  if [ -z "$DONE_ROW" ]; then
    DONE_ROW=$(first_worker_row "$UDID" Done)
    MINE=no
  fi
  echo "  opening: ${DONE_ROW:-no finished worker on the sheet}$([ "$MINE" = yes ] && echo "  (this run's own)" || echo "  (an older one — none of this run's are on this page)")"
  if [ -n "$DONE_ROW" ]; then
    # Not silenced. A refused tap and a tap that landed wrong look identical
    # on the screen afterwards, and this check spent cycle 195 reporting the
    # second when it was the first.
    TAPPED=$(ui tap "$DONE_ROW" 2>&1) || echo "  the tap was refused: $TAPPED"
    sleep 5
    shot 05-a-finished-worker
    # Did it land on the worker that was tapped? Cycle 185 tapped
    # "w191149 rivers, Done" and opened a chat headed "say sentence about
    # th…" — a different worker entirely — and the check said "opening:
    # w191149 rivers" and then reported on whatever it found, without
    # noticing. A row you tapped and a chat you are in are two claims.
    LANDED=$(ui dump | awk '$2+0 < 120 && $3 == "StaticText" { $1=""; $2=""; $3=""; sub(/^ +/, ""); print; exit }')
    WANTED=$(echo "$DONE_ROW" | sed 's/, Done$//')
    case "$LANDED" in
      "$WANTED"*) echo "  landed on: $LANDED";;
      *) echo "  LANDED ELSEWHERE: tapped '$WANTED' and the header reads '$LANDED'"
         echo "  Nothing below is about the worker this run chose."
         MINE=elsewhere;;
    esac
    HELD=$(ui dump | grep -cE "StaticText")
    EMPTY=$(ui dump | grep -c "Nothing on record yet")
    ui dump | awk '$3 == "StaticText" { $1="";$2="";$3=""; sub(/^ +/,""); print }' \
      | head -4 | cut -c1-64 | sed 's/^/     /'
    if [ "$MINE" = elsewhere ]; then
      echo "  VERDICT archived: cannot say — the tap opened a different worker's chat"
    elif [ "$EMPTY" != 0 ]; then
      echo "  VERDICT archived: still 'Nothing on record yet.' — the finding from cycle 32 stands"
    elif [ "$HELD" -ge 2 ]; then
      echo "  VERDICT archived: its chat holds $HELD line(s) of what it did — cycle 32's"
      echo "                    'Nothing on record yet' no longer reproduces"
      [ "$MINE" = no ] && echo "                    (on an older worker; this run's were not on the page)"
    else
      echo "  VERDICT archived: cannot say — $HELD line(s) and no empty-state text either"
    fi
    ui tap "Back" >/dev/null 2>&1; sleep 2
  fi
else
  echo "  no pill in this chat, so the sheet could not be opened"
fi


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
PILL=$(ui dump | grep -E "Button +(Agents|Working) [0-9]+" | head -1)
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
# A finished worker leaves a `Done <goal>` line in the transcript; tapping
# it should reach the same chat. This is the half never driven.
# The line the kernel leaves reads `<worker> · Turn ended. Last words: …`,
# and it is *older* than the tail, so the search has to walk backwards —
# `page_up` goes towards the newest line and would never reach it.
find_line() { ui dump | grep -E "Turn ended|, Done$" | head -1 | awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }'; }
LINE=$(find_line)
for _ in 1 2 3 4 5 6; do
  [ -n "$LINE" ] && break
  page_back "$UDID" >/dev/null 2>&1
  sleep 1
  LINE=$(find_line)
done
if [ -z "$LINE" ]; then
  echo "  VERDICT: no Done line to tap, so this half is untested again"
else
  echo "  the line: $LINE"
  ui tap "$LINE" >/dev/null 2>&1
  sleep 4
  shot 04-worker-chat-from-its-line
  WHERE=$(where)
  echo "  landed on: $WHERE"
  case "$WHERE" in
    worker-chat*) echo "  VERDICT: the line opens the worker's chat, as the sheet does";;
    *)            echo "  VERDICT: tapping the line did not open a worker's chat — it left us on $WHERE";;
  esac
fi
echo
echo "stills in $OUT"

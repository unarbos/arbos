#!/bin/bash
# COVERS: the chrome — what each control is called
#
# Markdown the app forgot to render, anywhere a person can see it.
#
#   no-raw-markdown.sh <cycle>
#
# Cycle 203 found the "While you were away" card printing
# `**Sun Sep 20 06:06:42 UTC 2026**` with its asterisks, directly beneath the
# transcript line showing that same date in bold. The card drew the model's
# words with a plain Text while the transcript passes them through
# `ChatRow.prose`. One line fixed it and nothing would have caught it coming
# back — or caught the next surface that renders model text and misses the
# helper.
#
# So this walks the screens that carry model words and looks for the markers
# themselves. Only unambiguous ones: `**bold**`, `__bold__`, a backtick, and
# `](` from a link. A hyphen bullet and a full stop are ordinary prose and
# are not looked for.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/markdown"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

FAULTS=0
# Every visible string on the screen, then the markers. The text is kept in a
# file per screen so a hit can be read rather than guessed at.
check() {
  local what=$1
  ui dump > "$OUT/$what.txt" 2>/dev/null
  local text hits
  # On-screen rows only. This check is about what a person can see, and its
  # first run read a transcript row at y=-572 — above the top, in a dump that
  # carries the whole scroll view — and called it a fault. That is the
  # mistake found in several-workers at 195 and in tool-fold at 197, made
  # again here in the check written to catch a different one.
  text=$(awk '$2 + 0 > 40 && $2 + 0 < 820 { $1=""; $2=""; $3=""; sub(/^ +/, ""); print }' "$OUT/$what.txt")
  hits=$(echo "$text" | grep -nE '\*\*|__|`|\]\(' | head -4)
  if [ -n "$hits" ]; then
    echo "  $what: RAW MARKDOWN"
    echo "$hits" | sed 's/^/      /'
    FAULTS=$((FAULTS + 1))
  else
    echo "  $what: clean ($(echo "$text" | grep -cE '.') line(s) read)"
  fi
}

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 8
reach_the_list "$UDID" || { echo "could not reach the list"; exit 1; }

echo "screens read:"
check the-list

ui tap "$ROW" >/dev/null 2>&1 || { echo "  no $ROW row"; exit 1; }
sleep 5
check the-chat

# Older lines, where the model's longer answers are.
page_back "$UDID" >/dev/null 2>&1
sleep 1
check the-chat-scrolled-back

# A worker's chat: its own report, written by a model, on a different screen.
if ui try-tap "Agents" >/dev/null 2>&1; then
  sleep 3
  W=$(ui dump | grep -E ", Done" | awk '$2 + 0 > 60 && $2 + 0 < 800' | head -1 \
    | awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }')
  if [ -n "$W" ] && ui tap "$W" >/dev/null 2>&1; then
    sleep 4
    check a-workers-chat
  else
    echo "  a-workers-chat: not reached — no finished worker within reach on the sheet"
  fi
else
  echo "  a-workers-chat: not reached — no workers pill on this chat"
fi

echo
if [ "$FAULTS" = 0 ]; then
  echo "VERDICT: no raw markdown on any screen read — the marker characters"
  echo "         appear nowhere a person can see them"
else
  echo "VERDICT: $FAULTS screen(s) show markdown the app did not render. Each is"
  echo "         a surface drawing model words without ChatRow.prose, which is"
  echo "         what cycle 203 found in the away card."
fi
echo "the text of each screen is in $OUT"

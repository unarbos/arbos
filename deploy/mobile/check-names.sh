#!/bin/bash
# Does every control on screen have a name a person would recognise?
#
#   check-names.sh <cycle> [project]
#
# When a SwiftUI control has no `accessibilityLabel`, the system reads the SF
# Symbol's own name instead — and it reads like a name, so nothing looks
# wrong. This app shipped `Gear Shape` for settings, `Move` for a project's
# glyph, `Up` for send, `Close` for the call's end button, and two bare
# `PopUpButton`s, for weeks (M-264, M-297, M-314).
#
# Two audiences suffer, and the second is why this is a harness file rather
# than a nice-to-have: a person using VoiceOver hears furniture instead of
# purpose, and every scenario that taps one of those names is coupled to a
# *missing* label, so it breaks the moment somebody supplies one. That
# happened three times in one day (M-307, M-313).
#
# So this walks the screens and flags anything whose name looks like a
# rendered symbol rather than a decision.
#
# Where these faults live, learned the hard way at cycles 141 and 142: a
# fix applied *inside* a `Button`'s label does not hold. A button builds its
# label by walking its children, and that walk varies between runs — the
# projects row's separator was hidden, read clean, and came back twice. A
# fix in a view that is not a composed button label (the chat's worker line,
# the away card's bullet) has held since it was made. When this check flags
# something under a button, say it on the button.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/ui.py" "$UDID" "$@"; }

# A name is suspect when it is the element's own type, or Title Case words
# that spell an SF Symbol. The list is what this app has actually shipped,
# plus the shapes those take, rather than a guess at Apple's catalogue.
# The detector lives in a file, not a heredoc. `python3 - <<EOF` takes its
# *program* from stdin, so piping a dump into it fed the data nowhere and
# the check silently passed everything. Its own self-test caught that.
SUSPECT_PY=$(mktemp -t suspect)
cat > "$SUSPECT_PY" <<'DETECTOR'
import re, sys
BAD_EXACT = {
    "PopUpButton", "Button", "Image", "StaticText", "TextField",
    "Gear Shape", "Move", "Up", "Close", "Xmark", "Ellipsis",
    "Magnifyingglass", "Chevron Left", "Chevron Right", "Plus",
    "Arrow Turning Down Then Right", "Line 3 Horizontal",
    "Mic", "Mic Fill", "Mic Slash", "Speaker Wave 2", "Phone Down",
    "Slider Horizontal 3", "Doc", "Checkmark", "Arrow Up",
}
# A symbol name is not always two words. The multi-word rule below walked
# straight past `Circle`, the away card's bullet, sitting one row from a
# name it did flag. These are shapes and objects nobody names a control
# after, so one word is enough to be sure.
BAD_EXACT |= {
    "Circle", "Square", "Triangle", "Star", "Bolt", "Bell", "Trash",
    "Folder", "Gear", "Person", "Clock", "Hammer", "Wrench", "Paperclip",
    "Pencil", "Bookmark", "Flag", "House", "Tag", "Bubble", "Chevron",
    "Arrow", "Circle Fill", "Questionmark Circle", "Exclamationmark Circle",
}
# Two or more capitalised words with no lower-case connective reads like a
# symbol spelled out ("Arrow Turning Down Then Right"), not like a label.
# Every word a capital *letter*: "Arrow Turning Down Then Right" is a
# symbol spelled out, "Agents 36" is a pill with a count in it and was
# being flagged because the pattern let a digit begin a word.
SYMBOLISH = re.compile(r"^(?:[A-Z][a-z]*)(?: [A-Z][a-z]*){1,5}$")
OURS = (
    "Back", "Send", "Mute", "Unmute", "Filter", "Search", "Settings",
    "More", "Call",
    # UIKit draws and names the sheet's drag handle. An app cannot relabel
    # it, so flagging it only teaches the reader to ignore the report.
    "Sheet Grabber",
)
for line in sys.stdin.read().splitlines():
    parts = line.split(None, 3)
    if len(parts) < 4:
        continue
    kind, label = parts[2], parts[3].strip()
    if kind not in ("Button", "Image", "PopUpButton"):
        continue
    if label in BAD_EXACT or (SYMBOLISH.match(label) and label not in OURS):
        print(f"  {kind:12} {label}")
DETECTOR
trap 'rm -f "$SUSPECT_PY"' EXIT
suspect() { python3 "$SUSPECT_PY"; }

screen() {
  local what=$1
  echo "== $what =="
  local bad tree looked
  tree=$(ui dump)
  # How many things were examined, not only how many were wrong. "Every
  # control here is named" over three elements is a different sentence from
  # the same words over thirty, and cycle 142 spent five runs believing a
  # verdict that was true of an empty set.
  looked=$(echo "$tree" | grep -cE "^ *[0-9-]+ +[0-9-]+ +(Button|Image|PopUpButton) ")
  bad=$(echo "$tree" | suspect)
  VISITED=$((VISITED + 1))
  if [ -z "$bad" ]; then
    echo "  every control here is named   ($looked examined)"
  else
    echo "$bad"
    FOUND=$((FOUND + 1))
  fi
}

# Prove the detector can fail before trusting it to pass. Today's lesson,
# eight times over, is that a check reports "fine" just as confidently when
# it has stopped looking — so feed it names this app really did ship and
# require it to object to them.
SELFTEST=$(printf '%s\n' \
  "  42   85  Button       Gear Shape" \
  " 351   85  PopUpButton  PopUpButton" \
  "  26  196  Image        Arrow Turning Down Then Right" \
  " 299   85  Button       Search" \
  " 196  120  Button       Sheet Grabber" \
  "  39  713  Image        Circle" | suspect | grep -c .)
if [ "$SELFTEST" != 4 ]; then
  echo "the detector failed its own self-test ($SELFTEST of 4 known-bad names caught)."
  echo "Not running: a check that cannot fail cannot pass either."
  exit 1
fi
echo "detector self-test: caught 4 of 4 known-bad names, and let 'Search' and"
echo "the system's 'Sheet Grabber' through"
echo

FOUND=0
# How many screens were actually looked at. A tap that misses used to skip a
# screen silently and the verdict still said the app was clean: cycle 117
# reported "no control reads as a symbol name" having never opened the chat
# or the sheet. A check that cannot reach a screen has not cleared it.
VISITED=0
MISSED=
xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 9
# The app comes back to the chat that was in front (#615), so reach the list
# deliberately rather than assuming it is first.
ui dump | grep -qE "Button +Back" && { ui tap "Back" >/dev/null 2>&1; sleep 3; }
screen "the projects list"

if ui tap "$ROW" >/dev/null 2>&1; then
  sleep 5
  screen "a project's chat"
else
  echo "== a project's chat =="
  echo "  could not open '$ROW' — not checked"
  MISSED="$MISSED chat"
fi

# The workers sheet. Its rows are worker names, so anything symbol-shaped
# here is chrome nobody labelled.
PILL=$(ui dump | grep -E "Button +([^ ]+, )?(Agents|Working) [0-9]+" | head -1)
if [ -z "$PILL" ]; then
  echo "== the workers sheet =="
  echo "  no workers pill on screen — not checked"
  MISSED="$MISSED sheet"
fi
if [ -n "$PILL" ]; then
  idb ui tap "$(echo "$PILL" | awk '{print $1}')" "$(echo "$PILL" | awk '{print $2}')" --udid "$UDID" >/dev/null 2>&1
  sleep 3
  screen "the workers sheet"
  idb ui swipe 196 300 196 800 --duration 0.3 --udid "$UDID" >/dev/null 2>&1
  sleep 2
fi

# Settings, which has more controls than any other screen and produced the
# first symbol name anybody noticed.
ui dump | grep -qE "Button +Back" && { ui tap "Back" >/dev/null 2>&1; sleep 3; }
if ui tap "Settings" >/dev/null 2>&1; then
  sleep 3
  screen "the settings sheet"
  ui tap "Done" >/dev/null 2>&1; sleep 2
else
  echo "== the settings sheet =="
  echo "  could not open Settings — not checked"
  MISSED="$MISSED settings"
fi

# The call, which shows almost no words by design and so rests entirely on
# the labels of its four controls.
xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 -previewCall 1 >/dev/null 2>&1
sleep 9
ui dump | grep -qE "Button +Allow" && { ui tap "Allow" >/dev/null 2>&1; sleep 4; }
screen "the call"

# Leave the app where the next run expects it. Ending inside the preview
# call sent the following steps tapping at a screen that has no composer,
# and they reported about the call while believing they were in a chat.
xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1
sleep 8
ui dump | grep -qE "Button +Back" && { ui tap "Back" >/dev/null 2>&1; sleep 2; }

echo
echo "screens looked at: $VISITED of 5"
if [ -n "$MISSED" ]; then
  echo "VERDICT: incomplete — never reached:$MISSED. A screen this did not open"
  echo "         is not a screen it cleared, whatever the rest of it found"
  exit 1
elif [ "$FOUND" = 0 ]; then
  echo "VERDICT: no control reads as a symbol name across all 5 screens"
else
  echo "VERDICT: $FOUND screen(s) carry a control named after its symbol — each is a"
  echo "         label nobody wrote, and a name a scenario must not tap"
fi

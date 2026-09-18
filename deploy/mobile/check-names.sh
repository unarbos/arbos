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
# Two or more capitalised words with no lower-case connective reads like a
# symbol spelled out ("Arrow Turning Down Then Right"), not like a label.
SYMBOLISH = re.compile(r"^(?:[A-Z][a-z0-9]*)(?: [A-Z0-9][a-z0-9]*){1,5}$")
for line in sys.stdin.read().splitlines():
    parts = line.split(None, 3)
    if len(parts) < 4:
        continue
    kind, label = parts[2], parts[3].strip()
    if kind not in ("Button", "Image", "PopUpButton"):
        continue
    if label in BAD_EXACT or (SYMBOLISH.match(label) and label not in ("Back", "Send", "Mute", "Unmute", "Filter", "Search", "Settings", "More", "Call")):
        print(f"  {kind:12} {label}")
DETECTOR
trap 'rm -f "$SUSPECT_PY"' EXIT
suspect() { python3 "$SUSPECT_PY"; }

screen() {
  local what=$1
  echo "== $what =="
  local bad
  bad=$(ui dump | suspect)
  if [ -z "$bad" ]; then
    echo "  every control here is named"
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
  " 299   85  Button       Search" | suspect | grep -c .)
if [ "$SELFTEST" != 3 ]; then
  echo "the detector failed its own self-test ($SELFTEST of 3 known-bad names caught)."
  echo "Not running: a check that cannot fail cannot pass either."
  exit 1
fi
echo "detector self-test: caught 3 of 3 known-bad names, and let 'Search' through"
echo

FOUND=0
xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 9
# The app comes back to the chat that was in front (#615), so reach the list
# deliberately rather than assuming it is first.
ui dump | grep -qE "Button +Back" && { ui tap "Back" >/dev/null 2>&1; sleep 3; }
screen "the projects list"

ui tap "$ROW" >/dev/null 2>&1 && sleep 5 && screen "a project's chat"

echo
if [ "$FOUND" = 0 ]; then
  echo "VERDICT: no control reads as a symbol name"
else
  echo "VERDICT: $FOUND screen(s) carry a control named after its symbol — each is a"
  echo "         label nobody wrote, and a name a scenario must not tap"
fi

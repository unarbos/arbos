#!/bin/bash
# COVERS: list composer → names its project, rides above the keyboard
# The composer on the projects list: whose project is it, and where does it sit?
#
#   list-composer.sh <cycle>
#
# Four claims, all measured off the accessibility tree rather than looked at:
#
#   1. at rest it names a project, and one that is on screen;
#   2. with a filter it names the project the filter left, not the last one
#      opened (#433, M-157);
#   3. with a filter matching nothing it offers no project and cannot send;
#   4. with the keyboard up it rides above it rather than under it.
#
# The fourth is the one a screenshot is worst at: "above the keyboard" is a
# comparison of two numbers, and cycle 57 got them off the tree — composer
# top 480 pt against a keyboard top of 683.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/list-composer"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }
placeholder() { ui dump | grep -E " TextField " | grep -oE "(Message [^ ]+…|Plan, ask, build…)" | head -1; }
# A row's name is the part before the first comma in its label.
rows_on_screen() { ui dump | grep -oE "Button +[a-z][a-z0-9-]*," | awk '{print $2}' | tr -d ','; }

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 9
# Every step below reads the projects list. A cold start comes back to the
# chat that was in front, so run second in the sweep and all four steps read
# a chat instead: "rows left:" empty, "composer: nothing", and step 4 gave
# "could not read both numbers — inconclusive" in every sweep for a week.
reach_the_list "$UDID" || exit 1
shot 01-at-rest

echo "--- 1. at rest ---"
AT_REST=$(placeholder)
echo "  composer:        ${AT_REST:-nothing}"
NAMED=$(echo "$AT_REST" | sed -E 's/^Message (.*)…$/\1/')
if [ "$AT_REST" = "$NAMED" ]; then
  echo "  it names no project"
elif rows_on_screen | grep -qx "$NAMED"; then
  echo "  and '$NAMED' is a row on screen"
else
  echo "  BUT '$NAMED' IS NOT A ROW ON SCREEN — the M-157 fault"
fi

echo
echo "--- 2. with a filter ---"
ui tap "Search" >/dev/null 2>&1; sleep 2
idb ui text "sub" --udid "$UDID" >/dev/null 2>&1; sleep 3
shot 02-filtered
FILTERED=$(placeholder)
LEFT=$(rows_on_screen | tr '\n' ' ')
echo "  rows left:       $LEFT"
echo "  composer:        ${FILTERED:-nothing}"
NAMED2=$(echo "$FILTERED" | sed -E 's/^Message (.*)…$/\1/')
if echo "$LEFT" | grep -qw "$NAMED2"; then
  echo "  it names one of them"
else
  echo "  IT NAMES '$NAMED2', WHICH THE FILTER LEFT OUT"
fi

echo
echo "--- 3. with a filter matching nothing ---"
for _ in 1 2 3; do idb ui key 42 --udid "$UDID" >/dev/null 2>&1; done
sleep 1
idb ui text "zzzz" --udid "$UDID" >/dev/null 2>&1; sleep 3
shot 03-no-match
NONE=$(placeholder)
echo "  composer:        ${NONE:-nothing}"
[ "$NONE" = "Plan, ask, build…" ] && echo "  it offers no project, as it should" \
                                  || echo "  IT STILL NAMES A PROJECT WITH NOTHING ON SCREEN"
ui dump | grep -qi "no project matches" && echo "  and the screen says why" || echo "  BUT THE SCREEN SAYS NOTHING"

echo
echo "--- 4. above the keyboard ---"
# The keyboard is a big element low on the screen; the composer is the
# TextField. Both tops, in points, from the same dump.
# Two TextFields are on screen with the search open — the search box near
# the top and the composer near the bottom — so take the lower one. Reading
# the first gave 177 pt, which is the search box, and made the comparison
# nonsense.
#
# The keyboard is not exposed as keys. It arrives as the GenericElement that
# covers the bottom of the screen, so its top is the number wanted.
read -r COMPOSER_Y KEY_Y <<<"$(idb ui describe-all --udid "$UDID" | python3 -c '
import json, sys
els = json.load(sys.stdin)
def tops(kind):
    return [e["frame"]["y"] for e in els
            if (e.get("type") or "") == kind and (e.get("frame") or {}).get("y") is not None]
fields, generic = tops("TextField"), tops("GenericElement")
print(round(max(fields)) if fields else "", round(min(generic)) if generic else "")
')"
echo "  composer top:    ${COMPOSER_Y:-?} pt"
echo "  keyboard top:    ${KEY_Y:-?} pt"
if [ -n "$COMPOSER_Y" ] && [ -n "$KEY_Y" ]; then
  if [ "$COMPOSER_Y" -lt "$KEY_Y" ]; then
    echo "  VERDICT: the composer sits $((KEY_Y - COMPOSER_Y)) pt above the keyboard"
  else
    echo "  VERDICT: THE KEYBOARD IS OVER THE COMPOSER by $((COMPOSER_Y - KEY_Y)) pt"
  fi
else
  echo "  VERDICT: could not read both numbers — inconclusive"
fi
echo
echo "--- 4. the same project a cold start would reopen? ---"
# Two features promise the same thing and nothing checked they agree. The
# composer names "the project in front"; a cold start reopens "the project
# that was in front" (M-338). If those two ever drift apart, the list would
# offer to message one project while the app reopens another — and each
# feature's own check would still pass.
if [ -z "${NAMED:-}" ] || [ "$NAMED" = "$AT_REST" ]; then
  echo "  the composer named no project at rest, so there is nothing to compare"
else
  xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
  xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 11
  shot 04-after-a-cold-start
  LANDED=$(ui dump | awk '$2+0 < 120 && $3 == "StaticText" { $1=""; $2=""; $3=""; sub(/^ +/, ""); print; exit }')
  echo "  the composer named:   $NAMED"
  echo "  a cold start opened:  ${LANDED:-the list}"
  if [ "$LANDED" = "$NAMED" ]; then
    echo "  VERDICT: they agree — the list offers the project the app comes back to"
  elif [ -z "$LANDED" ]; then
    echo "  VERDICT: cannot say — the cold start landed on the list, so there is no"
    echo "           project to compare the composer's name against"
  else
    echo "  VERDICT: they disagree. The list offers to message '$NAMED' and a cold"
    echo "           start opens '$LANDED'. Both features are about the project in"
    echo "           front, and each one's own check passes."
  fi
fi

echo
echo "stills in $OUT"

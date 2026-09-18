#!/bin/bash
# What the four surfaces say while a worker is actually working.
#
#   worker-while-it-works.sh <cycle> [project]
#
# Four attempts at this in cycle 61 failed for four different reasons, all
# of them the rig's: the worker finished before the sheet opened; a tap for
# the pill matched a sheet row and drilled into a worker's chat; a Back tap
# left a screen with no composer so nothing was sent. Hence:
#
#   * a `sleep` worker, so the window does not depend on the model's mood;
#   * the pill tapped by its **frame**, because the label it carries is one
#     the sheet's rows carry too;
#   * every step checked before the next, so a failure says which one.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}; ROW=${2:-phone}
OUT="$HOME/mobile-out/$CYCLE/live-worker"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; echo "$(date -u +%H:%M:%S) shot $1"; }

# The pill is a Button whose label ends "Working N" or "Agents N". Sheet rows
# end ", Done" or ", Working", so matching on the whole label tells them
# apart where a substring cannot.
pill_at() {
  idb ui describe-all --udid "$UDID" | python3 -c '
import json, re, sys
for e in json.load(sys.stdin):
    if e.get("type") != "Button":
        continue
    if re.search(r"(Working|Agents) \d+$", e.get("AXLabel") or ""):
        f = e.get("frame") or {}
        print(round(f.get("x", 0) + f.get("width", 0) / 2),
              round(f.get("y", 0) + f.get("height", 0) / 2))
        break
'
}

# Always start from a known screen: the project chat, freshly opened.
xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 >/dev/null 2>&1; sleep 8
ui tap "$ROW" >/dev/null || { echo "no $ROW row"; exit 1; }
sleep 4
ui field >/dev/null 2>&1 || { echo "no composer after opening $ROW"; exit 1; }

# The duration doubles as this run's signature. A first attempt scored on a
# worker left over from the run before it — the sheet said `sleep 150
# seconds` while this run had asked for 300 — which is M-160's fault again:
# evidence belonging to an earlier run, read as this one's.
# Start from a project with nothing running. Otherwise a leftover worker
# lights the pill within seconds, the wait below is satisfied by somebody
# else's work, and the sheet is read before this run's worker exists.
for t in $(seq 1 60); do
  ui dump | grep -qE "Working [0-9]+" || break
  [ "$t" = 1 ] && echo "  waiting for an earlier worker to finish before starting"
  sleep 10
done
if ui dump | grep -qE "Working [0-9]+"; then
  echo "a worker is still running after 10 minutes; not starting another"; exit 1
fi

NAP=$(( 280 + RANDOM % 40 ))
LINE="Through one worker you wait for: run the bash command sleep $NAP and nothing else, then reply done."
ui focus >/dev/null; sleep 0.7
idb ui text "$LINE" --udid "$UDID"
for _ in $(seq 1 80); do [ "$(ui field plain 2>/dev/null)" = "$LINE" ] && break; sleep 0.25; done
[ "$(ui field plain 2>/dev/null)" = "$LINE" ] || { echo "the line never landed in the box"; exit 1; }
ui tap "Up" >/dev/null || { echo "no send button"; exit 1; }
echo "$(date -u +%H:%M:%S) sent"

for t in $(seq 1 25); do
  sleep 4
  ui dump | grep -qE "Working [0-9]+" && { echo "  the pill reads Working after $((t*4))s"; break; }
done
shot 01-chat-while-working
echo "  the chat's worker line: $(ui dump | grep -oE '1 Working[^;]*' | head -1)"

P=$(pill_at)
[ -n "$P" ] || { echo "  no pill found to tap"; exit 1; }
echo "  tapping the pill at $P"
idb ui tap $P --udid "$UDID"; sleep 2
shot 02-sheet-while-working
# A finished row reads "<goal>, Done". A live one reads
# "<spinner>, <goal>, <step>" — no literal "Working" anywhere, because the
# spinner glyph carries that. Counting on the word printed "the sheet marks
# none of them working" with the live row plainly on screen.
# The sheet scrolls, and only rendered rows are in the tree — the same trap
# that made cycle 71 read four projects as missing when they were under the
# keyboard. So collect the rows by scrolling to the end of the list before
# saying anything about what it does or does not contain.
COLLECT=$OUT/sheet-rows.txt; : > "$COLLECT"
for page in $(seq 1 8); do
  ui dump | grep -E "Button +.+, " >> "$COLLECT"
  idb ui swipe 236 900 236 560 --duration 0.4 --udid "$UDID" >/dev/null 2>&1
  sleep 1.2
done
sort -u -k4 "$COLLECT" -o "$COLLECT"
ROWS=$(wc -l < "$COLLECT" | tr -d ' ')
LIVE=$(grep -cE "[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]|, running |Working" "$COLLECT" | tr -d ' ')
echo "  sheet rows after scrolling to the end: $ROWS   of them live: $LIVE"
grep -E "[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]|, running |Working" "$COLLECT" | sed 's/^/    live: /'
# Where the live one sits matters as much as whether it is there: a worker
# that is working, listed below a dozen finished ones, is the hardest row to
# find on the sheet that exists to show it.
MINE=$(grep -cE "[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏].*$NAP" "$COLLECT" | tr -d ' ')
if [ "$MINE" -gt 0 ]; then
  echo "  this run's own worker ($NAP s): $(grep -E "$NAP" "$COLLECT" | head -1 | sed 's/^ *//')"
elif grep -qE "[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏]" "$COLLECT"; then
  echo "  A LIVE ROW, BUT NOT THIS RUN'S — a worker from an earlier run is still going."
  echo "  Nothing below is evidence about this run. Wait for it to end and run again."
fi
if [ "$ROWS" = 0 ]; then echo "  the sheet did not open"
elif [ "$MINE" -gt 0 ]; then echo "  VERDICT: the sheet marks this run's live worker, with its step"
elif [ "$LIVE" -gt 0 ]; then echo "  VERDICT: inconclusive — the only live row belongs to an earlier run"
else echo "  VERDICT: the sheet lists $ROWS workers and marks none of them live"; fi
echo "  kernel, same moment: $(python3 "$HERE/../kernel.py" pod history 2 2>/dev/null | tail -1 | cut -c1-80)"

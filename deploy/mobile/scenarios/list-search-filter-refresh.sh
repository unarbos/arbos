#!/bin/bash
# The projects list's three ways of changing what it shows.
#
#   list-search-filter-refresh.sh <cycle>
#
# Search, the All/Live filter and pull-to-refresh were last exercised at
# cycle 47, and two of the three were read off a screenshot. All three are
# claims about which rows are present, so all three are counted here off the
# accessibility tree instead.
#
# Refresh needs the roster to change under the app, which the live hub will
# not do to order. The fixture therefore adds a project to its answer after
# the third /list, so a refresh that works shows a row that was not there
# before and one that does not, does not.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/list-search-filter-refresh"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
PORT=8793
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }
# Only the project rows: a row is `<name>, <state>…`, and the chrome
# buttons (Search, Read, Gear Shape) carry no comma.
rows() { ui dump | grep -cE "Button +[a-z][a-z0-9-]*,"; }
names() { ui dump | grep -oE "Button +[a-z][a-z0-9-]*," | awk '{print $2}' | tr -d ',' | sort | tr '\n' ' '; }

cat > /tmp/fixture-list.py <<'PY'
import http.server, json, time

PORT = 8793
NOW = int(time.time() * 1000)
CALLS = {"n": 0}

BASE = [
    {"name": "alpha", "live": True, "kind": "", "last_activity_ms": NOW - 120_000},
    {"name": "beta", "live": True, "kind": "", "last_activity_ms": NOW - 300_000},
    {"name": "beta-two", "live": False, "kind": "", "last_activity_ms": NOW - 900_000},
    {"name": "gamma", "live": False, "kind": "", "last_activity_ms": NOW - 3_600_000},
]
LATE = {"name": "arrived-late", "live": True, "kind": "", "last_activity_ms": NOW}


class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        CALLS["n"] += 1
        projects = list(BASE) + ([LATE] if CALLS["n"] > 3 else [])
        body = json.dumps({"machines": [
            {"name": "fixture-box", "online": True, "projects": projects}]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


http.server.HTTPServer(("127.0.0.1", PORT), H).serve_forever()
PY

python3 /tmp/fixture-list.py & FIX=$!
trap 'kill $FIX 2>/dev/null' EXIT
sleep 2
curl -s "http://127.0.0.1:$PORT/list" >/dev/null || { echo "the fixture hub did not start"; exit 1; }
echo "fixture up: alpha, beta, beta-two, gamma — and arrived-late from the fourth /list on"

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 -hubURL "http://127.0.0.1:$PORT" >/dev/null 2>&1
sleep 9
shot 01-the-list
echo
echo "at rest:            $(rows) rows — $(names)"

echo
echo "--- search ---"
ui tap "Search" >/dev/null || echo "  no Search button"
sleep 2
idb ui text "beta" >/dev/null 2>&1
sleep 3
shot 02-search-beta
echo "typing 'beta':      $(rows) rows — $(names)"

# Undo the query a character at a time: the field has no clear button, and
# retyping from a stale caret is how cycle 48's typing went wrong.
for _ in 1 2 3 4; do idb ui key 42 >/dev/null 2>&1; done
sleep 2
idb ui text "zzzz" >/dev/null 2>&1
sleep 3
shot 03-search-nothing
echo "typing 'zzzz':      $(rows) rows"
if ui dump | grep -qi "no project matches"; then
  echo "  and it says so: $(ui dump | grep -oiE "No project matches.*" | head -1)"
else
  echo "  BUT IT SAYS NOTHING — a blank screen under the search box (M-191 regressed)"
fi

for _ in 1 2 3 4; do idb ui key 42 >/dev/null 2>&1; done
sleep 3
echo "cleared:            $(rows) rows — $(names)"
ui tap "Search" >/dev/null 2>&1
sleep 2

echo
echo "--- the All / Live only filter ---"
ui menu >/dev/null 2>&1 || echo "  no filter button"
sleep 2
shot 04-the-filter-menu
if ui dump | grep -q "Live only"; then
  ui tap "Live only" >/dev/null 2>&1
  sleep 3
  shot 05-live-only
  echo "Live only:          $(rows) rows — $(names)"
  echo "  (the fixture serves two live: alpha, beta)"
  ui menu >/dev/null 2>&1; sleep 2
  ui tap "All projects" >/dev/null 2>&1; sleep 3
  echo "All projects:       $(rows) rows — $(names)"
else
  echo "  the menu did not open — it is a PopUpButton with no label"
fi

echo
echo "--- pull to refresh ---"
BEFORE=$(names)
# A long, slow drag from just under the header: a flick is a scroll.
idb ui swipe 196 300 196 760 --duration 1.2 >/dev/null 2>&1
sleep 6
shot 06-after-refresh
AFTER=$(names)
echo "before:             $BEFORE"
echo "after:              $AFTER"
if [ "$BEFORE" = "$AFTER" ]; then
  echo "  VERDICT: the refresh brought nothing new — either the gesture missed or the list does not reload"
elif echo "$AFTER" | grep -q "arrived-late"; then
  echo "  VERDICT: the new project arrived on the refresh"
else
  echo "  VERDICT: the list changed, but not by gaining arrived-late"
fi
echo
echo "still in $OUT"

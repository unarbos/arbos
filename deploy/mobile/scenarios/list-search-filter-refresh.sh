#!/bin/bash
# COVERS: projects list — search, filter, refresh
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
#
# TWO RULES FOR COUNTING ROWS HERE, both learned the hard way.
#
# The list is a `LazyVStack`, so a row below the fold is never built and
# `describe-all` cannot see it. A count is therefore a count of *rendered*
# rows, not of the list — sound only while every row fits the screen. Raising
# the keyboard is enough to break it: a first run of this scenario read 11
# rows at rest and 7 after clearing the search, and the four that "vanished"
# were simply under the keyboard.
#
# And the app remembers every project it has ever opened (M-176), so
# yesterday's fixtures are still rows today. The app is reinstalled first, or
# the counts are of this run plus every run before it.
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
# buttons (Search, Read, Settings) carry no comma.
rows() { ui dump | grep -cE "Button +[a-z][a-z0-9-]*,"; }
names() { ui dump | grep -oE "Button +[a-z][a-z0-9-]*," | awk '{print $2}' | tr -d ',' | sort | tr '\n' ' '; }

cat > /tmp/fixture-list.py <<'PY'
import http.server, json, os, time

PORT = 8793
NOW = int(time.time() * 1000)
LOG = "/tmp/fixture-list-calls.log"
# The scenario touches this the moment before it pulls to refresh, so the
# new project appears exactly then. Counting /list calls instead would make
# the answer depend on how many the app happened to make first — a first
# run added the project on the fourth call, a second run never reached four,
# and the two runs disagreed about whether refresh works.
TRIGGER = "/tmp/fixture-add-late"

BASE = [
    {"name": "alpha", "live": True, "kind": "", "last_activity_ms": NOW - 120_000},
    {"name": "beta", "live": True, "kind": "", "last_activity_ms": NOW - 300_000},
    {"name": "beta-two", "live": False, "kind": "", "last_activity_ms": NOW - 900_000},
    {"name": "gamma", "live": False, "kind": "", "last_activity_ms": NOW - 3_600_000},
]
LATE = {"name": "arrived-late", "live": True, "kind": "", "last_activity_ms": NOW}


class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        # Log what was *answered*, not only that something was asked. The
        # count alone cannot tell "the app asked again before the fixture
        # began serving the new project" from "it was served and the list
        # did not redraw" — and the two runs of cycle 118 disagreed for
        # exactly that reason, with no way to say which had happened.
        late = os.path.exists(TRIGGER)
        with open(LOG, "a") as f:
            f.write(f"{time.time():.3f} {self.path} late={int(late)}\n")
        projects = list(BASE) + ([LATE] if late else [])
        body = json.dumps({"machines": [
            {"name": "fixture-box", "online": True, "projects": projects}]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


open(LOG, "w").close()
http.server.HTTPServer(("127.0.0.1", PORT), H).serve_forever()
PY

# The trigger is a file, and a file outlives the run that made it. Left
# behind by an earlier cycle it makes the fixture serve the "new" project
# from the very first call, so it is on screen before the pull and the
# refresh verdict congratulates itself on a row that was always there.
rm -f /tmp/fixture-add-late
python3 /tmp/fixture-list.py & FIX=$!
trap 'kill $FIX 2>/dev/null' EXIT
sleep 2
curl -s "http://127.0.0.1:$PORT/list" >/dev/null || { echo "the fixture hub did not start"; exit 1; }
echo "fixture up: alpha, beta, beta-two, gamma — and arrived-late once the pull asks for it"

# The app to reinstall is the one the loop just built, not a path some cycle
# left in /tmp. This read /tmp/dd/Build/Products/..., a build from the
# previous day, so every scenario after this one in the sweep measured
# yesterday's app — which is where the separator that six cycles chased kept
# coming from (M-498, M-503). Proven: fresh build reads "phone, Idle, home",
# this scenario runs, and the next read is "phone, Idle, · , home" with the
# binary dated a day earlier.
DERIVED=${DERIVED:-$HOME/mobile-derived/Build/Products/Debug-iphonesimulator/Arbos.app}
APP=${APP:-$DERIVED}
xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
if [ -d "$APP" ]; then
  xcrun simctl uninstall "$UDID" $B >/dev/null 2>&1
  xcrun simctl install "$UDID" "$APP" >/dev/null 2>&1
  echo "reinstalled, so the list starts with no remembered projects"
else
  echo "NOTE: $APP is not there, so old remembered rows are still in the list"
fi
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
sleep 2
# Count with the keyboard down: see the note at the top about LazyVStack.
ui tap "Search" >/dev/null 2>&1
sleep 3
echo "cleared:            $(rows) rows — $(names)"

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
CALLS_BEFORE=$(wc -l < /tmp/fixture-list-calls.log)
# From here the fixture serves a fifth project, so anything the app asks for
# after this moment carries it.
touch /tmp/fixture-add-late
# A long, slow drag from just under the header: a flick is a scroll.
idb ui swipe 196 300 196 760 --duration 1.2 >/dev/null 2>&1
sleep 6
shot 06-after-refresh
AFTER=$(names)
CALLS_AFTER=$(wc -l < /tmp/fixture-list-calls.log)
echo "before:             $BEFORE"
echo "after:              $AFTER"
echo "/list calls:        $CALLS_BEFORE before the pull, $CALLS_AFTER after"
# Two separate questions, and the old version could not tell them apart:
# did the gesture make the app ask again, and did the answer reach the screen?
# Two separate things: did the app ask again, and did the answer change the
# screen. The second only means something if the project was absent before.
case " $BEFORE " in
  *" arrived-late "*) echo "  NOTE: arrived-late was already on screen before the pull — the"
                      echo "        'new project' half of this says nothing this run";;
esac
# Of the calls after the pull, how many were answered with the new project?
SERVED=$(tail -n +$((CALLS_BEFORE + 1)) /tmp/fixture-list-calls.log | grep -c "late=1")
echo "answers carrying it:  $SERVED of $((CALLS_AFTER - CALLS_BEFORE)) calls since the pull"
if [ "$CALLS_AFTER" -le "$CALLS_BEFORE" ]; then
  echo "  VERDICT: the app never asked again — the gesture missed, or the pull does not refetch"
elif echo "$AFTER" | grep -q "arrived-late"; then
  echo "  VERDICT: the pull refetched and the new project reached the screen"
elif [ "$SERVED" = 0 ]; then
  echo "  VERDICT: cannot say — the app asked again, but every answer since the pull"
  echo "           was sent before the fixture began serving the new project. The"
  echo "           request was already in flight; this run tests nothing."
else
  echo "  VERDICT: the app asked again, was answered with the new project $SERVED time(s),"
  echo "           and did not draw it — the list is not redrawing the answer"
fi
echo
echo "still in $OUT"

#!/bin/bash
# COVERS: projects list — faces, rows, sections
#
# What does a person see before there is anything to see?
#
#   nothing-to-show.sh <cycle>
#
# The empty list is the first screen of a new install and nothing has ever
# driven it. `ProjectsView.emptyState` has three things to say — "Asking the
# hub…" while it loads, the hub's problem if there is one, and "No projects
# yet." otherwise — and which of them appears, and what surrounds it, has
# never been looked at.
#
# It is served by a fixture hub answering with no projects at all, the same
# way list-search-filter-refresh serves its four. The app is reinstalled
# first: remembered rows from real runs would otherwise fill a list this
# check needs empty, and the screen would be about yesterday.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
. "$HERE/../sim-lib.sh"
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/nothing"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
PORT=8795
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }

cat > /tmp/fixture-empty.py <<'PY'
import http.server, json

PORT = 8795


class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        # A hub that is answering perfectly well and simply has nothing on
        # it. Not an error, not a refusal: the case of a new install.
        body = json.dumps({"machines": []}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


http.server.HTTPServer(("127.0.0.1", PORT), H).serve_forever()
PY

python3 /tmp/fixture-empty.py & FIX=$!
trap 'kill $FIX 2>/dev/null' EXIT
sleep 2
curl -s "http://127.0.0.1:$PORT/list" >/dev/null || { echo "the fixture hub did not start"; exit 1; }
echo "a hub with nothing on it is answering on $PORT"

# A reinstall, or the rows this check needs gone are still there from real
# runs and the screen is about yesterday.
APP=${APP:-$HOME/mobile-derived/Build/Products/Debug-iphonesimulator/Arbos.app}
[ -d "$APP" ] || { echo "no app at $APP — refusing to run against whatever is installed"; exit 1; }
xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl uninstall "$UDID" $B >/dev/null 2>&1
xcrun simctl install "$UDID" "$APP" >/dev/null 2>&1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 -hubURL "http://127.0.0.1:$PORT" >/dev/null 2>&1
sleep 10
shot 01-nothing-to-show

DUMP=$(ui dump)
ROWS=$(echo "$DUMP" | grep -cE "Button +[a-z][a-z0-9-]*,")
SAYS=$(echo "$DUMP" | grep -E "StaticText" | awk '{ $1=""; $2=""; $3=""; sub(/^ +/, ""); print }' | grep -vE "^(Projects|Read|Working)$" | head -3 | tr '\n' '|')
CHROME=$(echo "$DUMP" | grep -oE "Button +(Settings|Search|Filter)" | awk '{print $2}' | sort -u | tr '\n' ' ')
COMPOSER=$(echo "$DUMP" | grep -oE "Message [^ ]+…|Plan, ask, build…|Message …" | head -1)

echo "  project rows:      $ROWS"
echo "  what it says:      ${SAYS:-nothing}"
echo "  chrome offered:    ${CHROME:-none}"
echo "  the composer says: ${COMPOSER:-nothing}"

echo
if [ "$ROWS" != 0 ]; then
  echo "VERDICT: cannot say — $ROWS row(s) are on the list, so this is not the empty"
  echo "         case. The reinstall did not clear what an earlier run remembered."
elif echo "$SAYS" | grep -q "No projects yet"; then
  echo "VERDICT: an empty hub says 'No projects yet.' — the plain case, said plainly"
elif echo "$SAYS" | grep -qi "asking the hub"; then
  echo "VERDICT: still 'Asking the hub…' after ten seconds, against a fixture that"
  echo "         answers at once — the list is waiting for something that arrived"
else
  echo "VERDICT: an empty hub shows '${SAYS:-nothing}', which is neither the loading"
  echo "         line nor 'No projects yet.' Read the still before deciding which"
  echo "         it should have been."
fi
echo "stills in $OUT"

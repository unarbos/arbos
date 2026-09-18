#!/bin/bash
# What the list says about a machine the hub is holding open (#545).
#
#   sleeping-machine.sh <cycle>
#
# The live roster has no sleeping machine, and stopping somebody else's
# kernels to make one is not a test worth running. So the app is pointed at
# a small local hub that serves a fixture: one machine awake, one asleep
# with its projects `live: false` and an `offline_since_ms`.
#
# This tests the phone, not the hub — the fixture is exactly the shape #545
# documents, checked against the live roster's own fields first.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/sleeping-machine"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
PORT=8791
. "$HERE/../sim-lib.sh"
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }

cat > /tmp/fixture-hub.py <<'PY'
import http.server, json, time
NOW = int(time.time() * 1000)
ROSTER = {"machines": [
    {"name": "awake-box", "online": True, "projects": [
        {"name": "alpha", "live": True, "kind": "", "last_activity_ms": NOW - 120_000}]},
    {"name": "sleepy-box", "online": False, "offline_since_ms": NOW - 3_600_000, "projects": [
        {"name": "beta", "live": False, "kind": ""},
        {"name": "gamma", "live": False, "kind": ""}]},
]}
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps(ROSTER).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", 8791), H).serve_forever()
PY
python3 /tmp/fixture-hub.py & FIX=$!
trap 'kill $FIX 2>/dev/null' EXIT
sleep 2
curl -s "http://127.0.0.1:$PORT/list" >/dev/null || { echo "the fixture hub did not start"; exit 1; }
echo "fixture hub up: one machine awake, one asleep for an hour"

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 -hubURL "http://127.0.0.1:$PORT" >/dev/null 2>&1
sleep 9
# Every line below reads the projects list, and a cold start comes back to
# the chat that was in front.
reach_the_list "$UDID" || exit 1
xcrun simctl io "$UDID" screenshot "$OUT/01-sleeping-machine.png" >/dev/null 2>&1
echo "what the list says:"
ui dump | grep -E "Button +[a-z0-9-]+," | sed 's/^/  /'
echo
if ui dump | grep -qi "sleepy-box is asleep"; then
  echo "VERDICT: a sleeping machine's projects say so by name"
else
  echo "VERDICT: the sleeping machine's rows do not name it — they read:"
  ui dump | grep -E "Button +(beta|gamma)," | sed 's/^/    /'
fi
echo "still in $OUT"

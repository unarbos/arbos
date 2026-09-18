#!/bin/bash
# A refusal and a transport failure are opposite cases. Does the phone say so?
#
#   refusal-and-transport.sh <cycle>
#
# Two things must be true at once, and cycle 49 could only show one of them
# (M-163, M-165 — the refusal half was never driven on a device):
#
#   a refusal is final    — the hub has answered; say its reason and stop.
#   a transport failure is not — nobody answered; say what broke and retry.
#
# Handled the wrong way round, the app retries for ever on an answer that
# will never change, and gives up on the one case that usually clears by
# itself.
#
# The live hub cannot produce either on demand, so the app is pointed at a
# small fixture. Its refusals are copied verbatim from `Hub::kernel` in
# `crates/arbos-hub/src/hub.rs` — all three of them, plus a 502 at the
# tunnel. Verbatim matters more than it sounds: the app recognises these by
# shape, and a paraphrased fixture tests the paraphrase. A first draft of
# this file put the project before the machine and produced a confident
# report of an app bug that did not exist.
#
# What is counted, rather than looked at: how many times the app comes back
# to each path. Retrying and stopping are claims about behaviour over time,
# and a screenshot cannot tell them apart.
set -uo pipefail
export PATH="/opt/homebrew/bin:$HOME/Library/Python/3.14/bin:$PATH"
HERE=$(cd "$(dirname "$0")" && pwd)
CYCLE=${1:?cycle}
OUT="$HOME/mobile-out/$CYCLE/refusal-and-transport"; mkdir -p "$OUT"
UDID=$(xcrun simctl list devices booted -j | python3 -c 'import json,sys;print(next(d["udid"] for v in json.load(sys.stdin)["devices"].values() for d in v))')
B=com.unarbos.arbos.ios
PORT=8792
LOG=/tmp/fixture-attach.log
ui() { python3 "$HERE/../ui.py" "$UDID" "$@"; }
shot() { xcrun simctl io "$UDID" screenshot "$OUT/$1.png" >/dev/null 2>&1; }

cat > /tmp/fixture-refusals.py <<'PY'
import base64, hashlib, json, socket, socketserver, sys, threading, time

PORT, LOG = 8792, "/tmp/fixture-attach.log"
GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
NOW = int(time.time() * 1000)

ROSTER = {"machines": [{"name": "fixture-box", "online": True, "projects": [
    {"name": "no-machine", "live": True, "kind": "", "last_activity_ms": NOW - 60_000},
    {"name": "machine-offline", "live": True, "kind": "", "last_activity_ms": NOW - 60_000},
    {"name": "no-kernel", "live": True, "kind": "", "last_activity_ms": NOW - 60_000},
    {"name": "bad-tunnel", "live": True, "kind": "", "last_activity_ms": NOW - 60_000},
]}]}

# Verbatim from `Hub::kernel`, trailing detail and all, because the trailing
# detail is part of what a person is shown when the app does not recognise
# the shape.
REFUSALS = {
    "no-machine": 'no machine named "ghost-box" is registered (known: fixture-box)',
    "machine-offline": 'fixture-box is offline: nothing of it has been connected since '
                       '2026-09-18T02:14:03Z; it served "machine-offline" — start a kernel '
                       'there and it comes back',
    "no-kernel": 'fixture-box has no kernel serving "no-kernel" (live: alpha); it has a '
                 'worker, so a claim can start one',
}

log_lock = threading.Lock()


def note(path):
    with log_lock:
        with open(LOG, "a") as f:
            f.write(f"{time.time():.3f} {path}\n")


def ws_frame(opcode, payload):
    n = len(payload)
    head = bytes([0x80 | opcode])
    if n < 126:
        head += bytes([n])
    else:
        head += bytes([126, n >> 8 & 0xFF, n & 0xFF])
    return head + payload


class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        line = self.rfile.readline().decode("latin-1").strip()
        if not line:
            return
        try:
            _, path, _ = line.split(" ", 2)
        except ValueError:
            return
        headers = {}
        while True:
            h = self.rfile.readline().decode("latin-1").strip()
            if not h:
                break
            if ":" in h:
                k, v = h.split(":", 1)
                headers[k.strip().lower()] = v.strip()

        if path.rstrip("/").endswith("/list") or path.rstrip("/") == "":
            body = json.dumps(ROSTER).encode()
            self.wfile.write(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"
                             b"Content-Length: " + str(len(body)).encode() + b"\r\n\r\n" + body)
            return

        if not path.startswith("/attach/"):
            self.wfile.write(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
            return

        project = path.rstrip("/").split("/")[-1]
        note(project)

        # A tunnel that is not there answers before any WebSocket exists. This
        # is the case the app must retry, and the one it used to give up on.
        if project == "bad-tunnel":
            body = b"Bad Gateway"
            self.wfile.write(b"HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain\r\n"
                             b"Content-Length: " + str(len(body)).encode() + b"\r\n\r\n" + body)
            return

        key = headers.get("sec-websocket-key")
        if not key:
            self.wfile.write(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
            return
        accept = base64.b64encode(hashlib.sha1((key + GUID).encode()).digest()).decode()
        self.wfile.write(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n"
                         b"Connection: Upgrade\r\nSec-WebSocket-Accept: " + accept.encode() + b"\r\n\r\n")
        self.wfile.flush()

        # `Hub::attach` sends the reason in one error frame, `detail` prefixed
        # with "hub: ", and then `refuse_close` waits for the peer and closes
        # with no code and no reason at all. The reason therefore exists in
        # exactly one place on the wire. A draft of this fixture put it in the
        # close frame instead and read the app as losing reasons it was never
        # sent — and the long one overflowed the 125-byte limit a control
        # frame has, which the app correctly saw as a broken socket.
        reason = REFUSALS.get(project, "refused")
        frame = {"type": "error", "agent": None, "detail": f"hub: {reason}"}
        self.wfile.write(ws_frame(0x1, json.dumps(frame).encode()))
        self.wfile.flush()
        time.sleep(0.3)
        self.wfile.write(ws_frame(0x8, b""))
        self.wfile.flush()
        time.sleep(0.2)


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


open(LOG, "w").close()
Server(("127.0.0.1", PORT), Handler).serve_forever()
PY

python3 /tmp/fixture-refusals.py & FIX=$!
trap 'kill $FIX 2>/dev/null' EXIT
sleep 2
curl -s "http://127.0.0.1:$PORT/list" >/dev/null || { echo "the fixture hub did not start"; exit 1; }
echo "fixture hub up on $PORT: two refusals with reasons, one 502 tunnel"

xcrun simctl terminate "$UDID" $B 2>/dev/null; sleep 1
xcrun simctl launch "$UDID" $B -noAskNotifications 1 -hubURL "http://127.0.0.1:$PORT" >/dev/null 2>&1
sleep 9
# A cold start comes back to the chat that was in front (M-338, cycle 100),
# so the app no longer opens on the list. Without this the four probes below
# tapped names that were not on screen, each said "no <row> row", and the
# run still printed its closing paragraph as though it had measured the
# pair. Reach the list deliberately.
ui dump | grep -qE "Button +Back" && { ui tap "Back" >/dev/null 2>&1; sleep 3; }
shot 01-the-fixture-list
echo "the list:"
ui dump | grep -E "Button +[a-z-]+," | sed 's/^/  /'
SEEN=$(ui dump | grep -cE "Button +(no-machine|machine-offline|no-kernel|bad-tunnel),")
if [ "$SEEN" -lt 4 ]; then
  echo
  echo "only $SEEN of the fixture's 4 rows are on screen — the app is not"
  echo "looking at the fixture hub, so nothing below would be about it."
  echo "Stopping rather than reporting on four rows that are not there."
  exit 1
fi

# Each case gets the same treatment: open it, wait a fixed window, read what
# the app says, and count how often it came back during that window.
probe() {
  local row=$1 name=$2 window=$3
  echo
  echo "=== $row ==="
  local start
  start=$(python3 -c 'import time;print(f"{time.time():.3f}")')
  ui tap "$row" >/dev/null || { echo "  no $row row"; MISSED=$((MISSED + 1)); return; }
  sleep "$window"
  shot "$name"
  echo "  what the app says:"
  ui dump | grep -viE "Button|Image|^ *[0-9]+ +[0-9]+ +(Other|Application)" \
          | grep -oE "StaticText +.*" | cut -c1-140 | sed 's/^/    /' | head -12
  local hits
  hits=$(python3 - "$start" "$row" <<'PY'
import sys
start, row = float(sys.argv[1]), sys.argv[2]
n = 0
for line in open("/tmp/fixture-attach.log"):
    t, p = line.split()
    if float(t) >= start and p == row:
        n += 1
print(n)
PY
)
  echo "  attach attempts in ${window}s: $hits"
  ui tap "Back" >/dev/null 2>&1 || ui back >/dev/null 2>&1 || true
  sleep 2
}

MISSED=0
probe no-machine 02-no-machine-of-that-name 25
probe machine-offline 03-the-machine-is-offline 25
probe no-kernel 04-has-no-kernel-serving 25
probe bad-tunnel 05-a-transport-failure 25

echo
echo "--- the whole attach log ---"
sort "$LOG" | awk '{print $2}' | uniq -c | sed 's/^/  /'
echo
if [ "$MISSED" -gt 0 ]; then
  echo "VERDICT: none — $MISSED of 4 cases never opened, so this run says nothing"
  echo "         about how the phone handles a refusal or a transport failure"
else
  echo "A refusal should be counted once and left alone; the 502 should be"
  echo "counted several times. Any other shape is the pair handled backwards."
fi
echo "still in $OUT"

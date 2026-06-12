#!/usr/bin/env bash
# arbos dev launcher — build once, run the server, get out of the way.
#
# There is no file watcher and no restart supervisor here. The server watches
# its OWN executable (engine.WatchRestart): whenever the file at $BIN is
# replaced by a successful build, the running process re-execs the new binary
# at its next IDLE turn boundary — never mid-turn, same PID, sessions intact.
# That makes in-place self-editing safe by construction, whether the edit
# comes from a terminal or from inside a running arbos turn.
#
# To ship a change while the server runs:
#   go build -o /tmp/arbos-dev ./cmd/arbos    # backend: hot-swaps at idle
#   (cd web && npm run build)                 # frontend: then refresh browser
#
# A broken tree never reaches the server: a failed compile writes no binary,
# so the last good build keeps serving.
#
# Usage: scripts/dev.sh            (port 8499; ARBOS_PORT overrides)
# Stop:  ctrl-c
#
# Single-instance: ONE canonical server per machine. A second launch exits
# immediately instead of fighting the first for the port and the Telegram
# bot's long poll.
set -euo pipefail
cd "$(dirname "$0")/.."

PORT="${ARBOS_PORT:-8499}"
BIN=/tmp/arbos-dev # outside the repo so builds never churn git status

LOCKDIR=/tmp/arbos-dev.lock
if ! mkdir "$LOCKDIR" 2>/dev/null; then
  other=$(cat "$LOCKDIR/pid" 2>/dev/null || true)
  if [ -n "$other" ] && kill -0 "$other" 2>/dev/null; then
    echo "dev: already running (pid $other) — this copy exits"
    exit 0
  fi
  rm -rf "$LOCKDIR" && mkdir "$LOCKDIR" # stale lock from a dead server
fi
echo $$ >"$LOCKDIR/pid"

echo "dev: building…"
go generate ./internal/tool/coding
go build -o "$BIN" ./cmd/arbos
(cd web && npm run build >/dev/null 2>&1)

echo "dev: serving on http://localhost:$PORT — rebuild $BIN in place to hot-swap"
# exec keeps this script's PID as the server's ancestor recorded in the lock;
# the stale-lock check above reclaims it once the server is gone.
exec doppler run -p arbos -c dev -- "$BIN" --web ":$PORT" --web-dist web/dist

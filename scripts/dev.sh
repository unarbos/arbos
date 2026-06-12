#!/usr/bin/env bash
# arbos dev loop — keeps localhost running the LATEST code, always.
#
# Three processes, one contract:
#   1. vite build --watch   — every web/src change rewrites web/dist; the
#      gateway serves the UI from disk (--web-dist), so a browser refresh
#      is all it takes. No Go rebuild for frontend work.
#   2. a server supervisor  — reruns the binary whenever the process dies
#      (the watcher kills it after a successful rebuild; a crash restarts
#      it with the same binary). Sessions live in the durable sqlite
#      store, and the web UI auto-reconnects its seam, so a restart costs
#      a dropped in-flight turn at most.
#   3. watchexec on Go code — any .go/go.mod change regenerates tool
#      schemas and rebuilds; ONLY a successful build signals a restart, by
#      touching /tmp/arbos-dev.pending. The server re-execs the new binary
#      at its next IDLE turn boundary (never mid-turn), so a self-edit from
#      inside a running turn can never decapitate that turn. A broken tree
#      leaves the last good binary serving while the compiler errors print
#      here, and no sentinel is written.
#
# Usage: scripts/dev.sh            (port 8499; ARBOS_PORT overrides)
# Stop:  ctrl-c (takes all three processes down)
#
# Single-instance: there is ONE canonical dev loop per machine. A second
# launch (a stray terminal, a respawn loop) exits immediately instead of
# fighting the first for the port and the Telegram bot's long poll.
set -euo pipefail
cd "$(dirname "$0")/.."

PORT="${ARBOS_PORT:-8499}"
BIN=/tmp/arbos-dev # outside the repo so builds never churn the watchers

LOCKDIR=/tmp/arbos-dev.lock
if ! mkdir "$LOCKDIR" 2>/dev/null; then
  other=$(cat "$LOCKDIR/pid" 2>/dev/null || true)
  if [ -n "$other" ] && kill -0 "$other" 2>/dev/null; then
    echo "dev: already running (pid $other) — this copy exits"
    exit 0
  fi
  rm -rf "$LOCKDIR" && mkdir "$LOCKDIR" # stale lock from a dead loop
fi
echo $$ >"$LOCKDIR/pid"

cleanup() {
  # Stop the loops FIRST (vite, supervisor, watcher — they're our jobs),
  # then the server they tend; otherwise the supervisor respawns it.
  trap - EXIT INT TERM
  # shellcheck disable=SC2046
  kill $(jobs -p) 2>/dev/null || true
  pkill -f "/tmp/arbos-dev[ ]--web" 2>/dev/null || true
  rm -rf "$LOCKDIR"
}
trap cleanup EXIT INT TERM

echo "dev: initial build…"
go generate ./internal/tool/coding
go build -o "$BIN" ./cmd/arbos
(cd web && npm run build >/dev/null 2>&1)

(cd web && exec npx vite build --watch --logLevel warn) &

(
  while true; do
    ARBOS_RESTART_SENTINEL=/tmp/arbos-dev.pending \
      doppler run -p arbos -c dev -- "$BIN" --web ":$PORT" --web-dist web/dist || true
    sleep 1
  done
) &

echo "dev: watching Go sources (server on http://localhost:$PORT)"
# No exec: the trap above must survive to tear everything down on exit.
# --shell=none: run bash -c directly instead of wrapping it in another shell.
watchexec --postpone --shell=none -e go,mod,sum -w cmd -w internal -w go.mod -w go.sum -- bash -c '
  echo "dev: change detected — rebuilding…" &&
  go generate ./internal/tool/coding &&
  go build -o /tmp/arbos-dev.new ./cmd/arbos &&
  mv /tmp/arbos-dev.new /tmp/arbos-dev &&
  touch /tmp/arbos-dev.pending &&
  echo "dev: rebuilt — restart pending (server re-execs at its next idle turn boundary)" ||
  echo "dev: BUILD FAILED — old server still running"
'

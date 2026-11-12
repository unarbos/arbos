#!/usr/bin/env bash
# Start the Arbos relay with a public URL that boxes and phones can dial back.
#
#   ARBOS_TUNNEL=named  scripts/relay-start.sh     # stable URL (recommended for persistent projects)
#   ARBOS_TUNNEL=quick  scripts/relay-start.sh     # dev only: random *.trycloudflare.com, new on every run
#   ARBOS_TUNNEL=none   ARBOS_RELAY_PUBLIC_URL=https://relay.example scripts/relay-start.sh
#
# named: runs `cloudflared tunnel run --token $ARBOS_TUNNEL_TOKEN` beside the relay. The token comes
#        from the env or from 1Password (`op read op://Arbos/<item>/credential`); the hostname the
#        tunnel serves is ARBOS_TUNNEL_HOSTNAME (default relay.arbos.life). The URL never changes, so
#        boxes created earlier keep dialing back after a restart.
# quick: runs `cloudflared tunnel --url`, reads the generated URL, exports it. Existing boxes get the
#        new URL pushed at their next wake (relay-url file), but a stable URL is the right fix.
# none:  you already have a reverse proxy or a reachable IP; ARBOS_RELAY_PUBLIC_URL is required.
#
# Model provider keys are read from the env (OPENROUTER_API_KEY, OPENAI_API_KEY) or, when `op` is on
# the PATH and ARBOS_OP_KEYS=1, from the Arbos vault. Everything else is the relay's normal env
# (README).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${ARBOS_TUNNEL:-named}"
LISTEN="${ARBOS_RELAY_LISTEN:-127.0.0.1:7700}"
PORT="${LISTEN##*:}"
RELAY_BIN="${ARBOS_RELAY_BIN:-}"
LOG_DIR="${ARBOS_RELAY_LOG_DIR:-${ARBOS_RELAY_DATA:-$ROOT/.arbos-local/relay}/logs}"
mkdir -p "$LOG_DIR"

if [[ -z "$RELAY_BIN" ]]; then
  if [[ -x "$ROOT/target/release/arbos-relay" ]]; then
    RELAY_BIN="$ROOT/target/release/arbos-relay"
  else
    (cd "$ROOT" && cargo build --release -p arbos-relay)
    RELAY_BIN="$ROOT/target/release/arbos-relay"
  fi
fi
command -v cloudflared >/dev/null 2>&1 || CLOUDFLARED="${ARBOS_CLOUDFLARED:-cloudflared}"
CLOUDFLARED="${CLOUDFLARED:-cloudflared}"

PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do [[ -n "$p" ]] && kill "$p" 2>/dev/null || true; done
}
trap cleanup EXIT INT TERM

case "$MODE" in
  named)
    HOST="${ARBOS_TUNNEL_HOSTNAME:-relay.arbos.life}"
    if [[ -z "${ARBOS_TUNNEL_TOKEN:-}" ]] && command -v op >/dev/null 2>&1; then
      ARBOS_TUNNEL_TOKEN="$(op read "op://Arbos/${ARBOS_TUNNEL_OP_ITEM:-km3t5533nywq2ugvp7dg45bsiu}/credential")"
    fi
    : "${ARBOS_TUNNEL_TOKEN:?ARBOS_TUNNEL_TOKEN (or op access to the Arbos vault) is required for ARBOS_TUNNEL=named}"
    export ARBOS_RELAY_PUBLIC_URL="https://$HOST"
    echo "==> named tunnel → $ARBOS_RELAY_PUBLIC_URL (origin http://127.0.0.1:$PORT)"
    "$CLOUDFLARED" tunnel --no-autoupdate run --token "$ARBOS_TUNNEL_TOKEN" >"$LOG_DIR/cloudflared.log" 2>&1 &
    PIDS+=($!)
    ;;
  quick)
    echo "==> quick tunnel (dev only; the URL changes on every start)"
    "$CLOUDFLARED" tunnel --no-autoupdate --url "http://127.0.0.1:$PORT" >"$LOG_DIR/cloudflared.log" 2>&1 &
    PIDS+=($!)
    URL=""
    for _ in $(seq 1 30); do
      URL="$(grep -o 'https://[a-z0-9-]*\.trycloudflare\.com' "$LOG_DIR/cloudflared.log" | head -1 || true)"
      [[ -n "$URL" ]] && break
      sleep 1
    done
    [[ -n "$URL" ]] || { echo "relay-start: quick tunnel gave no URL (see $LOG_DIR/cloudflared.log)" >&2; exit 1; }
    export ARBOS_RELAY_PUBLIC_URL="$URL"
    echo "==> $ARBOS_RELAY_PUBLIC_URL"
    ;;
  none)
    : "${ARBOS_RELAY_PUBLIC_URL:?ARBOS_RELAY_PUBLIC_URL is required for ARBOS_TUNNEL=none}"
    ;;
  *)
    echo "relay-start: ARBOS_TUNNEL must be named, quick, or none (got $MODE)" >&2; exit 2;;
esac

if [[ "${ARBOS_OP_KEYS:-0}" == "1" ]] && command -v op >/dev/null 2>&1; then
  export OPENROUTER_API_KEY="${OPENROUTER_API_KEY:-$(op read "op://Arbos/${ARBOS_OP_OPENROUTER_ITEM:?set ARBOS_OP_OPENROUTER_ITEM}/credential")}"
fi

# Stale exports from earlier sessions must not leak into boxes (snapshot and harness overrides).
unset ARBOS_DEFAULT_HARNESS ARBOS_BOOTSTRAP_DIR
export ARBOS_RELAY_LISTEN="$LISTEN"
echo "==> relay on $LISTEN, public $ARBOS_RELAY_PUBLIC_URL, snapshot ${ARBOS_DAYTONA_SNAPSHOT:-<relay default>}"
exec "$RELAY_BIN"

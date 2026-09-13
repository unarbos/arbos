#!/usr/bin/env bash
# Keep the stack alive and publish the public URL. Runs forever; stack.sh starts it in tmux.
#
#   VOICE_HOME=/root/arbos-voice deploy/supervise.sh
#
# Every 15 s: bring back any tmux session that died (stack.sh up is idempotent), restart the
# gateway if /healthz fails three times, restart the quick tunnel if its public URL stops
# answering, and when the public URL changes write it to $VOICE_HOME/public-url.txt and run
# deploy/publish-url.sh (which pushes it to the repo's qa-results branch when ARBOS_GITHUB is set).
set -uo pipefail
SRC="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "${VOICE_HOME:-}" ]; then
  case "$(basename "$SRC")" in src) VOICE_HOME="$(dirname "$SRC")" ;; *) VOICE_HOME="$SRC" ;; esac
fi
export VOICE_HOME
[ -f "$VOICE_HOME/env" ] && set -a && . "$VOICE_HOME/env" && set +a
PORT="${VOICE_PORT:-8765}"
INTERVAL="${SUPERVISE_INTERVAL:-15}"
log() { echo "$(date -u +%FT%TZ) $*"; }

gateway_fails=0; quick_fails=0; nim_fails=0; phone_fails=0; kquick_fails=0; tick=0
last_url="$(cat "$VOICE_HOME/public-url.txt" 2>/dev/null || true)"
last_kurl="$(cat "$VOICE_HOME/kernel-url.txt" 2>/dev/null || true)"
last_hurl="$(cat "$VOICE_HOME/hub-url.txt" 2>/dev/null || true)"

current_quick_url() {
  grep -o 'https://[a-z0-9-]*\.trycloudflare\.com' "$VOICE_HOME/logs/quick.log" 2>/dev/null | tail -1
}

while true; do
  # 1. dead sessions come back
  "$SRC/deploy/stack.sh" up 2>/dev/null | grep -v "already running" | sed 's/^/stack: /' || true

  # 2. gateway health
  if curl -sf -m 5 -o /dev/null "http://127.0.0.1:$PORT/healthz"; then
    gateway_fails=0
  else
    gateway_fails=$((gateway_fails + 1))
    log "gateway healthz failed ($gateway_fails)"
    if [ "$gateway_fails" -ge 3 ]; then
      log "restarting gateway"; tmux kill-session -t "=voice" 2>/dev/null; gateway_fails=0
    fi
  fi

  # 3. duplex model container (only when this host runs it)
  if tmux has-session -t "=nim" 2>/dev/null; then
    if curl -sf -m 5 "http://127.0.0.1:9000/v1/realtime/health" | grep -q '"status":"ok"'; then
      nim_fails=0
    else
      nim_fails=$((nim_fails + 1))
      if [ "$nim_fails" -ge 40 ]; then  # ten minutes: model load takes up to five
        log "restarting duplex container"; docker rm -f nemotron-labs-voicechat >/dev/null 2>&1
        tmux kill-session -t "=nim" 2>/dev/null; nim_fails=0
      fi
    fi
  fi

  # 4. quick tunnel reachability and URL publishing
  if tmux has-session -t "=quick" 2>/dev/null; then
    url="$(current_quick_url)"
    if [ -n "$url" ]; then
      if curl -sf -m 10 -o /dev/null "$url/healthz"; then
        quick_fails=0
        if [ "$url" != "$last_url" ]; then
          echo "$url" > "$VOICE_HOME/public-url.txt"
          last_url="$url"
          log "public url is now $url"
          [ -x "$SRC/deploy/publish-url.sh" ] && "$SRC/deploy/publish-url.sh" "$url" "$last_kurl" "$last_hurl" 2>&1 | sed 's/^/publish: /'
        fi
      else
        quick_fails=$((quick_fails + 1))
        log "quick tunnel $url not answering ($quick_fails)"
        if [ "$quick_fails" -ge 3 ]; then
          log "restarting quick tunnel"; : > "$VOICE_HOME/logs/quick.log"
          tmux kill-session -t "=quick" 2>/dev/null; quick_fails=0
        fi
      fi
    fi
  fi

  # 5. phone kernel (port check; it speaks WebSocket/NDJSON, not HTTP) and its quick tunnel
  if tmux has-session -t "=phone" 2>/dev/null; then
    if (exec 3<>/dev/tcp/127.0.0.1/"${PHONE_PORT:-7788}") 2>/dev/null; then
      phone_fails=0
    else
      phone_fails=$((phone_fails + 1))
      log "phone kernel port closed ($phone_fails)"
      if [ "$phone_fails" -ge 3 ]; then log "restarting phone kernel"; tmux kill-session -t "=phone" 2>/dev/null; phone_fails=0; fi
    fi
  fi
  tick=$((tick + 1))
  # every fourth pass: the probe shows up as one refused attach in the kernel log
  if [ $((tick % 4)) -eq 1 ] && tmux has-session -t "=kquick" 2>/dev/null; then
    kurl="$(grep -o 'https://[a-z0-9-]*\.trycloudflare\.com' "$VOICE_HOME/logs/kquick.log" 2>/dev/null | tail -1)"
    if [ -n "$kurl" ]; then
      # Any HTTP answer means the tunnel reaches the pod; the kernel refuses a bare GET but still answers.
      if curl -s -m 10 -o /dev/null -w '%{http_code}' "$kurl/" | grep -qE '^[1-5]'; then
        kquick_fails=0
        if [ "$kurl" != "$last_kurl" ]; then
          echo "$kurl" > "$VOICE_HOME/kernel-url.txt"; last_kurl="$kurl"
          log "kernel public url is now $kurl"
          [ -x "$SRC/deploy/publish-url.sh" ] && "$SRC/deploy/publish-url.sh" "$last_url" "$kurl" "$last_hurl" 2>&1 | sed 's/^/publish: /'
        fi
      else
        kquick_fails=$((kquick_fails + 1))
        log "kernel quick tunnel $kurl not answering ($kquick_fails)"
        if [ "$kquick_fails" -ge 3 ]; then
          log "restarting kernel quick tunnel"; : > "$VOICE_HOME/logs/kquick.log"
          tmux kill-session -t "=kquick" 2>/dev/null; kquick_fails=0
        fi
      fi
    fi
  fi

  # 6. mesh hub URL (another service on this host writes it); republish when it changes
  hub_file="${HUB_URL_FILE:-/root/arbos-hub/public-url.txt}"
  if [ -s "$hub_file" ]; then
    hurl="$(head -1 "$hub_file" | tr -d '[:space:]')"
    if [ -n "$hurl" ] && [ "$hurl" != "$last_hurl" ]; then
      echo "$hurl" > "$VOICE_HOME/hub-url.txt"; last_hurl="$hurl"
      log "hub public url is now $hurl"
      [ -x "$SRC/deploy/publish-url.sh" ] && "$SRC/deploy/publish-url.sh" "$last_url" "$last_kurl" "$hurl" 2>&1 | sed 's/^/publish: /'
    fi
  fi

  sleep "$INTERVAL"
done

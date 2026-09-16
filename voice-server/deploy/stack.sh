#!/usr/bin/env bash
# Run the whole voice stack from one directory, each part in its own tmux session:
#   kernel   arbos-kernel serve $VOICE_HOME/kernel/place        (if bin/arbos-kernel exists)
#   voice    deploy/run.sh --kernel-place ... $VOICE_ARGS        (the gateway, port 8765)
#   tunnel   cloudflared named tunnel (tunnel.token) or a quick *.trycloudflare.com tunnel
#   nim      NemotronLabs VoiceChat container (if nim/ holds the Triton model repo)
#   supervisor  deploy/supervise.sh: restarts what dies, publishes the public URL (VOICE_SUPERVISE=0 to skip)
#   phone    token-guarded kernel for the phone app, as user $PHONE_USER (set PHONE_HOME in env; see below)
#   kquick   quick tunnel for the phone kernel
#
#   VOICE_HOME=/root/arbos-voice deploy/stack.sh up|down|status|logs
#
# $VOICE_HOME/env (chmod 600) holds: OPENROUTER_API_KEY=..., VOICE_TOKEN=..., VOICE_ARGS="..."
#   and either PHONE_HOME=... (run the phone kernel here) or VOICE_KERNEL_URL=wss://... + VOICE_KERNEL_TOKEN=...
#   (attach to a kernel elsewhere). With neither, bin/arbos-kernel serves a local place.
set -euo pipefail
SRC="$(cd "$(dirname "$0")/.." && pwd)"
# Deployed layout is $VOICE_HOME/src/<this repo dir>; a bare checkout is its own home.
if [ -z "${VOICE_HOME:-}" ]; then
  case "$(basename "$SRC")" in src) VOICE_HOME="$(dirname "$SRC")" ;; *) VOICE_HOME="$SRC" ;; esac
fi
[ -f "$VOICE_HOME/env" ] && set -a && . "$VOICE_HOME/env" && set +a
export VOICE_HOME
PORT="${VOICE_PORT:-8765}"
NIM_DIR="$VOICE_HOME/nim/nemotron-labs-voicechat_v1.0.0"

tm() { tmux "$@"; }
start() {  # name, command  (a running tmux server does not inherit our env, so re-source it inside)
  tm has-session -t "=$1" 2>/dev/null && { echo "$1: already running"; return; }
  local prelude="export VOICE_HOME='$VOICE_HOME'; [ -f '$VOICE_HOME/env' ] && set -a && . '$VOICE_HOME/env' && set +a;"
  tm new-session -d -s "$1" -c "$VOICE_HOME" -- bash -lc "$prelude $2 2>&1 | tee -a $VOICE_HOME/logs/$1.log"
  echo "$1: started"
}

case "${1:-status}" in
  up)
    mkdir -p "$VOICE_HOME/logs" "$VOICE_HOME/kernel/place" "$VOICE_HOME/kernel/home"
    if [ -d "$NIM_DIR" ] && command -v docker >/dev/null; then
      start nim "docker run --rm --name=nemotron-labs-voicechat --runtime=nvidia --gpus all --shm-size=8GB \
        -e NIM_HTTP_API_PORT=9000 -p 127.0.0.1:9000:9000 -v $(readlink -f "$NIM_DIR"):/data/models \
        --entrypoint /s2s/run_s2s_server.sh nvcr.io/nim/nvidia/nemotron-labs-voicechat:latest"
    fi
    # Phone kernel: a token-guarded `arbos-kernel serve --bind` run as its own Unix user so a runaway
    # job can only hurt that user. $PHONE_HOME holds bin/arbos-kernel, env (OPENROUTER_API_KEY,
    # ARBOS_PHONE_TOKEN), xdg/arbos/config.toml, homedir/, and home/ (the project folder) with
    # home/.arbos/access.toml ([[client]] name/token_env/role). The voice gateway attaches to it too.
    if [ -n "${VOICE_KERNEL_URL:-}" ]; then
      # Remote kernel (e.g. wss://kernel-api.arbos.life behind its tunnel); VOICE_KERNEL_TOKEN in env.
      KERNEL_ARGS="--kernel $VOICE_KERNEL_URL"
    elif [ -n "${PHONE_HOME:-}" ] && [ -x "$PHONE_HOME/bin/arbos-kernel" ]; then
      PHONE_PORT="${PHONE_PORT:-7788}"
      start phone "runuser -u ${PHONE_USER:-arbos-phone} -- bash -c 'set -a; . $PHONE_HOME/env; set +a; \
        cd $PHONE_HOME/home && XDG_CONFIG_HOME=$PHONE_HOME/xdg HOME=$PHONE_HOME/homedir \
        exec $PHONE_HOME/bin/arbos-kernel serve $PHONE_HOME/home --bind 127.0.0.1:$PHONE_PORT'"
      start kquick "$VOICE_HOME/bin/cloudflared tunnel --no-autoupdate --url http://127.0.0.1:$PHONE_PORT"
      KERNEL_ARGS="--kernel tcp://127.0.0.1:$PHONE_PORT"
      for _ in $(seq 1 40); do (exec 3<>/dev/tcp/127.0.0.1/$PHONE_PORT) 2>/dev/null && break; sleep 0.5; done
    elif [ -x "$VOICE_HOME/bin/arbos-kernel" ]; then
      start kernel "cd $VOICE_HOME/kernel/place && XDG_CONFIG_HOME=$VOICE_HOME/kernel/xdg HOME=$VOICE_HOME/kernel/home \
        $VOICE_HOME/bin/arbos-kernel serve $VOICE_HOME/kernel/place"
      KERNEL_ARGS="--kernel-place $VOICE_HOME/kernel/place"
      for _ in $(seq 1 40); do [ -f "$VOICE_HOME/kernel/place/.arbos/kernel.json" ] && break; sleep 0.5; done
    else
      KERNEL_ARGS=""
    fi
    start voice "cd $SRC && $SRC/deploy/run.sh --host 127.0.0.1 --port $PORT $KERNEL_ARGS ${VOICE_ARGS:-}"
    if [ -s "$VOICE_HOME/tunnel.token" ]; then
      start tunnel "$VOICE_HOME/bin/cloudflared tunnel --no-autoupdate run --token \$(cat $VOICE_HOME/tunnel.token)"
    fi
    # A quick tunnel gives a throwaway https://*.trycloudflare.com URL: the interim endpoint until DNS for
    # the named tunnel exists, or the whole endpoint when there is no Cloudflare account.
    if [ ! -s "$VOICE_HOME/tunnel.token" ] || [ "${VOICE_QUICK_TUNNEL:-0}" = "1" ]; then
      start quick "$VOICE_HOME/bin/cloudflared tunnel --no-autoupdate --url http://127.0.0.1:$PORT"
    fi
    [ "${VOICE_SUPERVISE:-1}" = "1" ] && start supervisor "$SRC/deploy/supervise.sh"
    ;;
  down)
    for s in supervisor quick kquick tunnel voice kernel phone; do tm kill-session -t "=$s" 2>/dev/null && echo "$s: stopped" || true; done
    [ "${2:-}" = "--nim" ] && { docker stop nemotron-labs-voicechat 2>/dev/null; tm kill-session -t "=nim" 2>/dev/null; echo "nim: stopped"; } || true
    ;;
  status)
    tm ls 2>/dev/null || echo "nothing running"
    curl -s -o /dev/null -w "gateway /healthz: %{http_code}\n" "http://127.0.0.1:$PORT/healthz" || true
    curl -s "http://127.0.0.1:9000/v1/realtime/health" 2>/dev/null | head -c 200 || true; echo
    [ -f "$VOICE_HOME/public-url.txt" ] && echo "voice public url: $(cat "$VOICE_HOME/public-url.txt")"
    [ -f "$VOICE_HOME/kernel-url.txt" ] && echo "kernel public url: $(cat "$VOICE_HOME/kernel-url.txt")"
    ;;
  logs)
    tail -n "${2:-30}" "$VOICE_HOME"/logs/{voice,kernel,phone,tunnel,quick,kquick,supervisor}.log 2>/dev/null
    ;;
  *) echo "usage: $0 up|down [--nim]|status|logs [n]"; exit 2 ;;
esac

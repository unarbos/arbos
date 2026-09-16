#!/usr/bin/env bash
# Record display :1 to an mp4 file.
#
#   rec.sh start <app> <name>   -> begins $MEDIA/<app>/<name>.mp4, prints the pid
#   rec.sh stop                 -> stops the recording started last
#
# Uses ffmpeg with x11grab (the X11 screen-capture input) at 15 frames per
# second. 15 fps is enough to see a status animation and keeps files small.
set -euo pipefail
source "$(dirname "$0")/env.sh"

pidfile=/tmp/parity-rec.pid

case "${1:-}" in
  start)
    app="$2"; name="$3"
    out="$MEDIA/$app/$name.mp4"
    ffmpeg -loglevel error -y -f x11grab -framerate 15 -video_size "$SCREEN" \
      -i "$DISPLAY" -c:v libx264 -preset veryfast -pix_fmt yuv420p "$out" \
      </dev/null >/tmp/parity-rec.log 2>&1 &
    echo $! > "$pidfile"
    echo "$out" > /tmp/parity-rec.out
    log "recording -> $out (pid $!)"
    ;;
  stop)
    if [ -f "$pidfile" ]; then
      # SIGINT lets ffmpeg write the mp4 index; SIGKILL would leave a broken file.
      kill -INT "$(cat "$pidfile")" 2>/dev/null || true
      sleep 2
      rm -f "$pidfile"
      ls -la "$(cat /tmp/parity-rec.out)" >&2
      cat /tmp/parity-rec.out
    fi
    ;;
  *) echo "usage: rec.sh start <app> <name> | rec.sh stop" >&2; exit 2 ;;
esac

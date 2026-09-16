#!/usr/bin/env bash
# Install (if needed) and launch Cursor desktop on display :1 with $PROJ open.
#
#   cursor-launch.sh            -> Cursor window up; log at /tmp/cursor-app.log
#
# Steps:
#   1. Ask Cursor's update API for the current stable Linux AppImage URL.
#   2. Download it and unpack it (no FUSE needed when unpacked).
#   3. Start it inside a tmux session so it outlives this script.
#   4. Dismiss the "OS keyring" dialog by choosing "Use weaker encryption".
set -euo pipefail
source "$(dirname "$0")/env.sh"

mkdir -p "$CURSOR_DIR" "$PROJ"
cd "$CURSOR_DIR"

if [ ! -x squashfs-root/AppRun ]; then
  url="$(curl -sL 'https://api2.cursor.sh/updates/api/download/stable/linux-x64/cursor' \
        | python3 -c 'import json,sys; print(json.load(sys.stdin)["downloadUrl"])')"
  log "downloading $url"
  curl -sL -o Cursor.AppImage "$url"
  chmod +x Cursor.AppImage
  ./Cursor.AppImage --appimage-extract >/dev/null
fi

# Seed the sample project once. Same files as arbos-launch.sh uses.
bash "$PARITY/seed-project.sh"

S=cursor-app
tmux -f /exec-daemon/tmux.portal.conf has-session -t "=$S" 2>/dev/null \
  || tmux -f /exec-daemon/tmux.portal.conf new-session -d -s "$S" -c "$CURSOR_DIR" -- bash -l
tmux -f /exec-daemon/tmux.portal.conf send-keys -t "$S:0.0" \
  "export DISPLAY=$DISPLAY; ./squashfs-root/AppRun --no-sandbox --disable-gpu-sandbox '$PROJ' 2>&1 | tee /tmp/cursor-app.log" C-m

# Wait for the keyring dialog, then pick "Use weaker encryption".
for _ in $(seq 1 30); do
  sleep 1
  if id="$(win '^Cursor$')" && [ -n "$id" ]; then
    xdotool windowactivate --sync "$id"
    # Button sits right of centre in the dialog; coordinates are for 1920x1200.
    xdotool mousemove 1119 661 click 1
    break
  fi
done
sleep 3
wmctrl -l
log "Cursor is up. Log in state: see cursor-login-link.sh"

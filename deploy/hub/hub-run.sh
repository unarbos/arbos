#!/bin/bash
# Keep arbos-hub running. Layout: <dir>/bin/arbos-hub, <dir>/hub-server.toml,
# <dir>/logs/. Run under tmux, systemd, or launchd.
dir=${ARBOS_HUB_DIR:-$HOME/arbos-hub}
cd "$dir" || exit 1
mkdir -p logs
chmod 600 hub-server.toml
while true; do
  echo "$(date -u +%FT%TZ) starting arbos-hub" >> logs/hub.log
  ./bin/arbos-hub --config "$dir/hub-server.toml" --bind "${ARBOS_HUB_BIND:-127.0.0.1:7010}" >> logs/hub.log 2>&1
  echo "$(date -u +%FT%TZ) arbos-hub exited $?" >> logs/hub.log
  sleep 2
done

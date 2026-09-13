#!/bin/bash
# Interim public address for the hub: a Cloudflare quick tunnel (no account,
# URL changes on every restart). Writes the wss:// URL to <dir>/public-url.txt.
# For a stable name use a named tunnel with an ingress rule such as
#   hub-api.example.com -> http://127.0.0.1:7010
# plus a proxied CNAME to <tunnel-id>.cfargotunnel.com.
dir=${ARBOS_HUB_DIR:-$HOME/arbos-hub}
cd "$dir" || exit 1
mkdir -p logs
while true; do
  echo "$(date -u +%FT%TZ) starting quick tunnel" >> logs/quick.log
  cloudflared tunnel --url "http://${ARBOS_HUB_BIND:-127.0.0.1:7010}" --no-autoupdate 2>&1 | while read -r line; do
    echo "$line" >> logs/quick.log
    url=$(echo "$line" | grep -o 'https://[a-z0-9-]*\.trycloudflare\.com' | head -1)
    if [ -n "$url" ]; then
      echo "${url/https:/wss:}" > public-url.txt
      echo "$(date -u +%FT%TZ) public url $url" >> logs/quick.log
    fi
  done
  sleep 3
done

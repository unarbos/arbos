#!/usr/bin/env bash
# Create (or reuse) a named Cloudflare Tunnel that fronts the voice server, point a
# hostname at it, and print the connector token to $VOICE_HOME/tunnel.token.
#
#   CLOUDFLARE_API_TOKEN=... CLOUDFLARE_ACCOUNT_ID=... \
#   deploy/cloudflare-tunnel.sh voice.arbos.life [tunnel-name] [local-port]
#
# Then, on the machine that runs the server (cloudflared binary lands in $VOICE_HOME/bin):
#   $VOICE_HOME/bin/cloudflared tunnel run --token "$(cat $VOICE_HOME/tunnel.token)"
# Idempotent: re-running updates the ingress and DNS record in place.
set -euo pipefail
: "${CLOUDFLARE_API_TOKEN:?}" "${CLOUDFLARE_ACCOUNT_ID:?}"
HOSTNAME="${1:?hostname, e.g. voice.arbos.life}"
NAME="${2:-arbos-voice}"
PORT="${3:-8765}"
VOICE_HOME="${VOICE_HOME:-$(cd "$(dirname "$0")/.." && pwd)}"
API="https://api.cloudflare.com/client/v4"
ZONE_NAME="$(echo "$HOSTNAME" | awk -F. '{print $(NF-1)"."$NF}')"

cf() { curl -sS -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" -H "Content-Type: application/json" "$@"; }
jqr() { python3 -c "import json,sys; d=json.load(sys.stdin); assert d.get('success'), d.get('errors'); print($1)"; }

TUNNEL_ID="$(cf "$API/accounts/$CLOUDFLARE_ACCOUNT_ID/cfd_tunnel?name=$NAME&is_deleted=false" \
  | jqr "next((t['id'] for t in d['result']), '')")"
if [ -z "$TUNNEL_ID" ]; then
  TUNNEL_ID="$(cf -X POST "$API/accounts/$CLOUDFLARE_ACCOUNT_ID/cfd_tunnel" \
    --data "{\"name\":\"$NAME\",\"config_src\":\"cloudflare\"}" | jqr "d['result']['id']")"
  echo "created tunnel $NAME ($TUNNEL_ID)" >&2
else
  echo "reusing tunnel $NAME ($TUNNEL_ID)" >&2
fi

cf -X PUT "$API/accounts/$CLOUDFLARE_ACCOUNT_ID/cfd_tunnel/$TUNNEL_ID/configurations" --data "{
  \"config\": {\"ingress\": [
    {\"hostname\": \"$HOSTNAME\", \"service\": \"http://127.0.0.1:$PORT\",
     \"originRequest\": {\"noTLSVerify\": true, \"connectTimeout\": 30}},
    {\"service\": \"http_status:404\"}
  ]}}" | jqr "'ingress set'" >&2

ZONE_ID="$(cf "$API/zones?name=$ZONE_NAME" | jqr "d['result'][0]['id']")"
RECORD_ID="$(cf "$API/zones/$ZONE_ID/dns_records?type=CNAME&name=$HOSTNAME" | jqr "next((r['id'] for r in d['result']), '')")"
BODY="{\"type\":\"CNAME\",\"name\":\"$HOSTNAME\",\"content\":\"$TUNNEL_ID.cfargotunnel.com\",\"proxied\":true,\"ttl\":1}"
if [ -z "$RECORD_ID" ]; then
  cf -X POST "$API/zones/$ZONE_ID/dns_records" --data "$BODY" | jqr "'dns created'" >&2
else
  cf -X PUT "$API/zones/$ZONE_ID/dns_records/$RECORD_ID" --data "$BODY" | jqr "'dns updated'" >&2
fi

mkdir -p "$VOICE_HOME/bin"
cf "$API/accounts/$CLOUDFLARE_ACCOUNT_ID/cfd_tunnel/$TUNNEL_ID/token" | jqr "d['result']" > "$VOICE_HOME/tunnel.token"
chmod 600 "$VOICE_HOME/tunnel.token"
if [ ! -x "$VOICE_HOME/bin/cloudflared" ]; then
  curl -sSL -o "$VOICE_HOME/bin/cloudflared" \
    "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/')"
  chmod +x "$VOICE_HOME/bin/cloudflared"
fi
echo "wss://$HOSTNAME/ws  -> http://127.0.0.1:$PORT via tunnel $TUNNEL_ID; token in $VOICE_HOME/tunnel.token"

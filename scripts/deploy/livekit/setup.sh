#!/usr/bin/env bash
# Stand up (or update) the LiveKit SFU for Arbos calls on the relay host. Idempotent; run as root:
#
#   sudo -E scripts/deploy/livekit/setup.sh                 # keys from /etc/livekit/livekit.env, the vault, or freshly generated
#   sudo -E scripts/deploy/livekit/setup.sh --print-env     # show the LIVEKIT_* lines the relay needs
#
# What it does (docs/self-host.md "Calls (LiveKit)"):
#   /etc/livekit/livekit.env          hostnames, ports and the API key pair (root only)
#   /etc/livekit/livekit.yaml         rendered from livekit.yaml.tmpl
#   /etc/livekit/docker-compose.yml   livekit/livekit-server, host networking, restart policy, log rotation
#   /etc/livekit/certs/               TURN/TLS certificate copied from Caddy by arbos-livekit-sync-cert(.timer)
#   /etc/arbos/sites.d/livekit.caddy  vhosts for wss://<LIVEKIT_HOST> and the TURN hostname (imported by the :443 Caddy)
#   livekit.service                   systemd wrapper around docker compose (After=docker)
#   ufw                               7881/tcp, 3478/udp, 5349/tcp, UDP media range
#   /etc/arbos/relay.env              LIVEKIT_URL / LIVEKIT_API_KEY / LIVEKIT_API_SECRET appended when missing (no restart)
#
# Secrets: with OP_SERVICE_ACCOUNT_TOKEN set, the key pair is read from the vault item "Arbos LiveKit"
# (fields LIVEKIT_API_KEY / LIVEKIT_API_SECRET); otherwise an existing /etc/livekit/livekit.env wins;
# otherwise a new pair is generated and printed once — store it in the vault yourself.
set -euo pipefail
[[ $EUID -eq 0 ]] || { echo "run as root (sudo -E)" >&2; exit 1; }
REPO_DIR="$(cd "$(dirname "$0")/../../.." && pwd)"
HERE="$REPO_DIR/scripts/deploy/livekit"

LIVEKIT_DIR=/etc/livekit
ENV_FILE=$LIVEKIT_DIR/livekit.env
SITES_D=/etc/arbos/sites.d
WWW_USER="${WWW_USER:-${SUDO_USER:-const}}"
WWW_DIR="${WWW_DIR:-/home/$WWW_USER/arbos-www}"
RELAY_ENV=/etc/arbos/relay.env
PRINT_ONLY=0
[[ "${1:-}" == "--print-env" ]] && PRINT_ONLY=1

# ---- settings (env overrides; existing livekit.env is the baseline) ---------------------------------
# the host's livekit.env, outside the repo
# shellcheck disable=SC1090
[[ -f "$ENV_FILE" ]] && . "$ENV_FILE"
LIVEKIT_HOST="${LIVEKIT_HOST:-livekit.arbos.life}"
TURN_DOMAIN="${TURN_DOMAIN:-turn.arbos.life}"
WEBHOOK_URL="${WEBHOOK_URL:-https://relay.arbos.life/livekit/webhook}"
RTC_PORT_START="${RTC_PORT_START:-50000}"
RTC_PORT_END="${RTC_PORT_END:-50200}"
CADDY_DATA="${CADDY_DATA:-$WWW_DIR/caddy-data}"
NODE_IP="${NODE_IP:-$(curl -fsS -4 --max-time 5 https://ifconfig.me || true)}"
[[ -n "$NODE_IP" ]] || { echo "cannot determine the public IP; set NODE_IP" >&2; exit 1; }

if [[ $PRINT_ONLY -eq 1 ]]; then
  : "${LIVEKIT_API_KEY:?no key yet; run the setup first}"
  printf 'LIVEKIT_URL=wss://%s\nLIVEKIT_API_KEY=%s\nLIVEKIT_API_SECRET=%s\n' "$LIVEKIT_HOST" "$LIVEKIT_API_KEY" "$LIVEKIT_API_SECRET"
  exit 0
fi

# ---- API key pair -----------------------------------------------------------------------------------
if [[ -z "${LIVEKIT_API_KEY:-}" || -z "${LIVEKIT_API_SECRET:-}" ]] && command -v op >/dev/null 2>&1 && [[ -n "${OP_SERVICE_ACCOUNT_TOKEN:-}" ]]; then
  # under sudo -E HOME is the invoking user's; op needs a config dir it owns
  LIVEKIT_API_KEY=$(HOME=/root XDG_CONFIG_HOME=/root/.config op read 'op://Arbos/ypyfs6fm7dtkzphirv6ep7lfjq/LIVEKIT_API_KEY' 2>/dev/null </dev/null || true)
  LIVEKIT_API_SECRET=$(HOME=/root XDG_CONFIG_HOME=/root/.config op read 'op://Arbos/ypyfs6fm7dtkzphirv6ep7lfjq/LIVEKIT_API_SECRET' 2>/dev/null </dev/null || true)
fi
GENERATED=0
if [[ -z "${LIVEKIT_API_KEY:-}" || -z "${LIVEKIT_API_SECRET:-}" ]]; then
  LIVEKIT_API_KEY="API$(openssl rand -base64 18 | tr -dc 'A-Za-z0-9' | head -c 14)"
  LIVEKIT_API_SECRET="$(openssl rand -base64 48 | tr -dc 'A-Za-z0-9' | head -c 44)"
  GENERATED=1
fi

say() { printf '\n==> %s\n' "$*"; }

# ---- 1. /etc/livekit -----------------------------------------------------------------------------
say "config"
install -d -m 0750 "$LIVEKIT_DIR"
umask 077
cat > "$ENV_FILE" <<EOF
# Arbos LiveKit — written by scripts/deploy/livekit/setup.sh on $(date -u +%FT%TZ). root only.
LIVEKIT_HOST=$LIVEKIT_HOST
TURN_DOMAIN=$TURN_DOMAIN
WEBHOOK_URL=$WEBHOOK_URL
NODE_IP=$NODE_IP
RTC_PORT_START=$RTC_PORT_START
RTC_PORT_END=$RTC_PORT_END
CADDY_DATA=$CADDY_DATA
LIVEKIT_API_KEY=$LIVEKIT_API_KEY
LIVEKIT_API_SECRET=$LIVEKIT_API_SECRET
EOF
umask 022
sed -e "s#@API_KEY@#$LIVEKIT_API_KEY#g" -e "s#@API_SECRET@#$LIVEKIT_API_SECRET#g" \
    -e "s#@TURN_DOMAIN@#$TURN_DOMAIN#g" -e "s#@WEBHOOK_URL@#$WEBHOOK_URL#g" -e "s#@NODE_IP@#$NODE_IP#g" \
    -e "s#@RTC_PORT_START@#$RTC_PORT_START#g" -e "s#@RTC_PORT_END@#$RTC_PORT_END#g" \
    "$HERE/livekit.yaml.tmpl" > "$LIVEKIT_DIR/livekit.yaml"
chmod 0640 "$LIVEKIT_DIR/livekit.yaml"
install -m 0644 "$HERE/docker-compose.yml" "$LIVEKIT_DIR/docker-compose.yml"
echo "wrote $LIVEKIT_DIR/{livekit.env,livekit.yaml,docker-compose.yml}"

# ---- 2. Caddy vhosts (certificates for both names) -------------------------------------------------
say "caddy"
install -d -m 0755 "$SITES_D"
sed -e "s#@LIVEKIT_HOST@#$LIVEKIT_HOST#g" -e "s#@TURN_HOST@#$TURN_DOMAIN#g" "$HERE/Caddyfile.livekit" > "$SITES_D/livekit.caddy"
if ! "$WWW_DIR/bin/caddy" list-modules 2>/dev/null | grep -q "^layer4$"; then
  echo "WARNING: $WWW_DIR/bin/caddy has no layer4 module — TURN/TLS on 443 will not work; run scripts/deploy/livekit/build-caddy.sh and restart arbos-www" >&2
fi
if ! grep -q "sites.d" "$WWW_DIR/Caddyfile" 2>/dev/null; then
  echo "WARNING: $WWW_DIR/Caddyfile does not import $SITES_D/*.caddy — redeploy the relay (scripts/deploy/relay-server.sh) first" >&2
fi
CADDY_BIN="$WWW_DIR/bin/caddy"; [[ -x "$CADDY_BIN" ]] || CADDY_BIN=$(command -v caddy)
"$CADDY_BIN" validate --config "$WWW_DIR/Caddyfile" --adapter caddyfile >/dev/null
systemctl reload arbos-www.service
echo "caddy reloaded with $SITES_D/livekit.caddy"

# ---- 3. TURN certificate from Caddy ---------------------------------------------------------------
say "turn certificate"
install -m 0755 "$HERE/sync-turn-cert.sh" /usr/local/bin/arbos-livekit-sync-cert
cat > /etc/systemd/system/arbos-livekit-sync-cert.service <<EOF
[Unit]
Description=Copy Caddy's $TURN_DOMAIN certificate into /etc/livekit/certs (restarts LiveKit on change)
[Service]
Type=oneshot
ExecStart=/usr/local/bin/arbos-livekit-sync-cert
EOF
cat > /etc/systemd/system/arbos-livekit-sync-cert.timer <<EOF
[Unit]
Description=Daily TURN certificate sync for LiveKit
[Timer]
OnCalendar=daily
RandomizedDelaySec=1h
Persistent=true
[Install]
WantedBy=timers.target
EOF
# Caddy issues the certificate on first use of the vhost; poke it and wait a little.
for _ in $(seq 1 30); do
  curl -fsS -o /dev/null --max-time 5 "https://$TURN_DOMAIN/" 2>/dev/null || true
  if find "$CADDY_DATA/certificates" -type f -name "$TURN_DOMAIN.crt" 2>/dev/null | grep -q .; then break; fi
  sleep 3
done
/usr/local/bin/arbos-livekit-sync-cert
[[ -f "$LIVEKIT_DIR/certs/$TURN_DOMAIN.crt" ]] || { echo "no certificate for $TURN_DOMAIN; is the A record DNS-only and pointing here?" >&2; exit 1; }

# ---- 4. firewall ----------------------------------------------------------------------------------
say "firewall"
if command -v ufw >/dev/null 2>&1 && ufw status | grep -q "Status: active"; then
  ufw allow 7881/tcp comment 'livekit ice/tcp' >/dev/null
  ufw allow 3478/udp comment 'livekit turn/udp' >/dev/null
  ufw allow 5349/tcp comment 'livekit turn/tls' >/dev/null
  ufw allow "$RTC_PORT_START:$RTC_PORT_END/udp" comment 'livekit media' >/dev/null
  echo "ufw: 7881/tcp 3478/udp 5349/tcp $RTC_PORT_START-$RTC_PORT_END/udp open (7880 stays Caddy-only)"
else
  echo "ufw not active; open 7881/tcp 3478/udp 5349/tcp $RTC_PORT_START-$RTC_PORT_END/udp on your firewall"
fi

# ---- 5. systemd + docker --------------------------------------------------------------------------
say "livekit.service"
cat > /etc/systemd/system/livekit.service <<EOF
[Unit]
Description=LiveKit SFU for Arbos calls (docker compose)
Requires=docker.service
After=docker.service network-online.target
Wants=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes
WorkingDirectory=$LIVEKIT_DIR
ExecStart=/usr/bin/docker compose -f $LIVEKIT_DIR/docker-compose.yml up -d --remove-orphans
ExecReload=/usr/bin/docker compose -f $LIVEKIT_DIR/docker-compose.yml restart
ExecStop=/usr/bin/docker compose -f $LIVEKIT_DIR/docker-compose.yml down
TimeoutStartSec=300

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable --now arbos-livekit-sync-cert.timer >/dev/null
docker compose -f "$LIVEKIT_DIR/docker-compose.yml" pull -q
systemctl enable livekit.service >/dev/null
if systemctl is-active --quiet livekit.service; then
  docker compose -f "$LIVEKIT_DIR/docker-compose.yml" up -d --force-recreate >/dev/null
else
  systemctl start livekit.service
fi
for _ in $(seq 1 30); do curl -fsS --max-time 2 http://127.0.0.1:7880/ >/dev/null 2>&1 && break; sleep 1; done
curl -fsS --max-time 2 http://127.0.0.1:7880/ >/dev/null && echo "livekit answering on 127.0.0.1:7880" || { echo "livekit not answering; docker logs livekit" >&2; docker logs --tail 50 livekit >&2; exit 1; }

# ---- 6. relay env ----------------------------------------------------------------------------------
say "relay env"
if [[ -f "$RELAY_ENV" ]]; then
  for kv in "LIVEKIT_URL=wss://$LIVEKIT_HOST" "LIVEKIT_API_KEY=$LIVEKIT_API_KEY" "LIVEKIT_API_SECRET=$LIVEKIT_API_SECRET"; do
    k=${kv%%=*}
    if grep -q "^$k=" "$RELAY_ENV"; then sed -i "s|^$k=.*|$kv|" "$RELAY_ENV"; else echo "$kv" >> "$RELAY_ENV"; fi
  done
  echo "LIVEKIT_URL/LIVEKIT_API_KEY/LIVEKIT_API_SECRET set in $RELAY_ENV (the relay reads them on its next restart)"
else
  echo "no $RELAY_ENV here; give the relay: LIVEKIT_URL=wss://$LIVEKIT_HOST LIVEKIT_API_KEY=… LIVEKIT_API_SECRET=…"
fi

say "done"
echo "signaling: wss://$LIVEKIT_HOST  (HTTP API https://$LIVEKIT_HOST)"
echo "turn:      turn:$TURN_DOMAIN:3478?transport=udp  turns:$TURN_DOMAIN:5349?transport=tcp"
echo "media:     udp $RTC_PORT_START-$RTC_PORT_END, tcp 7881 on $NODE_IP"
echo "webhook:   $WEBHOOK_URL (signed with $LIVEKIT_API_KEY)"
if [[ $GENERATED -eq 1 ]]; then
  echo "NEW API key pair generated — store it in the vault as \"Arbos LiveKit\": LIVEKIT_API_KEY=$LIVEKIT_API_KEY LIVEKIT_API_SECRET=$LIVEKIT_API_SECRET"
fi

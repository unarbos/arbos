#!/usr/bin/env bash
# Stand up (or update) Synapse + PostgreSQL for Arbos chats on the relay host (D88 stage A, ring 0).
# Idempotent; run as root:
#
#   sudo -E scripts/deploy/matrix/setup.sh                # secrets from /etc/arbos-matrix/matrix.env, the vault, or freshly generated
#   sudo -E scripts/deploy/matrix/setup.sh --print-env    # show the ARBOS_MATRIX_* lines the relay needs
#
# What it does (docs/self-host.md "Chats on Matrix"):
#   /etc/arbos-matrix/matrix.env               server name, ports, tokens (root only)
#   /etc/arbos-matrix/postgres.env             POSTGRES_* for the postgres container (root only)
#   /etc/arbos-matrix/synapse/homeserver.yaml  rendered from homeserver.yaml.tmpl
#   /etc/arbos-matrix/synapse/log.config       stdout logging
#   /etc/arbos-matrix/synapse/appservices/     arbos-users.yaml, arbos-agents.yaml, arbos-bridges.yaml
#   /etc/arbos-matrix/docker-compose.yml       ghcr.io/element-hq/synapse + postgres:16, host networking (loopback only)
#   /var/lib/arbos-matrix/{postgres,media}     data (uid 991 for Synapse media, postgres owns its dir)
#   arbos-matrix.service                       systemd wrapper around docker compose (After=docker)
#   /etc/arbos/relay.env                       ARBOS_MATRIX_* appended when missing (ARBOS_MATRIX stays as it is; default off)
#
# Nothing listens outside loopback: Synapse 127.0.0.1:7708 (client API), Postgres 127.0.0.1:7709.
# The apps only ever talk to relay.arbos.life; the relay proxies /_matrix/client/* when needed.
#
# Secrets: with OP_SERVICE_ACCOUNT_TOKEN set, tokens are read from the vault item "Arbos Matrix"
# (fields named like the env vars); otherwise an existing matrix.env wins; otherwise new tokens
# are generated and printed once — store them in the vault yourself.
set -euo pipefail
[[ $EUID -eq 0 ]] || { echo "run as root (sudo -E)" >&2; exit 1; }
REPO_DIR="$(cd "$(dirname "$0")/../../.." && pwd)"
HERE="$REPO_DIR/scripts/deploy/matrix"

MX_DIR=/etc/arbos-matrix
SYN_DIR=$MX_DIR/synapse
DATA_DIR=/var/lib/arbos-matrix
ENV_FILE=$MX_DIR/matrix.env
RELAY_ENV=/etc/arbos/relay.env
VAULT_ITEM="${ARBOS_MATRIX_VAULT_ITEM:-Arbos Matrix}"
PRINT_ONLY=0
[[ "${1:-}" == "--print-env" ]] && PRINT_ONLY=1

# ---- settings (env overrides; existing matrix.env is the baseline) ----------------------------------
# shellcheck disable=SC1090  # the host's matrix.env, outside the repo
[[ -f "$ENV_FILE" ]] && . "$ENV_FILE"
ARBOS_MATRIX_SERVER_NAME="${ARBOS_MATRIX_SERVER_NAME:-arbos.life}"
ARBOS_MATRIX_PUBLIC_BASEURL="${ARBOS_MATRIX_PUBLIC_BASEURL:-https://relay.arbos.life/}"
ARBOS_MATRIX_HS_URL="${ARBOS_MATRIX_HS_URL:-http://127.0.0.1:7708}"
CLIENT_PORT="${ARBOS_MATRIX_CLIENT_PORT:-7708}"
PG_PORT="${ARBOS_MATRIX_PG_PORT:-7709}"
RELAY_URL="${ARBOS_MATRIX_RELAY_URL:-http://127.0.0.1:7700}"

vault_read() {
  # under sudo -E HOME is the invoking user's; op needs a config dir it owns
  HOME=/root XDG_CONFIG_HOME=/root/.config op read "op://Arbos/$VAULT_ITEM/$1" 2>/dev/null </dev/null || true
}
TOKEN_NAMES=(ARBOS_MATRIX_PG_PASSWORD ARBOS_MATRIX_REGISTRATION_SHARED_SECRET ARBOS_MATRIX_MACAROON_SECRET_KEY
  ARBOS_MATRIX_FORM_SECRET ARBOS_MATRIX_AS_TOKEN_USERS ARBOS_MATRIX_HS_TOKEN_USERS ARBOS_MATRIX_AS_TOKEN_AGENTS
  ARBOS_MATRIX_HS_TOKEN_AGENTS ARBOS_MATRIX_AS_TOKEN_BRIDGES ARBOS_MATRIX_HS_TOKEN_BRIDGES)
GENERATED=0
for name in "${TOKEN_NAMES[@]}"; do
  if [[ -z "${!name:-}" ]] && command -v op >/dev/null 2>&1 && [[ -n "${OP_SERVICE_ACCOUNT_TOKEN:-}" ]]; then
    printf -v "$name" '%s' "$(vault_read "$name")"
  fi
  if [[ -z "${!name:-}" ]]; then
    printf -v "$name" '%s' "$(openssl rand -base64 48 | tr -dc 'A-Za-z0-9' | head -c 48)"
    GENERATED=1
  fi
done

if [[ $PRINT_ONLY -eq 1 ]]; then
  for name in ARBOS_MATRIX_HS_URL ARBOS_MATRIX_SERVER_NAME ARBOS_MATRIX_AS_TOKEN_USERS ARBOS_MATRIX_HS_TOKEN_USERS \
      ARBOS_MATRIX_AS_TOKEN_AGENTS ARBOS_MATRIX_HS_TOKEN_AGENTS ARBOS_MATRIX_AS_TOKEN_BRIDGES ARBOS_MATRIX_HS_TOKEN_BRIDGES; do
    printf '%s=%s\n' "$name" "${!name}"
  done
  exit 0
fi

say() { printf '\n==> %s\n' "$*"; }

# ---- 1. /etc/arbos-matrix ------------------------------------------------------------------------------
say "config"
install -d -m 0750 "$MX_DIR" "$SYN_DIR" "$SYN_DIR/appservices"
install -d -m 0750 "$DATA_DIR"
install -d -m 0700 "$DATA_DIR/postgres"
# The Synapse image runs as uid 991 and needs to read /data and write the media store + signing key.
install -d -o 991 -g 991 -m 0750 "$DATA_DIR/media"
umask 077
{
  echo "# Arbos Matrix (Synapse) — written by scripts/deploy/matrix/setup.sh on $(date -u +%FT%TZ). root only."
  echo "ARBOS_MATRIX_SERVER_NAME=$ARBOS_MATRIX_SERVER_NAME"
  echo "ARBOS_MATRIX_PUBLIC_BASEURL=$ARBOS_MATRIX_PUBLIC_BASEURL"
  echo "ARBOS_MATRIX_HS_URL=$ARBOS_MATRIX_HS_URL"
  echo "ARBOS_MATRIX_CLIENT_PORT=$CLIENT_PORT"
  echo "ARBOS_MATRIX_PG_PORT=$PG_PORT"
  echo "ARBOS_MATRIX_RELAY_URL=$RELAY_URL"
  for name in "${TOKEN_NAMES[@]}"; do printf '%s=%s\n' "$name" "${!name}"; done
} > "$ENV_FILE"
cat > "$MX_DIR/postgres.env" <<EOF
POSTGRES_USER=synapse
POSTGRES_PASSWORD=$ARBOS_MATRIX_PG_PASSWORD
POSTGRES_DB=synapse
POSTGRES_INITDB_ARGS=--encoding=UTF8 --lc-collate=C --lc-ctype=C
EOF
umask 022
sed -e "s#@SERVER_NAME@#$ARBOS_MATRIX_SERVER_NAME#g" -e "s#@PUBLIC_BASEURL@#$ARBOS_MATRIX_PUBLIC_BASEURL#g" \
    -e "s#@CLIENT_PORT@#$CLIENT_PORT#g" -e "s#@PG_PORT@#$PG_PORT#g" -e "s#@PG_PASSWORD@#$ARBOS_MATRIX_PG_PASSWORD#g" \
    -e "s#@REGISTRATION_SHARED_SECRET@#$ARBOS_MATRIX_REGISTRATION_SHARED_SECRET#g" \
    -e "s#@MACAROON_SECRET_KEY@#$ARBOS_MATRIX_MACAROON_SECRET_KEY#g" -e "s#@FORM_SECRET@#$ARBOS_MATRIX_FORM_SECRET#g" \
    "$HERE/homeserver.yaml.tmpl" > "$SYN_DIR/homeserver.yaml"
install -m 0644 "$HERE/log.config" "$SYN_DIR/log.config"
sed -i "s/synapse.access.http.8008/synapse.access.http.$CLIENT_PORT/" "$SYN_DIR/log.config"
render_as() { # id as_token hs_token sender_localpart kind
  python3 - "$HERE/appservice.yaml.tmpl" "$SYN_DIR/appservices/$1.yaml" "$1" "$RELAY_URL" "$2" "$3" "$4" "$5" "$ARBOS_MATRIX_SERVER_NAME" <<'PY'
import re, sys
src, dst, id_, url, as_tok, hs_tok, sender, kind, server = sys.argv[1:]
sn = re.escape(server).replace("\\", "\\\\")  # YAML double-quoted: \. stays a literal backslash-dot
def ns(entries):
    return "\n".join(f'    - exclusive: {"true" if ex else "false"}\n      regex: "{rx}"' for ex, rx in entries) or "    []"
users = {
    "users":   [(False, f"@gh_.*:{sn}"), (False, f"@u_.*:{sn}"), (False, f"@test_.*:{sn}")],
    "agents":  [(True, f"@_a_.*:{sn}"), (True, f"@_x_.*:{sn}")],
    "bridges": [(True, f"@_b_.*:{sn}"), (False, f"@gh_.*:{sn}")],
}[kind]
aliases = {"users": [], "agents": [], "bridges": [(True, f"#_b_.*:{sn}")]}[kind]
t = open(src).read()
for k, v in {"@ID@": id_, "@URL@": url, "@AS_TOKEN@": as_tok, "@HS_TOKEN@": hs_tok,
             "@SENDER_LOCALPART@": sender, "@USER_NAMESPACES@": ns(users), "@ALIAS_NAMESPACES@": ns(aliases),
             "@MSC4190@": "false" if kind == "users" else "true"}.items():
    t = t.replace(k, v)
open(dst, "w").write(t)
PY
  chmod 0640 "$SYN_DIR/appservices/$1.yaml"
}
render_as arbos-users "$ARBOS_MATRIX_AS_TOKEN_USERS" "$ARBOS_MATRIX_HS_TOKEN_USERS" _arbos users
render_as arbos-agents "$ARBOS_MATRIX_AS_TOKEN_AGENTS" "$ARBOS_MATRIX_HS_TOKEN_AGENTS" _arbos_agents agents
render_as arbos-bridges "$ARBOS_MATRIX_AS_TOKEN_BRIDGES" "$ARBOS_MATRIX_HS_TOKEN_BRIDGES" _arbos_bridges bridges
chmod 0640 "$SYN_DIR/homeserver.yaml"
chown -R 991:991 "$SYN_DIR"
install -m 0644 "$HERE/docker-compose.yml" "$MX_DIR/docker-compose.yml"
echo "wrote $MX_DIR/{matrix.env,postgres.env,docker-compose.yml} and $SYN_DIR/{homeserver.yaml,log.config,appservices/*.yaml}"

# ---- 2. signing key (generated once; Synapse never changes it) ----------------------------------------
say "signing key"
KEY="$SYN_DIR/$ARBOS_MATRIX_SERVER_NAME.signing.key"
if [[ ! -s "$KEY" ]]; then
  docker run --rm -u 991:991 -v "$SYN_DIR:/data" --entrypoint python ghcr.io/element-hq/synapse:v1.163.0 \
    -m synapse._scripts.generate_signing_key -o "/data/$ARBOS_MATRIX_SERVER_NAME.signing.key"
  echo "generated $KEY"
else
  echo "keeping $KEY"
fi

# ---- 3. systemd + docker --------------------------------------------------------------------------------
say "arbos-matrix.service"
cat > /etc/systemd/system/arbos-matrix.service <<EOF
[Unit]
Description=Synapse + PostgreSQL for Arbos chats (docker compose)
Requires=docker.service
After=docker.service network-online.target
Wants=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes
WorkingDirectory=$MX_DIR
ExecStart=/usr/bin/docker compose -f $MX_DIR/docker-compose.yml up -d --remove-orphans
ExecReload=/usr/bin/docker compose -f $MX_DIR/docker-compose.yml restart
ExecStop=/usr/bin/docker compose -f $MX_DIR/docker-compose.yml down
TimeoutStartSec=600

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
docker compose -f "$MX_DIR/docker-compose.yml" pull -q
systemctl enable arbos-matrix.service >/dev/null
if systemctl is-active --quiet arbos-matrix.service; then
  docker compose -f "$MX_DIR/docker-compose.yml" up -d --force-recreate >/dev/null
else
  systemctl start arbos-matrix.service
fi
for _ in $(seq 1 90); do curl -fsS --max-time 2 "http://127.0.0.1:$CLIENT_PORT/health" >/dev/null 2>&1 && break; sleep 2; done
if curl -fsS --max-time 2 "http://127.0.0.1:$CLIENT_PORT/health" >/dev/null; then
  echo "synapse answering on 127.0.0.1:$CLIENT_PORT: $(curl -fsS "http://127.0.0.1:$CLIENT_PORT/_matrix/client/versions" | python3 -c 'import json,sys; v=json.load(sys.stdin)["versions"]; print("spec", v[-1])')"
else
  echo "synapse not answering; docker logs arbos-matrix-synapse" >&2
  docker logs --tail 80 arbos-matrix-synapse >&2
  exit 1
fi

# ---- 4. relay env ----------------------------------------------------------------------------------------
say "relay env"
if [[ -f "$RELAY_ENV" ]]; then
  for name in ARBOS_MATRIX_HS_URL ARBOS_MATRIX_SERVER_NAME ARBOS_MATRIX_AS_TOKEN_USERS ARBOS_MATRIX_HS_TOKEN_USERS \
      ARBOS_MATRIX_AS_TOKEN_AGENTS ARBOS_MATRIX_HS_TOKEN_AGENTS ARBOS_MATRIX_AS_TOKEN_BRIDGES ARBOS_MATRIX_HS_TOKEN_BRIDGES; do
    kv="$name=${!name}"
    if grep -q "^$name=" "$RELAY_ENV"; then sed -i "s|^$name=.*|$kv|" "$RELAY_ENV"; else echo "$kv" >> "$RELAY_ENV"; fi
  done
  grep -q "^ARBOS_MATRIX=" "$RELAY_ENV" || echo "ARBOS_MATRIX=off" >> "$RELAY_ENV"
  echo "ARBOS_MATRIX_* set in $RELAY_ENV (ARBOS_MATRIX=$(grep '^ARBOS_MATRIX=' "$RELAY_ENV" | cut -d= -f2); the relay reads them on its next restart)"
else
  echo "no $RELAY_ENV here; give the relay the lines from: $0 --print-env"
fi

say "done"
echo "homeserver: $ARBOS_MATRIX_HS_URL (server_name $ARBOS_MATRIX_SERVER_NAME, loopback only, closed federation)"
echo "postgres:   127.0.0.1:$PG_PORT  (db synapse, user synapse)"
echo "appservice receiver: $RELAY_URL/_matrix/app/v1/… (hs_tokens in matrix.env)"
if [[ $GENERATED -eq 1 ]]; then
  echo "NEW secrets generated — store them in the vault as \"$VAULT_ITEM\" (one field per ARBOS_MATRIX_* name in $ENV_FILE)."
fi

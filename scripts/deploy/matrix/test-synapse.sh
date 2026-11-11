#!/usr/bin/env bash
# A throwaway Synapse for `cargo test -p arbos-matrix` (tests/e2ee.rs), in Docker on loopback.
# The production homeserver.yaml.tmpl and the image docker-compose.yml pins, with only what a
# test needs laid over it: SQLite inside the container, open registration with passwords (the
# tests make their own users), and one appservice shaped like arbos-users (people are registered
# and logged in with m.login.application_service, as the relay does it).
#
#   scripts/deploy/matrix/test-synapse.sh         # start (replaces a running one), print the exports
#   scripts/deploy/matrix/test-synapse.sh down    # remove the container; nothing else is kept
#
#   eval "$(scripts/deploy/matrix/test-synapse.sh | grep '^export ')"
#   cargo test -p arbos-matrix --locked --test e2ee -- --nocapture
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
NAME=arbos-matrix-test-synapse
PORT="${ARBOS_MATRIX_TEST_PORT:-7718}"
AS_TOKEN=test-as-token
IMAGE="$(sed -n 's/^ *image: *\(.*synapse.*\)$/\1/p' "$HERE/docker-compose.yml")"

docker rm -f "$NAME" >/dev/null 2>&1 || true
[[ "${1:-}" == "down" ]] && exit 0

CONF="$(mktemp -d)"
trap 'rm -rf "$CONF"' EXIT
mkdir "$CONF/appservices"
sed -e "s#@SERVER_NAME@#arbos.test#g" -e "s#@PUBLIC_BASEURL@#http://127.0.0.1:$PORT/#g" \
    -e "s#@CLIENT_PORT@#8008#g" -e "s#@PG_PORT@#0#g" -e "s#@PG_PASSWORD@#unused#g" \
    -e "s#@REGISTRATION_SHARED_SECRET@#test#g" -e "s#@MACAROON_SECRET_KEY@#test#g" -e "s#@FORM_SECRET@#test#g" \
    "$HERE/homeserver.yaml.tmpl" > "$CONF/homeserver.yaml"
cp "$HERE/log.config" "$CONF/log.config"
# Later config files replace whole top-level keys of earlier ones.
cat > "$CONF/test.yaml" <<'EOF'
listeners:
  - port: 8008
    bind_addresses: ["0.0.0.0"]
    type: http
    tls: false
    resources:
      - names: [client]
database:
  name: sqlite3
  args:
    database: /tmp/homeserver.db
signing_key_path: /tmp/signing.key
media_store_path: /tmp/media
enable_registration: true
enable_registration_without_verification: true
password_config:
  enabled: true
rc_registration:
  per_second: 100
  burst_count: 100
rc_login:
  address:
    per_second: 100
    burst_count: 100
  account:
    per_second: 100
    burst_count: 100
app_service_config_files:
  - /data/appservices/arbos-users.yaml
EOF
# arbos-users as setup.sh renders it (no MSC4190: people's devices log in for real tokens).
# Nothing listens on the url; Synapse retries its transactions quietly.
cat > "$CONF/appservices/arbos-users.yaml" <<EOF
id: "arbos-users"
url: "http://127.0.0.1:9"
as_token: "$AS_TOKEN"
hs_token: "test-hs-token"
sender_localpart: "_arbos"
rate_limited: false
namespaces:
  users:
    - exclusive: false
      regex: "@gh_.*:arbos\\\\.test"
  aliases: []
  rooms: []
receive_ephemeral: true
de.sorunome.msc2409.push_ephemeral: true
org.matrix.msc3202: true
io.element.msc4190: false
EOF

docker create --name "$NAME" -p "127.0.0.1:$PORT:8008" --entrypoint sh "$IMAGE" -c \
  'python -m synapse._scripts.generate_signing_key -o /tmp/signing.key &&
   exec python -m synapse.app.homeserver -c /data/homeserver.yaml -c /data/test.yaml' >/dev/null
docker cp -q "$CONF/." "$NAME:/data"
docker start "$NAME" >/dev/null
for _ in $(seq 1 60); do curl -fsS --max-time 2 "http://127.0.0.1:$PORT/health" >/dev/null 2>&1 && break; sleep 1; done
curl -fsS --max-time 2 "http://127.0.0.1:$PORT/health" >/dev/null || { docker logs --tail 40 "$NAME" >&2; exit 1; }
echo "synapse ($IMAGE) answering on 127.0.0.1:$PORT; stop it with: $0 down"
echo "export ARBOS_MATRIX_TEST_E2EE_HS=http://127.0.0.1:$PORT"
echo "export ARBOS_MATRIX_TEST_E2EE_AS_TOKEN=$AS_TOKEN"

#!/usr/bin/env bash
# Sets up the Subnet view's side of Grafana. Run on the host after `docker compose up -d`, and again
# whenever the Subnet dashboard or the viewer's password changes; every step is idempotent.
#
#   organisation "Subnet"   separate from the main one that holds the admin dashboards
#   its only datasource     the prometheus-subnet instance, which holds nothing but `subnet:*` aggregates
#   its only dashboard      grafana/dashboards-subnet/subnet.json, also its home page
#   one Viewer account      SUBNET_VIEWER_LOGIN / SUBNET_VIEWER_PASSWORD, a member of that organisation only
#
# Grafana cannot create an organisation from provisioning files (it refuses to start when a file
# names one that does not exist), so this goes through its HTTP API on loopback as the admin.
#
#   usage: grafana-access.sh [--check] [env file]      default /etc/arbos/monitoring.env
#
# --check only validates the env file and touches nothing: run it before the first `docker compose up`,
# because Grafana takes its admin password once, when it creates its database.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
CHECK_ONLY=
if [[ ${1:-} == --check ]]; then CHECK_ONLY=1; shift; fi
ENV_FILE=${1:-/etc/arbos/monitoring.env}
GRAFANA=${GRAFANA:-http://127.0.0.1:3300}
ORG_NAME=Subnet

value() { sed -n "s/^$1=//p" "$ENV_FILE" | tail -n 1; }
ADMIN_PASSWORD=$(value GF_SECURITY_ADMIN_PASSWORD)
LOGIN=$(value SUBNET_VIEWER_LOGIN)
PASSWORD=$(value SUBNET_VIEWER_PASSWORD)
WEBHOOK=$(value ALERT_WEBHOOK_URL)
ROOT_URL=$(value GF_SERVER_ROOT_URL)
bad() { echo "$ENV_FILE: $1" >&2; exit 1; }
# The passwords and the login are placed inside JSON below, so only characters that need no escaping
# are accepted; 16 characters of this alphabet is the floor for something generated, not chosen.
STRONG='^[A-Za-z0-9._~-]{16,128}$'
[[ $ADMIN_PASSWORD =~ $STRONG ]] || bad "GF_SECURITY_ADMIN_PASSWORD must be 16-128 of A-Z a-z 0-9 . _ ~ -"
[[ $PASSWORD =~ $STRONG ]] || bad "SUBNET_VIEWER_PASSWORD must be 16-128 of A-Z a-z 0-9 . _ ~ -"
[[ $PASSWORD != "$ADMIN_PASSWORD" ]] || bad "SUBNET_VIEWER_PASSWORD must differ from the admin password"
shopt -s nocasematch
for p in "$ADMIN_PASSWORD" "$PASSWORD"; do
  # fewer than 8 different characters, or a word everyone tries first
  [[ $(printf %s "$p" | fold -w 1 | sort -u | wc -l) -ge 8 && ! $p =~ (admin|password|grafana|arbos|changeme|123456) ]] || bad "a password is trivially guessable"
done
shopt -u nocasematch
[[ $LOGIN =~ ^[a-z0-9_-]{2,32}$ && $LOGIN != admin ]] || bad "SUBNET_VIEWER_LOGIN must be 2-32 of a-z 0-9 _ - and not 'admin'"
# https only, except a listener on this machine (the local run).
[[ $WEBHOOK =~ ^https://[A-Za-z0-9.-]+/[^[:space:]\"]+$ || $WEBHOOK =~ ^http://127\.0\.0\.1:[0-9]+/ ]] || bad "ALERT_WEBHOOK_URL must be an https:// URL"
[[ $ROOT_URL =~ ^https://[A-Za-z0-9.-]+/$ || $ROOT_URL =~ ^http://127\.0\.0\.1:[0-9]+/$ ]] || bad "GF_SERVER_ROOT_URL must be https://<host>/"
if [[ -n $CHECK_ONLY ]]; then echo "$ENV_FILE: ok"; exit 0; fi

# api METHOD PATH [ORG_ID] — JSON body on stdin; prints the response body, then the status on a last line.
# The admin password goes to curl through a config on a file descriptor, never on its command line.
api() {
  curl -sS -o - -w '\n%{http_code}' -X "$1" "$GRAFANA$2" -K <(printf 'user = "admin:%s"\n' "$ADMIN_PASSWORD") \
    -H 'Content-Type: application/json' ${3:+-H "X-Grafana-Org-Id: $3"} --data-binary @-
}
# expect "what" "allowed statuses" <<< response — fails loudly on anything else.
expect() {
  local out; out=$(cat); local code=${out##*$'\n'}
  [[ " $2 " == *" $code "* ]] || { echo "$1: HTTP $code: ${out%$'\n'*}" >&2; exit 1; }
  printf '%s' "${out%$'\n'*}"
}
id_of() { sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p' | head -n 1; }

api POST /api/orgs <<<"{\"name\":\"$ORG_NAME\"}" | expect "create organisation" "200 409" >/dev/null
ORG=$(api GET "/api/orgs/name/$ORG_NAME" </dev/null | expect "find organisation" "200" | id_of)
[[ $ORG =~ ^[0-9]+$ && $ORG != 1 ]] || { echo "unexpected organisation id '$ORG'" >&2; exit 1; }

DATASOURCE='{"name":"Prometheus","uid":"prometheus-subnet","type":"prometheus","access":"proxy","url":"http://127.0.0.1:9091","isDefault":true,"jsonData":{"timeInterval":"30s"}}'
api POST /api/datasources "$ORG" <<<"$DATASOURCE" | expect "create datasource" "200 409" >/dev/null
api PUT /api/datasources/uid/prometheus-subnet "$ORG" <<<"$DATASOURCE" | expect "update datasource" "200" >/dev/null

{ printf '{"overwrite":true,"dashboard":'; cat "$HERE/grafana/dashboards-subnet/subnet.json"; printf '}'; } \
  | api POST /api/dashboards/db "$ORG" | expect "load dashboard" "200" >/dev/null
api PUT /api/org/preferences "$ORG" <<<'{"homeDashboardUID":"arbos-subnet"}' | expect "set home dashboard" "200" >/dev/null
# The main organisation's home is set the same way, per organisation. There is deliberately no
# instance-wide default home dashboard: it would also be what the Subnet organisation falls back to
# if its own preference were ever lost.
api PUT /api/org/preferences 1 <<<'{"homeDashboardUID":"arbos"}' | expect "set the main home dashboard" "200" >/dev/null

found=$(api GET "/api/users/lookup?loginOrEmail=$LOGIN" </dev/null | expect "look up viewer" "200 404")
USER_ID=$(id_of <<<"$found")
if [[ -z $USER_ID ]]; then
  USER_ID=$(api POST /api/admin/users <<<"{\"login\":\"$LOGIN\",\"name\":\"$LOGIN\",\"password\":\"$PASSWORD\",\"OrgId\":$ORG}" \
    | expect "create viewer" "200" | id_of)
else
  api PUT "/api/admin/users/$USER_ID/password" <<<"{\"password\":\"$PASSWORD\"}" | expect "set viewer password" "200" >/dev/null
  api POST "/api/orgs/$ORG/users" <<<"{\"loginOrEmail\":\"$LOGIN\",\"role\":\"Viewer\"}" | expect "add viewer to organisation" "200 409" >/dev/null
fi
api PATCH "/api/orgs/$ORG/users/$USER_ID" <<<'{"role":"Viewer"}' | expect "make viewer a Viewer" "200" >/dev/null
# Not a member of the main organisation, whatever Grafana's defaults did (404/400: was not one).
api DELETE "/api/orgs/1/users/$USER_ID" </dev/null | expect "remove viewer from the main organisation" "200 400 404" >/dev/null

# Prove the boundary instead of assuming it: as the viewer, the admin dashboards and datasource must
# not be reachable, and the Subnet dashboard must be.
as_viewer() { curl -sS -o /dev/null -w '%{http_code}' "$GRAFANA$1" -K <(printf 'user = "%s:%s"\n' "$LOGIN" "$PASSWORD"); }
[[ $(as_viewer /api/dashboards/uid/arbos-subnet) == 200 ]] || { echo "viewer cannot open the Subnet view" >&2; exit 1; }
for path in /api/dashboards/uid/arbos /api/dashboards/uid/arbos-fleet /api/dashboards/uid/arbos-boxes /api/dashboards/uid/arbos-agents \
            /api/dashboards/uid/arbos-swarm \
            /api/datasources/uid/prometheus "/api/datasources/proxy/uid/prometheus/api/v1/query?query=up"; do
  code=$(as_viewer "$path")
  [[ $code == 403 || $code == 404 ]] || { echo "viewer got HTTP $code on $path — it must be refused" >&2; exit 1; }
done
# A Viewer has no Explore (no free-form query page). That is Grafana's default; this fails if a
# setting or an upgrade changes it.
actions=$(curl -sS "$GRAFANA/api/access-control/user/actions" -K <(printf 'user = "%s:%s"\n' "$LOGIN" "$PASSWORD"))
[[ $actions == *'"dashboards:read"'* ]] || { echo "could not read the viewer's permissions: $actions" >&2; exit 1; }
[[ $actions != *'"datasources:explore"'* ]] || { echo "the viewer has Explore — it must not" >&2; exit 1; }
# The viewer's home is the Subnet view, never an admin dashboard.
home=$(curl -sS "$GRAFANA/api/dashboards/home" -K <(printf 'user = "%s:%s"\n' "$LOGIN" "$PASSWORD"))
[[ $home == *arbos-subnet* && $home != *arbos-fleet* ]] || { echo "the viewer's home is not the Subnet view" >&2; exit 1; }
echo "Subnet organisation $ORG ready; viewer '$LOGIN' can open the Subnet view and nothing else."

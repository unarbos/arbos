#!/usr/bin/env bash
# Deploy the agent-sites edge (#2060): the R2 bucket, the `arbos-sites` Worker with its
# `*.arbos.life/*` route, and — on request — the zone changes that put the apex behind
# Cloudflare. Runs from any machine with node + the two tokens in the environment:
#
#   CLOUDFLARE_API_TOKEN     account token (vault "Cloudflare — account API token (cfat_…)"):
#                            Worker scripts, routes, R2
#   CLOUDFLARE_SITES_TOKEN   zone token (vault "Cloudflare — Arbos sites edge token"):
#                            zone settings, DNS, WAF for arbos.life
#
#   scripts/deploy/sites-edge.sh worker            # bucket (if missing) + wrangler deploy
#   scripts/deploy/sites-edge.sh route             # the *.arbos.life/* route alone (zone token)
#   scripts/deploy/sites-edge.sh verify            # read-only: token, ssl mode, routes, records
#   scripts/deploy/sites-edge.sh ssl-strict        # zone SSL mode → full (strict) + Always Use HTTPS
#   scripts/deploy/sites-edge.sh proxy-apex on|off # the arbos.life A record's orange cloud
#   scripts/deploy/sites-edge.sh rate-limit        # one WAF rate-limiting rule for *.arbos.life
#
# Secrets are read from the environment only (fill it with `op read` at deploy time); nothing
# is written to disk. Never pass a token on the command line.
set -euo pipefail

HERE="$(cd "$(dirname "$0")/../.." && pwd)"
WORKER_DIR="$HERE/edge/sites-worker"
ACCOUNT_ID="${CLOUDFLARE_ACCOUNT_ID:-00523074f51300584834607253cae0fa}"
ZONE_ID="${CLOUDFLARE_ZONE_ID:-9f03f556bfde998dc5982a3903246cad}"
DOMAIN="${ARBOS_SITES_DOMAIN:-arbos.life}"
BUCKET="${ARBOS_SITES_R2_BUCKET:-arbos-sites}"
API=https://api.cloudflare.com/client/v4

say() { printf '\n==> %s\n' "$*"; }

zone_api() {
  # $1 method, $2 path (after /zones/<id>), $3 json body (optional)
  local m=$1 p=$2 body=${3:-}
  [[ -n "${CLOUDFLARE_SITES_TOKEN:-}" ]] || { echo "CLOUDFLARE_SITES_TOKEN is not set" >&2; exit 1; }
  if [[ -n "$body" ]]; then
    curl -sS -X "$m" -H "Authorization: Bearer $CLOUDFLARE_SITES_TOKEN" -H 'Content-Type: application/json' \
      --data "$body" "$API/zones/$ZONE_ID$p"
  else
    curl -sS -X "$m" -H "Authorization: Bearer $CLOUDFLARE_SITES_TOKEN" "$API/zones/$ZONE_ID$p"
  fi
}

ok() { python3 -c 'import sys,json; d=json.load(sys.stdin); print(json.dumps(d.get("result"))[:400]); sys.exit(0 if d.get("success") else 1)'; }

cmd_verify() {
  say "zone token"
  curl -sS -H "Authorization: Bearer ${CLOUDFLARE_SITES_TOKEN:?}" "$API/accounts/$ACCOUNT_ID/tokens/verify" | ok
  say "ssl mode"; zone_api GET /settings/ssl | ok
  say "always use https"; zone_api GET /settings/always_use_https | ok
  say "worker routes"; zone_api GET /workers/routes | ok
  say "A records"
  zone_api GET "/dns_records?type=A&per_page=100" | python3 -c 'import sys,json
for r in json.load(sys.stdin)["result"]: print("  %-28s %-16s proxied=%s" % (r["name"], r["content"], r["proxied"]))'
}

cmd_worker() {
  [[ -n "${CLOUDFLARE_API_TOKEN:-}" ]] || { echo "CLOUDFLARE_API_TOKEN is not set" >&2; exit 1; }
  export CLOUDFLARE_ACCOUNT_ID="$ACCOUNT_ID"
  cd "$WORKER_DIR"
  [[ -d node_modules/wrangler ]] || npm install --no-audit --no-fund --silent
  say "R2 bucket $BUCKET"
  if npx wrangler r2 bucket list 2>/dev/null | grep -qx "name:\s*$BUCKET" || npx wrangler r2 bucket info "$BUCKET" >/dev/null 2>&1; then
    echo "bucket exists"
  else
    npx wrangler r2 bucket create "$BUCKET"
  fi
  say "worker deploy"
  npx wrangler deploy
  say "worker version"
  npx wrangler deployments list 2>/dev/null | head -12 || true
  cmd_route
}

# `*.<domain>/*` → arbos-sites, created once with the zone token (idempotent).
cmd_route() {
  local pattern="*.$DOMAIN/*" existing
  say "worker route $pattern"
  existing=$(zone_api GET /workers/routes)
  if python3 -c 'import sys,json; d=json.load(sys.stdin); sys.exit(0 if any(r["pattern"]==sys.argv[1] and r.get("script")=="arbos-sites" for r in d.get("result",[])) else 1)' "$pattern" <<<"$existing"; then
    echo "route exists"
  else
    zone_api POST /workers/routes "{\"pattern\":\"$pattern\",\"script\":\"arbos-sites\"}" | ok
  fi
}

cmd_ssl_strict() {
  say "origin must serve a valid certificate for $DOMAIN before strict"
  if ! echo | openssl s_client -servername "$DOMAIN" -connect 204.12.171.6:443 -verify_return_error >/dev/null 2>&1; then
    echo "origin certificate for $DOMAIN does not verify; not switching" >&2; exit 1
  fi
  say "ssl → strict"; zone_api PATCH /settings/ssl '{"value":"strict"}' | ok
  say "always use https → on"; zone_api PATCH /settings/always_use_https '{"value":"on"}' | ok
}

cmd_proxy_apex() {
  local want=${1:?on|off} flag
  case "$want" in on) flag=true;; off) flag=false;; *) echo "on|off" >&2; exit 2;; esac
  local rec id
  rec=$(zone_api GET "/dns_records?type=A&name=$DOMAIN")
  id=$(python3 -c 'import sys,json; r=json.load(sys.stdin)["result"]; print(r[0]["id"] if r else "")' <<<"$rec")
  [[ -n "$id" ]] || { echo "no A record for $DOMAIN" >&2; exit 1; }
  say "A $DOMAIN proxied → $flag"
  zone_api PATCH "/dns_records/$id" "{\"proxied\":$flag}" | ok
}

cmd_rate_limit() {
  say "rate-limiting rule on *.$DOMAIN (300 req / 10 s per IP → 429)"
  local existing rid
  existing=$(zone_api GET "/rulesets/phases/http_ratelimit/entrypoint" || true)
  rid=$(python3 -c 'import sys,json
try:
  d=json.load(sys.stdin); print(d["result"]["id"] if d.get("success") else "")
except Exception: print("")' <<<"$existing")
  local rule='{"description":"arbos sites: per-IP rate limit","expression":"(http.host wildcard \"*.'"$DOMAIN"'\" and not http.host in {\"relay.'"$DOMAIN"'\" \"livekit.'"$DOMAIN"'\" \"turn.'"$DOMAIN"'\"})","action":"block","ratelimit":{"characteristics":["ip.src","cf.colo.id"],"period":10,"requests_per_period":300,"mitigation_timeout":10}}'
  if [[ -z "$rid" ]]; then
    zone_api PUT "/rulesets/phases/http_ratelimit/entrypoint" "{\"name\":\"ratelimit\",\"kind\":\"zone\",\"phase\":\"http_ratelimit\",\"rules\":[$rule]}" | ok
  else
    zone_api POST "/rulesets/$rid/rules" "$rule" | ok
  fi
}

case "${1:-}" in
  verify) cmd_verify;;
  worker) cmd_worker;;
  ssl-strict) cmd_ssl_strict;;
  proxy-apex) cmd_proxy_apex "${2:-}";;
  rate-limit) cmd_rate_limit;;
  route) cmd_route;;
  *) sed -n '2,20p' "$0"; exit 2;;
esac

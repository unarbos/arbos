#!/usr/bin/env bash
# Static checks for this folder; needs Docker (for promtool/amtool at the pinned versions) and python3.
# CI runs it. The checks against a running stack are local/check-dashboards.py.
set -euo pipefail
cd "$(dirname "$0")"

image() { sed -n "s|^ *image: \($1:.*\)|\1|p" docker-compose.yml | head -n 1; }
promtool() { docker run --rm -v "$PWD/prometheus:/etc/prometheus:ro" -w /etc/prometheus --entrypoint promtool "$(image prom/prometheus)" "$@"; }

promtool check config prometheus.yml subnet.yml
promtool check rules rules.yml subnet-rules.yml
promtool test rules rules_test.yml
docker run --rm -v "$PWD/alertmanager:/etc/alertmanager:ro" --entrypoint amtool "$(image prom/alertmanager)" check-config /etc/alertmanager/alertmanager.yml
python3 check-rules.py
python3 dashboards.py --check
docker compose --env-file local/local.env config -q
# no secrets, no start: an empty env file must be refused
if docker compose --env-file .env.example config -q 2>/dev/null; then echo "compose accepted empty secrets" >&2; exit 1; fi
# the env check accepts the local values and refuses a guessable admin password
./grafana-access.sh --check local/local.env >/dev/null
weak=$(mktemp)
sed 's/^GF_SECURITY_ADMIN_PASSWORD=.*/GF_SECURITY_ADMIN_PASSWORD=Password-12345678/' local/local.env > "$weak"
if ./grafana-access.sh --check "$weak" 2>/dev/null; then echo "a guessable password was accepted" >&2; exit 1; fi
rm -f "$weak"
echo "monitoring: all checks passed"

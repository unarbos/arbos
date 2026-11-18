#!/usr/bin/env bash
# Static checks for this folder; needs Docker (for promtool/amtool at the pinned versions) and python3.
# CI runs it. The checks against a running stack are local/check-dashboards.py.
set -euo pipefail
cd "$(dirname "$0")"

image() { sed -n "s|^ *image: \($1:.*\)|\1|p" docker-compose.yml | head -n 1; }
promtool() { docker run --rm -v "$PWD/prometheus:/etc/prometheus:ro" -v "$PWD/local:/etc/secrets:ro" -w /etc/prometheus --entrypoint promtool "$(image prom/prometheus)" "$@"; }

promtool check config prometheus.yml subnet.yml
promtool check rules rules.yml subnet-rules.yml
promtool test rules rules_test.yml
docker run --rm -v "$PWD/alertmanager:/etc/alertmanager:ro" --entrypoint amtool "$(image prom/alertmanager)" check-config /etc/alertmanager/alertmanager.yml
python3 dashboards.py --check
OBS_ENV_FILE=.env.example docker compose config -q
echo "observability: all checks passed"

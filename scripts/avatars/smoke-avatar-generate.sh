#!/usr/bin/env bash
# One REAL avatar generation, end to end, and a check that an image came back. Costs about half
# a cent (gpt-image-1-mini, one 1024² square at low quality). Off unless ARBOS_AVATAR_SMOKE=1, so
# CI and casual runs stay offline.
#
# Two modes:
#   relay  — against a running relay with a device token (production or a local one):
#            ARBOS_AVATAR_SMOKE=1 ARBOS_RELAY_URL=https://relay.arbos.life ARBOS_DEVICE_TOKEN=… \
#            ARBOS_PROJECT_ID=<one of that user's agents> scripts/avatars/smoke-avatar-generate.sh
#            Calls POST /projects/{id}/avatar/generate, fetches the stored blob, asserts a JPEG.
#            The picture is stored but NOT set as the avatar (Set avatar does that).
#   direct — no relay: the relay crate's gated unit test against api.openai.com:
#            ARBOS_AVATAR_SMOKE=1 OPENAI_API_KEY=… scripts/avatars/smoke-avatar-generate.sh
#
# Optional: ARBOS_SMOKE_OUT=<path.jpg> saves the picture (relay mode).
set -euo pipefail

if [ "${ARBOS_AVATAR_SMOKE:-0}" != "1" ]; then
  echo "skip: set ARBOS_AVATAR_SMOKE=1 to spend ~\$0.005 on one real generation"
  exit 0
fi

if [ -n "${ARBOS_RELAY_URL:-}" ] && [ -n "${ARBOS_DEVICE_TOKEN:-}" ] && [ -n "${ARBOS_PROJECT_ID:-}" ]; then
  base=${ARBOS_RELAY_URL%/}
  prompt=${ARBOS_SMOKE_PROMPT:-a cat}
  body=$(curl -sS -w '\n%{http_code}' -X POST "$base/projects/$ARBOS_PROJECT_ID/avatar/generate" \
    -H "Authorization: Bearer $ARBOS_DEVICE_TOKEN" -H 'content-type: application/json' \
    -d "{\"prompt\":$(printf '%s' "$prompt" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read()))')}")
  code=${body##*$'\n'}
  json=${body%$'\n'*}
  if [ "$code" != "200" ]; then
    echo "FAIL: HTTP $code: $json"
    exit 1
  fi
  key=$(printf '%s' "$json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["image_key"])')
  model=$(printf '%s' "$json" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("model",""))')
  out=${ARBOS_SMOKE_OUT:-$(mktemp -t avatar-smoke.XXXXXX.jpg)}
  curl -sS -o "$out" -H "Authorization: Bearer $ARBOS_DEVICE_TOKEN" "$base/blob/$ARBOS_PROJECT_ID/$key"
  magic=$(head -c 3 "$out" | od -An -tx1 | tr -d ' \n')
  if [ "$magic" != "ffd8ff" ]; then
    echo "FAIL: $key is not a JPEG (first bytes $magic)"
    exit 1
  fi
  echo "OK: $model generated $key ($(wc -c < "$out") bytes) -> $out"
  exit 0
fi

if [ -n "${OPENAI_API_KEY:-}" ]; then
  cd "$(dirname "$0")/../.."
  exec cargo test -p arbos-relay --locked -q smoke_real_generation_when_asked -- --nocapture
fi

echo "need either ARBOS_RELAY_URL + ARBOS_DEVICE_TOKEN + ARBOS_PROJECT_ID (relay mode) or OPENAI_API_KEY (direct mode)" >&2
exit 2

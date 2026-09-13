#!/usr/bin/env bash
# Publish the current public URL so other agents can find the endpoint after a tunnel restart.
# Writes voice-endpoint.txt on the repo's qa-results branch (the branch the QA loop already
# syncs into the Project store). Needs ARBOS_GITHUB (GitHub PAT with repo write) in the env.
#
#   deploy/publish-url.sh https://xxxx.trycloudflare.com
set -euo pipefail
URL="${1:?public url}"
: "${ARBOS_GITHUB:?ARBOS_GITHUB not set; not publishing}"
VOICE_HOME="${VOICE_HOME:?}"
WORK="$VOICE_HOME/qa-results"
REMOTE="https://github.com/unarbos/arbos.git"
if [ ! -d "$WORK/.git" ]; then
  git clone -q --depth 1 --branch qa-results --single-branch "$REMOTE" "$WORK"
else
  git -C "$WORK" pull -q --rebase origin qa-results || true
fi
{
  echo "$URL"
  echo "# Arbos voice server public URL (Cloudflare quick tunnel; changes when the tunnel restarts)."
  echo "# WebSocket: ${URL/https:/wss:}/ws?token=<VOICE_TOKEN from 1Password item jmldktl7rrc4rw4sm2akej4qne>"
  echo "# Health: $URL/healthz   Updated: $(date -u +%FT%TZ) by deploy/supervise.sh on $(hostname)"
} > "$WORK/voice-endpoint.txt"
git -C "$WORK" add voice-endpoint.txt
if git -C "$WORK" diff --cached --quiet; then echo "unchanged"; exit 0; fi
git -C "$WORK" -c user.name=arbos-voice -c user.email=voice@arbos.local commit -q -m "voice endpoint: $URL"
git -C "$WORK" -c "credential.helper=!f() { echo username=x-access-token; echo password=\$ARBOS_GITHUB; }; f" push -q origin qa-results
echo "published $URL to qa-results/voice-endpoint.txt"

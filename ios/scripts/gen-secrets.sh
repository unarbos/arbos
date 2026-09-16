#!/bin/sh
# Writes ios/Arbos/Secrets.plist for a dev build: the live endpoints and
# the tokens the app needs on first launch, read from 1Password and the
# published endpoint directory. The file is gitignored; run this before
# building, never commit the output. Settings in the app still override.
set -eu
cd "$(dirname "$0")/.."
OUT="Arbos/Secrets.plist"

read_field() { op item get "$1" --vault Arbos --fields "label=$2" --reveal 2>/dev/null | tr -d '\n' || true; }

VOICE_TOKEN="$(read_field jmldktl7rrc4rw4sm2akej4qne credential)"
KERNEL_TOKEN="$(read_field 4vgzvrtucv6dm7eyaw42ckqsv4 credential)"
# The phone's own row on the hub (role owner); `credential` is the desktop's.
HUB_TOKEN="$(read_field 6uihrhmgfwncp3jz3vxtfxklhi client-phone)"

# Where the hub and the phone kernel live (2026-09-16 cutover): ArbosLife,
# Jacob's own box. The stable names (`hub-api` / `kernel-api.arbos.life`)
# survive a tunnel restart, so they win as soon as Jacob's CNAMEs point at
# the ArbosLife tunnel; until then the vault's interim quick-tunnel fields.
# ARBOS_HUB_URL / ARBOS_KERNEL_URL override both (a test, never a phone build
# — the guard refuses a local hub).
STABLE_HUB="$(read_field 6uihrhmgfwncp3jz3vxtfxklhi url)"
INTERIM_HUB="$(read_field 6uihrhmgfwncp3jz3vxtfxklhi arboslife-hub-url)"
INTERIM_KERNEL="$(read_field 6uihrhmgfwncp3jz3vxtfxklhi arboslife-kernel-url)"
STABLE_KERNEL="wss://kernel-api.arbos.life"

# A hub candidate is good when it lists machines for the phone token.
hub_ok() {
  [ -n "$1" ] || return 1
  code="$(curl -sS --max-time 10 -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $HUB_TOKEN" \
    "$(printf '%s' "$1" | sed -e 's#^wss://#https://#' -e 's#/*$##')/list" 2>/dev/null || echo 000)"
  [ "$code" = "200" ]
}
# A kernel name is good when its DNS is the ArbosLife tunnel (a CNAME to
# cfargotunnel.com), not a wildcard somewhere else.
kernel_ok() {
  [ -n "$1" ] || return 1
  host="$(printf '%s' "$1" | sed -e 's#^wss://##' -e 's#[/:].*$##')"
  dig +short CNAME "$host" 2>/dev/null | grep -q 'cfargotunnel.com' || host "$host" 2>/dev/null | grep -q 'cfargotunnel.com'
}

if [ -n "${ARBOS_HUB_URL:-}" ]; then HUB_URL="$ARBOS_HUB_URL"
elif hub_ok "$STABLE_HUB"; then HUB_URL="$STABLE_HUB"; echo "hub: the stable name answers"
elif hub_ok "$INTERIM_HUB"; then HUB_URL="$INTERIM_HUB"; echo "hub: the stable name is not the hub yet; the interim tunnel is"
else HUB_URL="${INTERIM_HUB:-$STABLE_HUB}"; echo "hub: neither name answers now — the guard decides" >&2
fi
if [ -n "${ARBOS_KERNEL_URL:-}" ]; then KERNEL_URL="$ARBOS_KERNEL_URL"
elif kernel_ok "$STABLE_KERNEL"; then KERNEL_URL="$STABLE_KERNEL"; echo "kernel: the stable name points at the tunnel"
else KERNEL_URL="$INTERIM_KERNEL"; echo "kernel: interim tunnel"
fi

# The voice server still comes from the published directory (it stays on
# the pod with the voice model).
DIRECTORY="$(curl -fsS --max-time 8 https://raw.githubusercontent.com/unarbos/arbos/qa-results/voice-endpoint.txt || true)"
VOICE_URL="$(printf '%s\n' "$DIRECTORY" | grep -v '^#' | grep -m1 '^http' | sed -e 's#^https://#wss://#' -e 's#^http://#ws://#' -e 's#/*$#/ws#')"

esc() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g'; }

cat > "$OUT" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>voiceServerURL</key><string>$(esc "$VOICE_URL")</string>
	<key>voiceToken</key><string>$(esc "$VOICE_TOKEN")</string>
	<key>kernelURL</key><string>$(esc "$KERNEL_URL")</string>
	<key>kernelToken</key><string>$(esc "$KERNEL_TOKEN")</string>
	<key>hubURL</key><string>$(esc "$HUB_URL")</string>
	<key>hubToken</key><string>$(esc "$HUB_TOKEN")</string>
</dict>
</plist>
EOF
plutil -lint "$OUT" >/dev/null
# A phone build never carries a local hub: the guard runs on every write.
"$(dirname "$0")/check-secrets.sh" "$OUT"
echo "wrote $OUT (voice=${VOICE_URL:-?} kernel=${KERNEL_URL:-?} hub=${HUB_URL:-?}; tokens: voice ${#VOICE_TOKEN} kernel ${#KERNEL_TOKEN} hub ${#HUB_TOKEN} chars)"

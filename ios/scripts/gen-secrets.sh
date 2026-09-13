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
HUB_TOKEN="$(read_field 6uihrhmgfwncp3jz3vxtfxklhi credential)"
HUB_URL="$(read_field 6uihrhmgfwncp3jz3vxtfxklhi url)"

DIRECTORY="$(curl -fsS --max-time 8 https://raw.githubusercontent.com/unarbos/arbos/qa-results/voice-endpoint.txt || true)"
VOICE_URL="$(printf '%s\n' "$DIRECTORY" | grep -v '^#' | grep -m1 '^http' | sed -e 's#^https://#wss://#' -e 's#^http://#ws://#' -e 's#/*$#/ws#')"
KERNEL_URL="$(printf '%s\n' "$DIRECTORY" | grep -m1 '^kernel:' | sed -e 's/^kernel:[[:space:]]*//' -e 's/?.*$//' -e 's/[[:space:]].*$//')"
DIR_HUB_URL="$(printf '%s\n' "$DIRECTORY" | grep -m1 '^hub:' | sed -e 's/^hub:[[:space:]]*//' -e 's/?.*$//' -e 's/[[:space:]].*$//')"
[ -n "$DIR_HUB_URL" ] && HUB_URL="$DIR_HUB_URL"

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
echo "wrote $OUT (voice=${VOICE_URL:-?} kernel=${KERNEL_URL:-?} hub=${HUB_URL:-?}; tokens: voice ${#VOICE_TOKEN} kernel ${#KERNEL_TOKEN} hub ${#HUB_TOKEN} chars)"

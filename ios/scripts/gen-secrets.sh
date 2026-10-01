#!/bin/sh
# Writes ios/Arbos/Secrets.plist for the orb app: voice server URL only.
# OpenAI key is typed in Settings. The file is gitignored.
set -eu
cd "$(dirname "$0")/.."
OUT="Arbos/Secrets.plist"

read_field() { op item get "$1" --vault Arbos --fields "label=$2" --reveal 2>/dev/null | tr -d '\n' || true; }

# Prefer a published voice endpoint; fall back to vault interim / env.
VOICE_URL="${ARBOS_VOICE_URL:-}"
if [ -z "$VOICE_URL" ]; then
  DIRECTORY="$(curl -fsS --max-time 8 https://raw.githubusercontent.com/unarbos/arbos/qa-results/voice-endpoint.txt || true)"
  VOICE_URL="$(printf '%s\n' "$DIRECTORY" | grep -v '^#' | grep -m1 '^http' | sed -e 's#^https://#wss://#' -e 's#^http://#ws://#' -e 's#/*$#/ws#')"
fi
if [ -z "$VOICE_URL" ]; then
  # Last resort: hub item notes sometimes hold a voice tunnel — leave empty if none.
  VOICE_URL=""
fi

esc() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g'; }

cat > "$OUT" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>voiceServerURL</key><string>$(esc "$VOICE_URL")</string>
	<key>voiceToken</key><string></string>
	<key>kernelURL</key><string></string>
	<key>kernelToken</key><string></string>
	<key>hubURL</key><string></string>
	<key>hubToken</key><string></string>
</dict>
</plist>
EOF
"$(dirname "$0")/check-secrets.sh" "$OUT"
echo "wrote $OUT (voice=${VOICE_URL:-?})"

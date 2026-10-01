#!/bin/sh
# Guard before a phone bake: Secrets.plist must name a public wss voice server.
# Orb product only requires voiceServerURL (OpenAI key is typed in Settings).
set -eu
PLIST="${1:-$(dirname "$0")/../Arbos/Secrets.plist}"
fail() { echo "check-secrets: $1" >&2; exit 1; }

[ -f "$PLIST" ] || fail "$PLIST is missing — run ios/scripts/gen-secrets.sh"
# plutil is macOS; on Linux accept a plist that Python can parse.
if command -v plutil >/dev/null 2>&1; then
  plutil -lint "$PLIST" >/dev/null 2>&1 || fail "$PLIST does not parse"
  read_key() { plutil -extract "$1" raw -o - "$PLIST" 2>/dev/null || true; }
else
  read_key() {
    python3 - "$PLIST" "$1" <<'PY'
import plistlib, sys
p = plistlib.load(open(sys.argv[1], "rb"))
print(p.get(sys.argv[2], "") or "")
PY
  }
fi

VOICE_URL="$(read_key voiceServerURL)"
[ -n "$VOICE_URL" ] || fail "voiceServerURL is empty"
case "$VOICE_URL" in
  wss://*) ;;
  ws://*|http://*) fail "voiceServerURL is plain ($VOICE_URL) — a phone build carries only wss://" ;;
  *) fail "voiceServerURL has no wss:// scheme ($VOICE_URL)" ;;
esac
HOST="$(printf '%s' "$VOICE_URL" | sed -e 's#^wss://##' -e 's#[/:].*$##')"
case "$HOST" in
  127.*|localhost|0.0.0.0|::1|10.*|192.168.*|172.1[6-9].*|172.2[0-9].*|172.3[01].*|*.local|"")
    fail "voiceServerURL points at a local host ($HOST) — never for a phone build" ;;
esac
if command -v nslookup >/dev/null 2>&1; then
  nslookup "$HOST" >/dev/null 2>&1 || fail "voiceServerURL host does not resolve ($HOST)"
fi
echo "check-secrets: ok — voice host $HOST"

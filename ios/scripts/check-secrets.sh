#!/bin/sh
# The guard before any build meant for a phone: Secrets.plist must point at
# the pod hub, never at a loopback or plain-text hub someone wired for a
# test, and the hub must accept the token. Fails loudly; prints no values.
#
#   ios/scripts/check-secrets.sh [path-to-Secrets.plist]
#
# Checks, in order: the file exists and parses; hubURL is wss:// to a public
# host (no 127.0.0.1, localhost, 10/172.16/192.168 addresses, .local names,
# or ws://); hubToken is present and is not a test token; GET /list with
# the token answers 200 and names at least one machine.
set -eu
PLIST="${1:-$(dirname "$0")/../Arbos/Secrets.plist}"
fail() { echo "check-secrets: $1" >&2; exit 1; }

[ -f "$PLIST" ] || fail "$PLIST is missing — run ios/scripts/gen-secrets.sh"
plutil -lint "$PLIST" >/dev/null 2>&1 || fail "$PLIST does not parse"

read_key() { plutil -extract "$1" raw -o - "$PLIST" 2>/dev/null || true; }
HUB_URL="$(read_key hubURL)"
HUB_TOKEN="$(read_key hubToken)"

[ -n "$HUB_URL" ] || fail "hubURL is empty"
case "$HUB_URL" in
  wss://*) ;;
  ws://*|http://*) fail "hubURL is plain ($HUB_URL) — a phone build carries only wss://" ;;
  *) fail "hubURL has no wss:// scheme ($HUB_URL)" ;;
esac
HOST="$(printf '%s' "$HUB_URL" | sed -e 's#^wss://##' -e 's#[/:].*$##')"
case "$HOST" in
  127.*|localhost|0.0.0.0|::1|10.*|192.168.*|172.1[6-9].*|172.2[0-9].*|172.3[01].*|*.local|"")
    fail "hubURL points at a local hub ($HOST) — never for a phone build" ;;
esac

[ -n "$HUB_TOKEN" ] || fail "hubToken is empty"
[ "${#HUB_TOKEN}" -ge 16 ] || fail "hubToken is too short to be a hub token"
case "$HUB_TOKEN" in
  *local*|*test*) fail "hubToken looks like a test token" ;;
esac

# The hub itself is the last word: the token must be accepted there.
LIST_URL="$(printf '%s' "$HUB_URL" | sed -e 's#^wss://#https://#' -e 's#/*$##')/list"
STATUS="$(curl -sS --max-time 15 -o /tmp/check-secrets-list.json -w '%{http_code}' \
  -H "Authorization: Bearer $HUB_TOKEN" "$LIST_URL" 2>/dev/null || echo 000)"
[ "$STATUS" = "200" ] || fail "the hub at $HOST answered $STATUS to the baked token (want 200)"
MACHINES="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(" ".join(m["name"] for m in d.get("machines", [])))' /tmp/check-secrets-list.json 2>/dev/null || true)"
rm -f /tmp/check-secrets-list.json
[ -n "$MACHINES" ] || fail "the hub at $HOST lists no machines for the baked token"
echo "check-secrets: ok — hub $HOST accepts the token; machines: $MACHINES"

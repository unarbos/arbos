#!/bin/bash
# check3.sh <version> — release object, DMG (downloaded, digest-matched), feed:
# authority = appcast.xml asset on the GitHub release (and the latest release), mirror = arbos.life copy.
set -u
V=$1; T=v$V; R=unarbos/arbos; K=${MERGE_KIT:-/tmp/k}
echo "== $T"
gh api repos/$R/releases/tags/$T --jq '"release: draft=\(.draft) prerelease=\(.prerelease) published_at=\(.published_at) assets=\(.assets|length) url=\(.html_url)"' || echo "release: MISSING"
dmg=$(gh api repos/$R/releases/tags/$T --jq '.assets[]|select(.name|endswith(".dmg"))|"\(.name) \(.state) \(.size) \(.digest) \(.browser_download_url)"' 2>/dev/null)
echo "dmg asset: ${dmg:-MISSING}"
if [ -n "$dmg" ]; then
  url=$(echo "$dmg" | awk '{print $5}'); dig=$(echo "$dmg" | awk '{print $4}' | sed 's/^sha256://')
  curl -sSL -o "$K/check.dmg" "$url"
  got=$(sha256sum "$K/check.dmg" | cut -d' ' -f1); sz=$(stat -c %s "$K/check.dmg")
  [ "$got" = "$dig" ] && echo "dmg download: sha256 ${got:0:8}…${got: -6} size $sz = GitHub digest" || echo "dmg download: MISMATCH got $got want $dig size $sz"
  rm -f "$K/check.dmg"
fi
feedcheck() {  # <label> <url>
  local x top
  x=$(curl -sSL --max-time 30 -H 'Cache-Control: no-cache' -A arbos-check "$2") || { echo "$1: FETCH FAILED ($2)"; return; }
  top=$(echo "$x" | grep -o '<sparkle:shortVersionString>[^<]*' | head -1 | cut -d'>' -f2)
  if echo "$x" | grep -q "<sparkle:shortVersionString>$V</sparkle:shortVersionString>"; then
    signed=$(echo "$x" | python3 -c "
import sys,re
x=sys.stdin.read()
it=[i for i in re.findall(r'<item>.*?</item>', x, re.S) if '<sparkle:shortVersionString>$V<' in i]
print('signed' if it and 'sparkle:edSignature' in it[0] else 'UNSIGNED')")
    echo "$1: lists $V ($signed); top item $top"
  else
    echo "$1: does NOT list $V; top item ${top:-none}"
  fi
}
echo "latest release: $(gh api repos/$R/releases/latest --jq .tag_name)"
feedcheck "feed (authority, release asset)" "https://github.com/$R/releases/download/$T/appcast.xml"
feedcheck "feed (authority, latest/download)" "https://github.com/$R/releases/latest/download/appcast.xml"
feedcheck "feed (mirror, arbos.life)" "https://arbos.life/updates/mac/appcast.xml"
echo "download/mac -> $(curl -sS --max-time 30 -o /dev/null -w '%{redirect_url}' https://arbos.life/download/mac)"

#!/bin/bash
# check3.sh <version> — release object, DMG (downloaded, digest-matched), feed:
# authority = appcast.xml asset on the GitHub release (and the latest release), mirror = arbos.life copy.
set -u
V=$1; T=v$V; R=unarbos/arbos
echo "== $T"
# A release object that does not exist yet must read as ABSENT, never as a corrupt one. `gh api --jq` writes
# the error body to STDOUT on a 404, so both reads are taken through their EXIT STATUS and the body is thrown
# away. Without that, a 404 left $dmg holding `{"message":"Not Found",...}`: non-empty, so the download block
# below ran with an empty URL, curl refused it, sha256sum hashed nothing, and the line came out
#   dmg download: MISMATCH got e3b0c442… want  size 0
# — a DIGEST MISMATCH reported for a release that has not been published yet. That is the normal healthy
# state for most of a release run: release.yml makes the bump, the tag, the build and the GitHub release all
# inside one run, so the tag exists for the whole of the build and the release object appears only at the end.
# Seen against v0.6.391 at 20:19Z while its `Appcast and release` job was still running.
# The NO-GO is unchanged either way — check_published() needs the "= GitHub digest" line and there is none —
# so this costs no strictness. It only stops the reason being a lie about the release's integrity.
rel=$(gh api repos/$R/releases/tags/$T --jq '"release: draft=\(.draft) prerelease=\(.prerelease) published_at=\(.published_at) assets=\(.assets|length) url=\(.html_url)"' 2>/dev/null) || rel="release: MISSING"
echo "$rel"
dmg=$(gh api repos/$R/releases/tags/$T --jq '.assets[]|select(.name|endswith(".dmg"))|"\(.name) \(.state) \(.size) \(.digest) \(.browser_download_url)"' 2>/dev/null) || dmg=""
echo "dmg asset: ${dmg:-MISSING}"
if [ -n "$dmg" ]; then
  url=$(echo "$dmg" | awk '{print $5}'); dig=$(echo "$dmg" | awk '{print $4}' | sed 's/^sha256://')
  # A temp file, not $MERGE_KIT/check.dmg. With no kit directory on disk the download had nowhere to land,
  # sha256sum read nothing, and the line came out "MISMATCH" — a healthy release reported as a broken one
  # because of a missing directory. check_published() turns that line into a NO-GO.
  f=$(mktemp) || { echo "dmg download: cannot create a temporary file to download into"; exit 1; }
  curl -sSL -o "$f" "$url"
  got=$(sha256sum "$f" | cut -d' ' -f1); sz=$(stat -c %s "$f")
  [ "$got" = "$dig" ] && echo "dmg download: sha256 ${got:0:8}…${got: -6} size $sz = GitHub digest" || echo "dmg download: MISMATCH got $got want $dig size $sz"
  rm -f "$f"
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

#!/bin/bash
# check3.sh <version> — release object, DMG (downloaded, digest-matched), feed:
# authority = appcast.xml asset on the GitHub release (and the latest release), mirror = arbos.life copy.
#
# THE DMG IS STILL DOWNLOADED IN FULL AND STILL HASHED. What changed (AF-1475) is that a release
# whose artifact has already been verified at this exact asset identity is not downloaded AGAIN on
# every sample. See verify-cache.sh for the key and for what it does and does not assert; the
# short version is that the identity comes from the release API on every run, so a changed asset
# changes the key and gets the full download. Nothing here answers the digest question from
# metadata.
#
# WHY THAT WAS WORTH DOING. merge-window.sh's check_published() runs this script, and several
# lanes sample it at once during a drain. Each sample re-read the whole 70 MB DMG and re-fetched
# the feed, so thirteen samples of one healthy release moved the better part of a gigabyte — and
# every one of those reads was a fresh chance for a transport failure to be reported as a fact
# about the release. On v0.6.405 the gate answered NO-GO on eight of thirteen samples, alternating
# between the DMG and the authority feed, while every release.yml run for the tag read
# completed/success. That is a failing READ of a healthy artifact, and sampling made it likelier,
# because sampling was the thing generating the load.
set -u
D=$(cd "$(dirname "$0")" && pwd)
# THE CACHE IS AN OPTIMISATION, SO LOSING IT MUST COST DOWNLOADS AND NEVER A VERDICT.
#
# Without these stubs the script still works when verify-cache.sh is missing — every vc_ call is
# reported as "command not found", vc_lookup's failure reads as a miss, and the full download and
# hash runs — but it works by accident, and it fills a release gate's output with error lines that
# look like a statement about the release. The other direction, refusing when the module cannot be
# loaded, would let deleting one file shut the merge queue, which is the shape of defect this kit
# has paid for repeatedly. So: say it once, name the consequence, carry on verifying in full.
#
# The stubs are defined AFTER the source on purpose. A file that loads halfway — a syntax error
# part way down — would otherwise leave some real functions and some missing ones; overwriting all
# four means a partial load degrades to the same full-verification path as no load at all.
# shellcheck source=scripts/merge/verify-cache.sh
if ! source "$D/verify-cache.sh" 2>/dev/null; then
  echo "verify cache: UNAVAILABLE ($D/verify-cache.sh could not be sourced), so every sample downloads and hashes the artifact in full — slower, never weaker"
  vc_key() { :; }
  vc_lookup() { return 1; }
  # 0, not 1: the line above has already said that nothing will be remembered, and a per-read
  # "could not be cached" on top of it would repeat the same fact once per check.
  vc_store() { return 0; }
  vc_age_note() { :; }
fi
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
# The identity fields are read on EVERY sample and six of them go into the cache key, so the whole
# of what GitHub says about this asset has to change for nothing, for an older verification to keep
# answering. .updated_at and .size are in the key beside .digest on purpose: they are a second and
# third way to notice a re-upload, and they cost nothing because this read is made anyway. The
# field ORDER matters to the two awk positions below, which is why updated_at is appended last.
dmg=$(gh api repos/$R/releases/tags/$T --jq '.assets[]|select(.name|endswith(".dmg"))|"\(.name) \(.state) \(.size) \(.digest) \(.browser_download_url) \(.updated_at)"' 2>/dev/null) || dmg=""
echo "dmg asset: ${dmg:-MISSING}"
if [ -n "$dmg" ]; then
  url=$(echo "$dmg" | awk '{print $5}'); dig=$(echo "$dmg" | awk '{print $4}' | sed 's/^sha256://')
  key=$(vc_key "$R" "$T" dmg "$(echo "$dmg" | awk '{print $1, $2, $3, $4, $6}')")
  if hit=$(vc_lookup "$key"); then
    echo "${hit#* } ($(vc_age_note "$T DMG" "${hit%% *}"))"
  else
    # A temp file, not $MERGE_KIT/check.dmg. With no kit directory on disk the download had nowhere to land,
    # sha256sum read nothing, and the line came out "MISMATCH" — a healthy release reported as a broken one
    # because of a missing directory. check_published() turns that line into a NO-GO.
    f=$(mktemp) || { echo "dmg download: cannot create a temporary file to download into"; exit 1; }
    curl -sSL -o "$f" "$url"; crc=$?
    got=$(sha256sum "$f" | cut -d' ' -f1); sz=$(stat -c %s "$f")
    # curl's exit status, which this block used to throw away. A dropped or timed-out transfer left
    # a short file behind, sha256sum hashed it, and the line came out as "MISMATCH got … want …" —
    # a statement that the release is serving the wrong bytes, made on the strength of a read that
    # failed. It is the same lie the 404 comment above is about, and it is the one the operator
    # acts on: a mismatch sends them to look at the release, a failed transfer sends them to look
    # at the network. THE NO-GO IS UNCHANGED either way, because check_published() needs the
    # "= GitHub digest" line and neither of these produces one, so saying which happened costs no
    # strictness.
    if [ $crc -ne 0 ]; then
      echo "dmg download: FETCH FAILED (curl rc=$crc after $sz of $(echo "$dmg" | awk '{print $3}') bytes) — the read failed, so nothing is claimed about the artifact's bytes"
    elif [ "$got" = "$dig" ]; then
      line="dmg download: sha256 ${got:0:8}…${got: -6} size $sz = GitHub digest"
      echo "$line"
      # Only here, and only for a full download whose hash matched. A mismatch and a failed fetch
      # are both re-done on the next sample.
      vc_store "$key" "$line" || echo "dmg download: (verified, but the verdict could not be cached, so the next sample downloads it again)"
    else
      echo "dmg download: MISMATCH got $got want $dig size $sz"
    fi
    rm -f "$f"
  fi
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
# WHICH OF THE FOUR READS BELOW IS CACHEABLE ON THE SAME KEY, AND WHY THE OTHER THREE ARE NOT.
#
# The key is "this release tag, this asset of it, at this identity". A read is cacheable on it only
# when its answer is a function of nothing else.
#
#   feed (authority, release asset)  — CACHED. The URL names this tag and resolves to appcast.xml
#     as published on THIS release, so the answer is a function of that asset's bytes and of $V,
#     and $V comes from the tag that is already in the key. It is the read check_published()
#     actually asserts on, and appcast.xml's digest arrives in the very release listing the DMG's
#     identity comes from, so keying it costs no extra request. Verified once per release, like the
#     DMG, and the same way: the feed is fetched in full and parsed in full on a miss.
#
#   feed (authority, latest/download) — NOT CACHEABLE, and not forced. Its URL goes through
#     releases/latest, so what it returns is a function of WHICH RELEASE IS LATEST and not of this
#     tag's assets at all. Publishing any newer release changes the answer while every field of
#     this key stays identical, so a cached hit would be a stale answer that the key cannot
#     possibly invalidate. That is the one thing a cache must not be.
#
#   feed (mirror, arbos.life) — NOT CACHEABLE. A third-party copy. No digest for it is published
#     anywhere in the release, so there is no identity to key on, and its whole purpose is to
#     answer "has the mirror caught up yet" — a question whose answer is expected to change over
#     time with this key fixed. check_published() already treats it as soft for that reason.
#
#   latest release / download/mac — NOT CACHEABLE, same reason as latest/download: both are live
#     facts about the repository and the site, not about an asset of this tag.
#
# Both uncacheable feed reads are small, and neither is the cost this change is about: between them
# they are a few kilobytes against the DMG's seventy megabytes.
echo "latest release: $(gh api repos/$R/releases/latest --jq .tag_name)"

# feedcheck_once <label> <url> <asset identity, empty when it could not be read>
#
# With no identity it degrades to the plain feedcheck, which is the uncached behaviour: a release
# listing that cannot be read must cost a fetch, never a pass.
feedcheck_once() {
  local key line hit
  if [ -z "$3" ]; then feedcheck "$1" "$2"; return; fi
  key=$(vc_key "$R" "$T" feed "$3")
  if hit=$(vc_lookup "$key"); then
    echo "${hit#* } ($(vc_age_note "$T authority feed" "${hit%% *}"))"
    return
  fi
  line=$(feedcheck "$1" "$2")
  echo "$line"
  # Only a line that LISTS this version is cached. A FETCH FAILED is transient and a "does NOT list"
  # is a release still being assembled; caching either would stick a momentary state to the tag.
  case "$line" in *": lists $V "*) vc_store "$key" "$line" || echo "$1: (read, but the verdict could not be cached)" ;; esac
}
feedasset=$(gh api repos/$R/releases/tags/$T --jq '.assets[]|select(.name=="appcast.xml")|"\(.name) \(.state) \(.size) \(.digest) \(.updated_at)"' 2>/dev/null) || feedasset=""
feedcheck_once "feed (authority, release asset)" "https://github.com/$R/releases/download/$T/appcast.xml" "$feedasset"
feedcheck "feed (authority, latest/download)" "https://github.com/$R/releases/latest/download/appcast.xml"
feedcheck "feed (mirror, arbos.life)" "https://arbos.life/updates/mac/appcast.xml"
echo "download/mac -> $(curl -sS --max-time 30 -o /dev/null -w '%{redirect_url}' https://arbos.life/download/mac)"

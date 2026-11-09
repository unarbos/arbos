#!/bin/bash
# verify-cache-selftest.sh — the verify-once cache's checks on itself, and on check3.sh's use of it.
# Run it directly: scripts/merge/verify-cache-selftest.sh
#
# No network, no credentials, no git. check3.sh is driven against a FAKE gh and a FAKE curl on a
# sandbox PATH that carries nothing able to reach the network, which is the same technique
# selftest.sh block 12 uses for merge.sh. That absence is load-bearing twice: it keeps these cases
# hermetic, and it means a case that accidentally reached the real release would die rather than
# quietly measure something else.
#
# WHAT THESE CASES ARE FOR. A cache in front of a security check is a way to make a check stop
# happening while every visible symptom improves, so almost everything below is a NEGATIVE control:
# it presents a changed, missing, truncated, corrupt or unfetchable artifact to a warm cache and
# requires a refusal. The one positive case that matters is counted rather than asserted — the
# second sample of an unchanged healthy artifact must perform ZERO downloads — because an assertion
# that the line still says "= GitHub digest" passes just as happily against the old code.
set -u
D=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=scripts/merge/verify-cache.sh
source "$D/verify-cache.sh"

fail=0
ok()  { printf '  ok    %s\n' "$1"; }
bad() { printf '  FAIL  %s\n' "$1"; fail=1; }

work=$(mktemp -d) || exit 1
trap 'rm -rf "$work"' EXIT

# ---------------------------------------------------------------------------------------------
echo "vc_key — identity, and nothing else, decides the key:"
k1=$(vc_key unarbos/arbos v0.6.999 dmg "a.dmg uploaded 10 sha256:aa 2026-11-09T00:00:00Z")
k2=$(vc_key unarbos/arbos v0.6.999 dmg "a.dmg uploaded 10 sha256:aa 2026-11-09T00:00:00Z")
[ "$k1" = "$k2" ] && ok "the same identity gives the same key" || bad "the same identity gave two keys"
vc_valid_key "$k1" && ok "a key is a bare sha256 digest" || bad "vc_key produced something that is not a sha256 digest: $k1"
for changed in \
  "unarbos/arbos|v0.6.999|dmg|a.dmg uploaded 10 sha256:bb 2026-11-09T00:00:00Z|the recorded digest" \
  "unarbos/arbos|v0.6.999|dmg|a.dmg uploaded 11 sha256:aa 2026-11-09T00:00:00Z|the recorded size" \
  "unarbos/arbos|v0.6.999|dmg|a.dmg uploaded 10 sha256:aa 2026-11-09T00:00:01Z|the recorded updated_at" \
  "unarbos/arbos|v0.6.999|dmg|b.dmg uploaded 10 sha256:aa 2026-11-09T00:00:00Z|the asset name" \
  "unarbos/arbos|v0.7.000|dmg|a.dmg uploaded 10 sha256:aa 2026-11-09T00:00:00Z|the release tag" \
  "unarbos/other|v0.6.999|dmg|a.dmg uploaded 10 sha256:aa 2026-11-09T00:00:00Z|the repository" \
  "unarbos/arbos|v0.6.999|feed|a.dmg uploaded 10 sha256:aa 2026-11-09T00:00:00Z|which read it answers"; do
  IFS='|' read -r a b c e label <<< "$changed"
  if [ "$(vc_key "$a" "$b" "$c" "$e")" = "$k1" ]; then
    bad "changing $label did not change the key — an older verification would keep answering"
  else ok "changing $label changes the key"; fi
done
# NUL separation, asserted where a space-joined key would collide. Both calls spell the same string
# when the fields are joined with a space, and they are different identities.
if [ "$(vc_key x "a b" c)" = "$(vc_key x a "b c")" ]; then
  bad "two different field splits produce one key — the fields are joined on a separator a field value can contain"
else ok "a field value cannot impersonate a field boundary"; fi

# ---------------------------------------------------------------------------------------------
echo "vc_valid_key — a key can never be a path:"
for k in "" "abc" "../../etc/passwd" "a/b" "$(printf 'g%.0s' $(seq 1 64))" "$(printf '0%.0s' $(seq 1 63))" "$(printf '0%.0s' $(seq 1 65))" "00 11"; do
  if vc_valid_key "$k"; then bad "'$k' was accepted as a key"; else ok "'${k:0:24}' is rejected"; fi
done
vc_valid_key "$(printf '0%.0s' $(seq 1 64))" && ok "64 hex characters are accepted" || bad "a real sha256 digest was rejected"

# ---------------------------------------------------------------------------------------------
echo "vc_lookup — every way an entry can be wrong fails towards the full verification:"
cache=$work/cache; export MW_VERIFY_CACHE=$cache
key=$(vc_key probe)
line='dmg download: sha256 aabbccdd…eeffaa size 70414309 = GitHub digest'
vc_store "$key" "$line" || bad "vc_store refused to write a first entry"
got=$(vc_lookup "$key") && [ "${got#* }" = "$line" ] \
  && ok "a freshly written PASS entry is returned verbatim" \
  || bad "a freshly written entry did not come back: '$got'"
# The timestamp travels on the line and not in a variable, because every caller reads this through
# a command substitution and a variable set in a subshell never comes back out. check3.sh printed
# "(cached)" with no date for exactly that reason before this was changed.
case "${got%% *}" in
  "" | *[!0-9]*) bad "vc_lookup does not lead with the entry's timestamp, so a caller cannot say how old the answer is: '$got'" ;;
  *) ok "the hit leads with the timestamp the entry was written at" ;;
esac

entry=$cache/$key
# A half-written entry, in the two shapes a killed writer or a full disk leaves behind: the payload
# with no sum line at all, and the payload with a sum line cut short. Neither may read as a PASS.
cp "$entry" "$work/good"
sed '/^sum /d' "$work/good" > "$entry"
vc_lookup "$key" >/dev/null && bad "an entry with no sum line read as a PASS" || ok "an entry with no sum line is a MISS"
{ sed '/^sum /d' "$work/good"; printf 'sum 0123456789abcdef\n'; } > "$entry"
vc_lookup "$key" >/dev/null && bad "an entry with a truncated sum read as a PASS" || ok "an entry with a truncated sum is a MISS"
head -c 40 "$work/good" > "$entry"
vc_lookup "$key" >/dev/null && bad "an entry cut off mid-payload read as a PASS" || ok "an entry cut off mid-payload is a MISS"
: > "$entry"
vc_lookup "$key" >/dev/null && bad "an empty entry read as a PASS" || ok "an empty entry is a MISS"

# A verdict edited in place without recomputing the sum. This is the one that matters most: the
# entry is the thing that authorises a pass, so it must not be forgeable by anyone who can write a
# file in the cache directory without also being able to recompute its checksum.
sed 's/= GitHub digest/= GitHub digest FORGED/' "$work/good" > "$entry"
vc_lookup "$key" >/dev/null && bad "an edited verdict read as a PASS — the checksum does not cover it" || ok "an edited verdict is a MISS"

# An entry recorded against a DIFFERENT key, moved into this key's filename. The filename alone
# must not carry the claim about which question the entry answers.
other=$(vc_key something-else)
vc_store "$other" "$line"
cp "$cache/$other" "$entry"
vc_lookup "$key" >/dev/null && bad "an entry recorded against another key answered this one" || ok "an entry states the key it answers for, and it is checked"

# Expiry. An entry authorises a pass for a bounded time and never indefinitely, which is what
# bounds the one exposure a digest-keyed cache has: bytes changing under a fixed recorded digest.
# A genuinely backdated entry, and not the zero-second boundary, because an entry written in this
# same second would pass a TTL case that had nothing to do with expiry.
reseal() {  # <written epoch> — rewrite the entry with that timestamp and a correct checksum
  local body; body=$(sed "s/^written .*/written $1/" "$work/good" | sed '/^sum /d')
  { printf '%s\n' "$body"; printf 'sum %s\n' "$(printf '%s\n' "$body" | sha256sum | cut -d' ' -f1)"; } > "$entry"
}
reseal $(( $(date -u +%s) - 7200 ))
MW_VERIFY_TTL=3600 vc_lookup "$key" >/dev/null && bad "an entry two hours old read as a PASS against a one-hour TTL" || ok "an entry past its TTL is a MISS"
MW_VERIFY_TTL=86400 vc_lookup "$key" >/dev/null && ok "that same two-hour-old entry is a hit inside a one-day TTL" || bad "a two-hour-old entry was refused by a one-day TTL"
reseal "$(date -u +%s)"
MW_VERIFY_TTL=0 vc_lookup "$key" >/dev/null && bad "MW_VERIFY_TTL=0 still served a hit, so there is no way to force re-verification by expiry" || ok "MW_VERIFY_TTL=0 expires every entry"
cp "$work/good" "$entry"
# A clock that moved backwards makes the recorded time meaningless, so it cannot be trusted to
# prove freshness either.
sed "s/^written .*/written $(( $(date -u +%s) + 7200 ))/" "$work/good" > "$work/future"
sum=$(sed '/^sum /d' "$work/future" | sha256sum | cut -d' ' -f1)
{ sed '/^sum /d' "$work/future"; printf 'sum %s\n' "$sum"; } > "$entry"
vc_lookup "$key" >/dev/null && bad "an entry stamped in the future read as a PASS" || ok "an entry stamped in the future is a MISS"
cp "$work/good" "$entry"

# Only a PASS is stored, and a non-PASS status is not honoured even if one appears.
sed 's/^status PASS/status FAIL/' "$work/good" > "$work/f"
sum=$(sed '/^sum /d' "$work/f" | sha256sum | cut -d' ' -f1)
{ sed '/^sum /d' "$work/f"; printf 'sum %s\n' "$sum"; } > "$entry"
vc_lookup "$key" >/dev/null && bad "a non-PASS entry read as a PASS" || ok "only status PASS is honoured"
cp "$work/good" "$entry"

vc_lookup "$(vc_key never-written)" >/dev/null && bad "an absent entry read as a PASS" || ok "an absent entry is a MISS"
MW_VERIFY_NOCACHE=1 vc_lookup "$key" >/dev/null && bad "MW_VERIFY_NOCACHE=1 still served a hit" || ok "MW_VERIFY_NOCACHE=1 forces a MISS"
# An unusable cache directory must cost a download, not a pass.
MW_VERIFY_CACHE=/proc/nowhere/cache vc_lookup "$key" >/dev/null \
  && bad "an unusable cache directory produced a hit" || ok "an unusable cache directory is a MISS"
vc_store "$key" "two
lines" && bad "a multi-line verdict was stored, so a reader would get a truncated answer back" \
        || ok "a multi-line verdict is refused at store time"

# ---------------------------------------------------------------------------------------------
# Concurrency. The promise is that a reader never observes an entry being written, so it either
# misses or gets a complete PASS — never a partial one. The design that makes that true is a single
# mv out of a temporary file created IN the cache directory, so the move is a rename inside one
# filesystem and not a copy. Both halves are checked: the behaviour, and the line that makes the
# behaviour true, because the behaviour passes on this filesystem even when the line is wrong.
echo "concurrency — writers publish by rename, readers never see a partial entry:"
if sed -n '/^vc_store()/,/^}/p' "$D/verify-cache.sh" | grep -q 'mktemp "$dir/'; then
  ok "the temporary file is created in the cache directory, so mv is a rename and not a copy"
else
  bad "vc_store no longer makes its temporary file inside the cache directory — mv across filesystems copies, and the destination is then observable half-written"
fi
if sed -n '/^vc_store()/,/^}/p' "$D/verify-cache.sh" | grep -q 'mv -f "$tmp" "$dir/$key"'; then
  ok "the entry is published by one mv"
else
  bad "vc_store no longer publishes with a single mv, so a reader can observe the entry being written"
fi
export MW_VERIFY_CACHE=$work/race
ckey=$(vc_key race-probe)
for i in $(seq 1 24); do
  ( vc_store "$ckey" "dmg download: sha256 writer$i = GitHub digest" ) &
done
badread=0; reads=0
for _ in $(seq 1 400); do
  r=$(vc_lookup "$ckey") && {
    reads=$((reads + 1))
    # A hit is "<timestamp> <verdict>", so both halves are checked: a partial entry would show up
    # as a verdict cut short, and a torn one as a timestamp that is not a number.
    case "${r%% *}" in "" | *[!0-9]*) badread=$((badread + 1)); continue ;; esac
    case "${r#* }" in "dmg download: sha256 writer"*" = GitHub digest") ;; *) badread=$((badread + 1)) ;; esac
  }
done
wait
if [ "$badread" -eq 0 ]; then
  ok "$reads hit(s) taken against 24 concurrent writers, and every one was a complete verdict"
else
  bad "$badread of $reads concurrent reads returned a partial or malformed verdict"
fi
vc_lookup "$ckey" >/dev/null && ok "the entry is intact after 24 concurrent writers" || bad "the entry is unreadable after concurrent writes"
stray=$(find "$work/race" -name '.entry.*' | wc -l)
[ "$stray" -eq 0 ] && ok "no temporary file is left behind" || bad "$stray temporary file(s) left in the cache directory"
unset MW_VERIFY_CACHE

# ---------------------------------------------------------------------------------------------
# check3.sh, end to end, against a fake release. Everything above tests the cache alone; a cache
# that is correct and unreached would pass all of it.
echo
echo "check3.sh — the wiring, measured in downloads rather than asserted in prose:"
sandbox=$work/sandbox
mkdir -p "$sandbox/bin" "$sandbox/count"
for t in bash dirname sha256sum stat mktemp awk sed grep head cut python3 date rm chmod mv mkdir cat tr sort wc find seq touch; do
  p=$(command -v "$t") && ln -sf "$p" "$sandbox/bin/$t" || bad "no $t on PATH, so the sandbox cannot be built"
done
cat > "$sandbox/bin/gh" <<'GH'
#!/bin/bash
# The release API, stubbed. Dispatches on the jq expression check3.sh actually sends, so a case
# that stops asking the question it means to ask gets no answer rather than a wrong one.
path=""; jqx=""
while [ $# -gt 0 ]; do
  case "$1" in repos/*) path=$1 ;; --jq) shift; jqx=${1:-} ;; esac
  shift
done
case "$path" in */releases/latest) printf '%s\n' "${STUB_LATEST:-}"; exit 0 ;; esac
case "$jqx" in
  *'release: draft='*)  [ -n "${STUB_REL:-}" ] || exit 1; printf '%s\n' "$STUB_REL"; exit 0 ;;
  *'endswith(".dmg")'*) [ -n "${STUB_DMG:-}" ] && printf '%s\n' "$STUB_DMG"; exit 0 ;;
  *'"appcast.xml"'*)    [ -n "${STUB_FEEDASSET:-}" ] && printf '%s\n' "$STUB_FEEDASSET"; exit 0 ;;
esac
exit 1
GH
cat > "$sandbox/bin/curl" <<'CURL'
#!/bin/bash
# Every transfer is recorded in $STUB_COUNT, which is how "the cache saved a download" is measured
# rather than argued. $STUB_DMG_BODY_FILE is what the CDN serves and $STUB_DMG is what GitHub
# records about it; the two are set independently on purpose, because that is the tamper case. The
# served bytes go through a FILE and not the environment: a megabyte of fixture in an exported
# variable overflows the exec argument limit, and every tool on the sandbox PATH then fails with
# "Argument list too long" while the cases read the empty results as findings.
out=""; url=""
while [ $# -gt 0 ]; do
  case "$1" in -o) shift; out=${1:-} ;; http*) url=$1 ;; esac
  shift
done
body=""
case "$url" in
  *.dmg)
    echo "$url" >> "$STUB_COUNT/dmg"
    # The bytes are written BEFORE the exit status is applied, so a case can hold the two apart:
    # $STUB_DMG_FETCH_RC=28 with the full body asks "is curl's status read at all", and the same rc
    # with a short body is the live failure, where the fragment hashes to something and the old code
    # reported that as a fact about the release.
    [ -n "$out" ] && cat "${STUB_DMG_BODY_FILE:-/dev/null}" > "$out"
    exit "${STUB_DMG_FETCH_RC:-0}" ;;
  */releases/download/*/appcast.xml)        echo x >> "$STUB_COUNT/feed";       body=${STUB_FEED_BODY:-} ;;
  */releases/latest/download/appcast.xml)   echo x >> "$STUB_COUNT/latestfeed"; body=${STUB_FEED_BODY:-} ;;
  *arbos.life/updates/mac/appcast.xml)      echo x >> "$STUB_COUNT/mirror";     body=${STUB_FEED_BODY:-} ;;
  *arbos.life/download/mac) printf 'https://example.invalid/stub.dmg'; exit 0 ;;
  *) echo "FAKE CURL: unexpected url $url" >&2; exit 7 ;;
esac
if [ -n "$out" ]; then printf '%s' "$body" > "$out"; else printf '%s' "$body"; fi
CURL
chmod +x "$sandbox/bin/gh" "$sandbox/bin/curl"

VER=0.6.999; TAG=v$VER
export STUB_COUNT=$sandbox/count
export STUB_LATEST=$TAG
export STUB_REL="release: draft=false prerelease=false published_at=2026-11-09T07:05:53Z assets=7 url=https://github.com/unarbos/arbos/releases/tag/$TAG"
export STUB_FEED_BODY="<rss><channel><item><sparkle:shortVersionString>$VER</sparkle:shortVersionString><enclosure sparkle:edSignature=\"stub\"/></item></channel></rss>"
export STUB_DMG_FETCH_RC=0
export STUB_DMG STUB_DMG_BODY_FILE STUB_FEEDASSET

# A quarter of a megabyte of fixture, in files, so "the whole thing was read" is a number and not a
# word. Files rather than variables: see the note in the fake curl above.
head -c 262144 /dev/zero | tr '\0' 'A' > "$sandbox/good.bin"
head -c 262144 /dev/zero | tr '\0' 'B' > "$sandbox/other.bin"
head -c 1000 "$sandbox/good.bin" > "$sandbox/short.bin"
good_sha=$(sha256sum "$sandbox/good.bin" | cut -d' ' -f1); good_len=$(stat -c %s "$sandbox/good.bin")
other_sha=$(sha256sum "$sandbox/other.bin" | cut -d' ' -f1)

asset() {  # <digest> <size> [updated_at]
  printf '%s uploaded %s sha256:%s https://github.com/unarbos/arbos/releases/download/%s/Arbos-%s-macos-universal.dmg %s' \
    "Arbos-$VER-macos-universal.dmg" "$2" "$1" "$TAG" "$VER" "${3:-2026-11-09T07:05:53Z}"
}
feedasset() { printf 'appcast.xml uploaded %s sha256:%s %s' "${#STUB_FEED_BODY}" "$1" "${2:-2026-11-09T07:05:50Z}"; }
reset() {  # a cold cache, zeroed counters and a healthy release
  rm -rf "$sandbox/cache" "$sandbox/count" "$sandbox/gatecount"; mkdir -p "$sandbox/count"
  export MW_VERIFY_CACHE=$sandbox/cache
  fixture_healthy
}
dl()   { local f=$STUB_COUNT/${1:-dmg}; [ -f "$f" ] && grep -c . "$f" || echo 0; }
# The two knobs are named in the prefix rather than inherited, because a prefix assignment before a
# FUNCTION call is exported by bash but not by every shell, and a case whose mechanism depends on
# that is a case whose result depends on who is running it.
c3()   { PATH=$sandbox/bin MW_VERIFY_TTL=${MW_VERIFY_TTL:-86400} MW_VERIFY_NOCACHE=${MW_VERIFY_NOCACHE:-0} \
           bash "$D/check3.sh" "$VER" 2>&1; }

# check_published(), lifted out of the shipped merge-window.sh and evaluated — not retyped. This is
# selftest.sh's technique for nomac_covers and publish_job, and the reason is the same: a copy here
# would leave every mutation of the real function invisible, so the end-to-end cases below would
# pass against a gate that no longer refuses anything.
cp_src=$(sed -n '/^check_published() {/,/^}/p' "$D/merge-window.sh")
if [ -z "$cp_src" ]; then
  bad "no check_published() in merge-window.sh — the end-to-end cases below are testing nothing"
else
  eval "$cp_src"
fi
gate() {  # the kit's own verdict for this tag, from the shipped function
  # Its own counter directory. gate() runs check3.sh, so without this every verdict reading would
  # add a download to the counts the cases beside it assert on — and the first draft of this file
  # did exactly that, reporting "3 downloads for 2 samples" for a working cache.
  ( PATH=$sandbox/bin; export PATH; STUB_COUNT=$sandbox/gatecount; mkdir -p "$STUB_COUNT"
    MW_VERIFY_TTL=${MW_VERIFY_TTL:-86400}; export MW_VERIFY_TTL
    nogo=()
    check_published "$TAG" > "$sandbox/gate.out" 2>&1
    if [ ${#nogo[@]} -eq 0 ]; then echo "GO"; else echo "NO-GO — ${nogo[*]}"; fi )
}

# Both halves of the fixture, set together. The served bytes and the bytes GitHub claims to have
# are separate knobs because that is the tamper case, which also means a case that sets one and
# inherits the other from the case before it is testing something nobody chose. Three cases in the
# first draft of this file passed for exactly that reason.
fixture_healthy() {
  STUB_DMG_BODY_FILE=$sandbox/good.bin
  STUB_DMG_FETCH_RC=0
  STUB_DMG=$(asset "$good_sha" "$good_len")
  STUB_FEEDASSET=$(feedasset feed0)
  STUB_FEED_BODY="<rss><channel><item><sparkle:shortVersionString>$VER</sparkle:shortVersionString><enclosure sparkle:edSignature=\"stub\"/></item></channel></rss>"
}

# --- the miss path: the first call still does the real work ------------------------------------
reset
out=$(c3)
case "$out" in
  *"= GitHub digest"*) ok "a cold cache verifies and passes" ;;
  *) bad "a cold cache did not report a digest match: $(printf '%s' "$out" | grep '^dmg download')" ;;
esac
[ "$(dl)" -eq 1 ] && ok "the miss path performed exactly one download" || bad "the miss path performed $(dl) download(s)"
case "$out" in
  *"size $good_len = GitHub digest"*) ok "the miss path read all $good_len bytes and hashed them" ;;
  *) bad "the miss path did not report the full asset size, so it did not read the whole artifact: $(printf '%s' "$out" | grep '^dmg download')" ;;
esac
case "$out" in
  *"sha256 ${good_sha:0:8}"*) ok "the digest it reports is the hash it computed, not the one GitHub recorded" ;;
  *) bad "the reported sha256 is not the hash of the served bytes" ;;
esac
case "$out" in *cached*) bad "a cold-cache line claims to be cached" ;; *) ok "a cold-cache line does not claim to be cached" ;; esac
[ "$(gate)" = GO ] && ok "the kit's verdict on a healthy release is GO" || bad "a healthy release is not GO: $(gate)"

# --- THE POINT OF THE CHANGE: the second sample of an unchanged artifact downloads nothing -----
echo "the point of the change:"
out2=$(c3)
if [ "$(dl)" -eq 1 ]; then
  ok "a second sample of the same artifact performed no further download (1 download for 2 samples)"
else
  bad "a second sample downloaded the artifact again: $(dl) download(s) for 2 samples. A healthy artifact must be verified once per release, not once per sample — each re-download is 70 MB on the machine running the drain and a fresh chance for a transport failure to be reported as a fact about the release."
fi
case "$out2" in
  *"= GitHub digest"*) ok "the cached sample still reports the digest match the gate greps for" ;;
  *) bad "the cached sample lost the '= GitHub digest' line, so check_published() turns it into a NO-GO" ;;
esac
case "$out2" in
  *"cached: this $TAG DMG was fully downloaded and digest-matched at "*) ok "the cached line says it is cached, and when it was verified" ;;
  *) bad "the cached line is indistinguishable from a fresh one: $(printf '%s' "$out2" | grep '^dmg download')" ;;
esac
[ "$(gate)" = GO ] && ok "the kit's verdict from cache is still GO" || bad "the cached sample is not GO: $(gate)"
for _ in 1 2 3 4 5 6 7 8 9 10; do c3 >/dev/null; done
[ "$(dl)" -eq 1 ] && ok "12 samples, 1 download" || bad "12 samples cost $(dl) downloads"
# ...and the cache is what is saving them, not something else about the stub.
reset
for _ in 1 2 3; do MW_VERIFY_NOCACHE=1 c3 >/dev/null; done
[ "$(dl)" -eq 3 ] && ok "with MW_VERIFY_NOCACHE=1, 3 samples cost 3 downloads — the cache is what saves them" \
  || bad "with the cache disabled, 3 samples cost $(dl) download(s), so the count above is not measuring the cache"

# --- negative controls ------------------------------------------------------------------------
echo "negative controls — a warm cache must not pass a changed, missing, short or corrupt artifact:"

# 1. The control the brief names: a cached PASS for this tag must not satisfy a later sample whose
#    recorded digest differs. The CDN still serves the old bytes, so if the key did not include the
#    digest the entry would match and the sample would pass. It must miss, re-download, and refuse,
#    because the served bytes are not what GitHub now records.
reset; c3 >/dev/null; [ "$(dl)" -eq 1 ] || bad "fixture: the warm-up sample did not download once"
STUB_DMG=$(asset "$other_sha" "$good_len")
out=$(c3)
if [ "$(dl)" -eq 2 ]; then ok "a changed recorded digest misses the cache and re-downloads" \
  else bad "a changed recorded digest was answered from the cache: still $(dl) download(s)"; fi
case "$out" in
  *"= GitHub digest"*) bad "a cached PASS satisfied a sample whose recorded digest differs — this is the whole thing the key exists to prevent: $(printf '%s' "$out" | grep '^dmg download')" ;;
  *MISMATCH*) ok "it refuses, and says the served bytes do not match the recorded digest" ;;
  *) bad "neither a pass nor a mismatch: $(printf '%s' "$out" | grep '^dmg download')" ;;
esac
v=$(gate)
case "$v" in "NO-GO — $TAG DMG"*) ok "the kit's verdict is NO-GO, naming the DMG" ;; *) bad "the gate did not refuse a digest change: $v" ;; esac

# 2. A corrupted download that the verification actually performs. GitHub's record is unchanged, so
#    the digest is the one that would have passed; what changed is the bytes being served. Only a
#    full download and hash can see this, which is why it is still the verification.
reset
STUB_DMG_BODY_FILE=$sandbox/other.bin
out=$(c3)
case "$out" in
  *"= GitHub digest"*) bad "a corrupted download passed: $(printf '%s' "$out" | grep '^dmg download')" ;;
  *MISMATCH*) ok "a corrupted download is a MISMATCH" ;;
  *) bad "a corrupted download produced neither: $(printf '%s' "$out" | grep '^dmg download')" ;;
esac
v=$(gate); case "$v" in "NO-GO — $TAG DMG"*) ok "a corrupted download is NO-GO" ;; *) bad "a corrupted download is not NO-GO: $v" ;; esac
[ "$(find "$MW_VERIFY_CACHE" -type f 2>/dev/null | wc -l)" -eq 1 ] \
  && ok "the failing sample wrote no entry of its own (only the feed's)" \
  || bad "a failing sample wrote a cache entry, so the failure would be remembered as a pass"

# 2b. THE ONE EXPOSURE THIS DESIGN HAS, PINNED RATHER THAN HIDDEN.
#
#     Warm cache, every field GitHub reports about the asset unchanged, and the bytes at the CDN
#     swapped underneath it. The sample is answered from the entry and passes. That is not a bug in
#     the key and no key can close it: the entry's claim is "these bytes hashed to this digest at
#     this time", and re-asserting it for now means downloading seventy megabytes, which is the
#     defect this change exists to remove. Every metadata-keyed cache in front of a content check
#     has this shape, and the honest thing is to measure where the boundary is rather than write a
#     case that pretends it is elsewhere.
#
#     WHAT MAKES IT BOUNDED is MW_VERIFY_TTL: an entry authorises a pass for a limited time, never
#     indefinitely. So the assertion is the bound — the same corruption, past the TTL, is caught.
reset; c3 >/dev/null
STUB_DMG_BODY_FILE=$sandbox/other.bin
out=$(c3)
case "$out" in
  *"= GitHub digest"*cached*) ok "KNOWN AND BOUNDED: with GitHub's metadata unchanged, a warm entry answers a sample whose served bytes changed — no metadata-keyed cache can see this" ;;
  *) bad "the known exposure did not behave as documented, so the comment above is now wrong and the boundary is somewhere else: $(printf '%s' "$out" | grep '^dmg download')" ;;
esac
[ "$(dl)" -eq 1 ] && ok "...and it cost no download, which is what makes it the exposure rather than a check" \
  || bad "the warm sample downloaded after all ($(dl) in 2 samples), so the case above is measuring something else"
out=$(MW_VERIFY_TTL=0 c3)
case "$out" in
  *MISMATCH*) ok "THE BOUND HOLDS: past the TTL the same corruption is downloaded, hashed and refused" ;;
  *) bad "past the TTL the corruption was still not caught — the expiry is not bounding the exposure: $(printf '%s' "$out" | grep '^dmg download')" ;;
esac
v=$(MW_VERIFY_TTL=0 gate)
case "$v" in "NO-GO — $TAG DMG"*) ok "and the gate refuses it" ;; *) bad "past the TTL the gate still passed a corrupted artifact: $v" ;; esac

# 3. A truncated download: the bytes stop early. Same mechanism, stated on its own because "short"
#    is the shape a half-finished transfer takes and a size comparison would be the tempting fix.
reset
STUB_DMG_BODY_FILE=$sandbox/short.bin
out=$(c3)
case "$out" in
  *"= GitHub digest"*) bad "a truncated download passed" ;;
  *MISMATCH*size\ 1000*) ok "a truncated download is a MISMATCH, and the short size is reported" ;;
  *MISMATCH*) ok "a truncated download is a MISMATCH" ;;
  *) bad "a truncated download produced neither: $(printf '%s' "$out" | grep '^dmg download')" ;;
esac
v=$(gate); case "$v" in "NO-GO — $TAG DMG"*) ok "a truncated download is NO-GO" ;; *) bad "a truncated download is not NO-GO: $v" ;; esac

# 4. A failed transfer. The NO-GO is the same; the REASON must not be a claim about the artifact's
#    bytes, because that sends the operator to look at the release instead of at the network. This
#    is the misreport that produced the alternating NO-GOs on v0.6.405.
reset
STUB_DMG_FETCH_RC=28
out=$(c3)
case "$out" in
  *"= GitHub digest"*) bad "a failed transfer passed" ;;
  *"FETCH FAILED (curl rc=28"*) ok "a failed transfer says the read failed, and names curl's status" ;;
  *MISMATCH*) bad "a failed transfer is still reported as a digest MISMATCH — a statement about the release's integrity made on a read that did not complete" ;;
  *) bad "a failed transfer produced neither: $(printf '%s' "$out" | grep '^dmg download')" ;;
esac
case "$out" in *"nothing is claimed about the artifact's bytes"*) ok "the reason says what it is not claiming" ;;
  *) bad "the failed-transfer line does not say it is making no claim about the bytes" ;; esac
v=$(gate); case "$v" in "NO-GO — $TAG DMG"*) ok "a failed transfer is still NO-GO" ;; *) bad "a failed transfer is not NO-GO: $v" ;; esac
# ...and a failed transfer is not remembered, so the next sample tries again.
STUB_DMG_FETCH_RC=0
out=$(c3)
case "$out" in *"= GitHub digest"*) ok "the next sample re-verifies and passes, so a transient failure does not stick" ;;
  *) bad "a transient failure was remembered: $(printf '%s' "$out" | grep '^dmg download')" ;; esac

# 4b. The SHAPE v0.6.405 actually produced: the transfer fails AND leaves a short file behind. The
#     case above holds curl's status constant with complete bytes, to isolate "is the status read at
#     all"; this one is the live failure, where the old code hashed the fragment and reported the
#     result as a statement about the release's bytes.
reset
STUB_DMG_BODY_FILE=$sandbox/short.bin
STUB_DMG_FETCH_RC=28
out=$(c3)
case "$out" in
  *"= GitHub digest"*) bad "a failed, truncated transfer passed" ;;
  *"FETCH FAILED (curl rc=28 after 1000 of $good_len bytes)"*) ok "a failed, truncated transfer names the status and how far it got, and claims nothing about the bytes" ;;
  *MISMATCH*) bad "a failed, truncated transfer is reported as a digest MISMATCH — this is the v0.6.405 misreport, where a dropped download read as a statement that the release serves the wrong bytes" ;;
  *) bad "a failed, truncated transfer produced neither: $(printf '%s' "$out" | grep '^dmg download')" ;;
esac
v=$(gate); case "$v" in "NO-GO — $TAG DMG"*) ok "it still refuses, so the better reason costs no strictness" ;; *) bad "a failed, truncated transfer is not NO-GO: $v" ;; esac
[ "$(find "$MW_VERIFY_CACHE" -type f 2>/dev/null | grep -c .)" -le 1 ] \
  && ok "and it is not remembered" || bad "a failed, truncated transfer was cached"

# 5. A missing asset. Nothing to key on and nothing to verify; the gate must refuse and the cache
#    must stay out of it.
reset; c3 >/dev/null
STUB_DMG=""
out=$(c3)
case "$out" in
  *"dmg asset: MISSING"*) ok "a missing asset reads as MISSING" ;;
  *) bad "a missing asset did not read as MISSING: $(printf '%s' "$out" | grep '^dmg asset')" ;;
esac
case "$out" in *"= GitHub digest"*) bad "a missing asset produced a digest-match line from the cache" ;;
  *) ok "a missing asset produces no digest-match line" ;; esac
v=$(gate); case "$v" in "NO-GO — $TAG DMG"*) ok "a missing asset is NO-GO" ;; *) bad "a missing asset is not NO-GO: $v" ;; esac

# 6. The tag. A PASS for one release must not answer for another, even byte-for-byte identical.
retag() {  # point the whole fixture at a different release, same bytes
  VER=$1; TAG=v$VER
  STUB_LATEST=$TAG
  STUB_REL="release: draft=false prerelease=false published_at=2026-11-09T07:05:53Z assets=7 url=https://github.com/unarbos/arbos/releases/tag/$TAG"
  fixture_healthy
}
reset; c3 >/dev/null
retag 0.7.000
c3 >/dev/null
[ "$(dl)" -eq 2 ] && ok "a different tag misses the cache and verifies on its own" \
  || bad "a different tag was answered by another release's entry: $(dl) download(s) for 2 tags"
retag 0.6.999

# --- the authority feed, and the three reads that are not cacheable on this key ----------------
echo "the authority feed is cached on the same key; the live reads are not:"
reset
c3 >/dev/null; c3 >/dev/null; c3 >/dev/null
[ "$(dl feed)" -eq 1 ] && ok "3 samples, 1 fetch of the tag's own appcast.xml" || bad "the authority release-asset feed was fetched $(dl feed) time(s) in 3 samples"
[ "$(dl latestfeed)" -eq 3 ] && ok "releases/latest/download/appcast.xml is fetched every sample — its answer depends on which release is latest, which this key cannot see" \
  || bad "the latest/download feed was fetched $(dl latestfeed) time(s) in 3 samples; it must not be cached on a key that cannot invalidate it"
[ "$(dl mirror)" -eq 3 ] && ok "the arbos.life mirror is fetched every sample — it has no published digest and its answer is expected to change" \
  || bad "the mirror feed was fetched $(dl mirror) time(s) in 3 samples"
out=$(c3)
case "$out" in
  *"feed (authority, release asset): lists $VER"*) ok "the cached feed line still starts with the text check_published() greps for" ;;
  *) bad "the cached feed line lost its prefix: $(printf '%s' "$out" | grep 'release asset')" ;;
esac
[ "$(gate)" = GO ] && ok "the gate is GO with the feed answered from cache" || bad "the cached feed is not GO: $(gate)"
# A re-uploaded appcast.xml re-reads, by the same key rule as the DMG.
STUB_FEEDASSET=$(feedasset feed1)
c3 >/dev/null
[ "$(dl feed)" -eq 2 ] && ok "a changed appcast.xml digest misses the cache and re-fetches" \
  || bad "a changed appcast.xml digest was answered from the cache: still $(dl feed) fetch(es)"
# A feed that does not list this version is never cached, so the gate does not remember a release
# that was still being assembled.
reset
STUB_FEEDASSET=$(feedasset feed2)
STUB_FEED_BODY="<rss><channel><item><sparkle:shortVersionString>0.0.1</sparkle:shortVersionString></item></channel></rss>"
c3 >/dev/null; c3 >/dev/null
[ "$(dl feed)" -eq 2 ] && ok "a feed that does not list this version is re-fetched rather than remembered" \
  || bad "a 'does NOT list' answer was cached: $(dl feed) fetch(es) in 2 samples"
v=$(gate); case "$v" in *"$TAG feed (authority)"*) ok "a feed that does not list this version is NO-GO" ;; *) bad "a feed missing this version is not NO-GO: $v" ;; esac

# --- several lanes at once, on a cold cache ----------------------------------------------------
echo "several lanes sampling at once:"
reset
for _ in 1 2 3 4 5 6; do ( c3 > "$sandbox/lane.$$.$RANDOM" 2>&1 ) & done
wait
lanes_ok=$(grep -l '= GitHub digest' "$sandbox"/lane.* 2>/dev/null | wc -l)
[ "$lanes_ok" -eq 6 ] && ok "all 6 concurrent lanes verified and passed" || bad "only $lanes_ok of 6 concurrent lanes passed"
rm -f "$sandbox"/lane.*
entries=$(find "$sandbox/cache" -type f ! -name '.entry.*' | wc -l)
[ "$entries" -eq 2 ] && ok "the cold-cache race leaves exactly the 2 entries it should (the DMG and the tag's feed)" \
  || bad "$entries cache entr(ies) after a 6-way race, expected 2"
stray=$(find "$sandbox/cache" -name '.entry.*' | wc -l)
[ "$stray" -eq 0 ] && ok "no half-written entry is left behind" || bad "$stray temporary file(s) left behind by the race"
out=$(c3)
case "$out" in *"= GitHub digest"*cached*|*cached*"= GitHub digest"*) ok "the sample after the race is served from a valid entry" ;;
  *) bad "the entry the race left is not usable: $(printf '%s' "$out" | grep '^dmg download')" ;; esac

# --- the wiring, which the cases above cannot see ----------------------------------------------
echo "check3.sh reaches the cache, and the full download is still the verification:"
c3body=$(grep -vE '^[[:space:]]*#' "$D/check3.sh")
for want in 'vc_lookup "$key"' 'vc_store "$key" "$line"' 'sha256sum "$f"' 'curl -sSL -o "$f" "$url"'; do
  if printf '%s\n' "$c3body" | grep -qF "$want"; then ok "check3.sh still contains: $want"
  else bad "check3.sh no longer contains '$want' — either the cache is unreached or the verification is no longer a full download and hash"; fi
done
# The substitution this change must never become. A HEAD request or a size-only comparison answers
# a different question from the one the gate asks, and either would look like a faster cache.
for forbidden in '\-I ' '\--head' '\-X HEAD' 'content-length'; do
  if printf '%s\n' "$c3body" | grep -qiE "$forbidden"; then
    bad "check3.sh now makes a HEAD-style or size-only request ($forbidden). The gate asks whether the bytes being served hash to the recorded digest; a HEAD answers whether something of about the right shape is there."
  else ok "check3.sh makes no HEAD-style request ($forbidden)"; fi
done
# The CALL form, not the bare name: check3.sh also carries a vc_store fallback stub for the case
# where verify-cache.sh cannot be sourced, and counting that as a call site would make this number
# meaningless.
stores=$(printf '%s\n' "$c3body" | grep -c 'vc_store "\$key"')
if [ "$stores" -eq 2 ]; then
  ok "vc_store is called in exactly two places, one per cacheable read"
else
  bad "vc_store is called $stores time(s) — a third would be a read whose answer this key cannot invalidate"
fi
lookups=$(printf '%s\n' "$c3body" | grep -c 'vc_lookup "\$key"')
[ "$lookups" -eq 2 ] && ok "vc_lookup is consulted in exactly those same two places" \
  || bad "vc_lookup is consulted $lookups time(s), which does not pair with the $stores store(s)"
# The store must sit on the matched branch and nowhere else. A vc_store reachable from the mismatch
# or fetch-failed arm would remember a refusal as a pass, which is the one unrecoverable mistake
# available here.
dmg_arm=$(sed -n '/elif \[ "\$got" = "\$dig" \]/,/^    else/p' "$D/check3.sh")
if printf '%s\n' "$dmg_arm" | grep -q 'vc_store'; then
  ok "the DMG's vc_store sits on the branch where the computed hash equalled the recorded digest"
else
  bad "the DMG's vc_store is not on the hash-matched branch — find out which branch it is on before this ships"
fi
for arm in 'FETCH FAILED' 'MISMATCH got'; do
  if sed -n "/$arm/p" "$D/check3.sh" | grep -q 'vc_store'; then
    bad "a vc_store appears on the '$arm' line — a refusal would be cached as a pass"
  else ok "no vc_store on the '$arm' path"; fi
done
# merge-window.sh must be untouched by this change: check_published()'s four greps are the contract
# the lines above have to keep satisfying, and verdict()'s branches are not this change's business.
if grep -q "grep -q '= GitHub digest'" "$D/merge-window.sh"; then
  ok "check_published() still greps for '= GitHub digest', which both the fresh and the cached line carry"
else
  bad "check_published()'s digest grep has changed — the cached line's wording was built to satisfy the old one"
fi

echo
if [ "$fail" -eq 0 ]; then echo "verify-cache selftest: PASS"; else echo "verify-cache selftest: FAIL"; fi
exit "$fail"

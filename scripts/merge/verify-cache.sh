#!/bin/bash
# verify-cache.sh — remember that one release artifact was verified, so a healthy artifact is
# verified ONCE PER RELEASE instead of once per sample. Functions only: sourcing this file reads
# nothing, writes nothing and reaches no network.
#
# WHAT THIS DOES NOT DO, because it is the only thing that matters about it.
#
# It does not verify anything and it does not make verification cheaper. The verification stays
# exactly what it was — download the whole asset, sha256 it, compare that against the digest
# GitHub records for the asset — and every cache MISS performs it in full. What this file removes
# is the REPETITION of a verification whose answer cannot have changed.
#
# A HEAD request or a size comparison would also have removed the repetition, and would have been
# a different check. "Is something of roughly the right shape being served" and "do the bytes
# being served hash to the recorded digest" are two questions, and only the second one is the
# question the release gate asks. Substituting the first would have narrowed the gate while every
# visible symptom improved, which is the shape of defect this merge kit has paid for repeatedly.
# So there is no cheap probe anywhere below. There is a key, and a key is not a check: it decides
# whether a previous full verification still answers this sample, and when it does not, the full
# verification runs.
#
# WHAT MAKES THE KEY SAFE TO TRUST.
#
# The caller builds the key out of the asset's own identity as GitHub reports it — the release
# tag, the asset name, its state, its size, its recorded sha256 digest and its updated_at — all
# read fresh on EVERY sample from the release API, which is a few hundred milliseconds against
# seventy megabytes. Change any one of those and the key changes, so the entry does not match and
# the artifact is downloaded and hashed again. An artifact that is replaced, re-uploaded,
# truncated at the source, or recorded under a different digest therefore cannot be answered by an
# older entry: the question being asked is not the question the entry answers.
#
# THE RESIDUAL EXPOSURE, STATED RATHER THAN LEFT FOR A READER TO FIND. A cache hit asserts that
# the bytes served at this URL hashed to this digest at the moment recorded in the entry. It does
# not re-assert it for now. If the CDN began serving different bytes while GitHub's recorded
# digest, size and updated_at all stayed fixed, a hit would not catch it until the entry expired.
# That is what MW_VERIFY_TTL bounds: an entry authorises a pass for a limited time and never
# indefinitely, so the exposure has an end even if nothing about the release ever changes again.
#
# ONLY A PASS IS EVER CACHED. A mismatch, a failed transfer, a missing asset and an unreadable
# release are all re-done on the next sample, because each of them can be transient and the gate's
# job on all of them is to refuse. Caching a refusal would make a momentary network failure stick
# to a healthy release, which is this defect pointed the other way.

# The cache directory, created on demand. Prints the path; rc 1 when it cannot be had.
#
# NOT $MERGE_KIT. The kit directory is frequently absent — that is the setup gap merge.sh exit
# code 5 exists for — and a cache that lives inside it would make "no kit on this host" into a
# reason the DMG check behaves differently. check3.sh already carries a comment about a missing
# kit directory turning a healthy release into a reported MISMATCH; this file does not add a
# second way for that to happen. MW_VERIFY_CACHE overrides it, which is also how the tests get a
# private cache per case.
vc_dir() {
  local d=${MW_VERIFY_CACHE:-${TMPDIR:-/tmp}/arbos-verify-cache}
  mkdir -p "$d" 2>/dev/null || return 1
  [ -w "$d" ] || return 1
  printf '%s' "$d"
}

# vc_key <field>... — the cache key for one artifact identity: sha256 over the fields.
#
# NUL-separated, so no field value can impersonate a field boundary. Joining on a space would let
# an asset literally named `a 0 1` collide with a different asset whose name, state and size
# happen to spell the same string.
vc_key() {
  local f
  { for f in "$@"; do printf '%s\0' "$f"; done; } | sha256sum | cut -d' ' -f1
}

# vc_valid_key <key> — 0 when <key> is a sha256 hex digest and nothing else.
#
# This is what keeps the key from being a path. Every entry is read and written at
# "$(vc_dir)/$key", so a key carrying a slash or a `..` would be a caller choosing a file outside
# the cache. Asserting the shape here makes that structural rather than a property of the two
# current call sites.
vc_valid_key() {
  case "${1:-}" in "" | *[!0-9a-f]*) return 1 ;; esac
  [ ${#1} -eq 64 ]
}

# vc_lookup <key> — print "<unix timestamp> <verdict line>" and return 0, ONLY for a complete,
# intact, unexpired PASS entry recorded against this exact key. Anything else returns 1 and prints
# nothing, which the caller must treat as "do the full verification".
#
# The timestamp is on the line and not in a variable because every caller reads this through a
# command substitution, and a variable set inside a subshell is lost on the way out. The first
# version of this file set VC_WRITTEN; check3.sh got an empty one every time and the cached lines
# came out saying "(cached)" with no date, which the selftest caught. Callers split on the first
# space: "${hit%% *}" is the timestamp, "${hit#* }" is the verdict.
#
# EVERY ONE OF THESE TESTS FAILS TOWARDS THE FULL VERIFICATION. There is no failure mode in which
# an unreadable, partial, foreign or stale entry produces a pass; the worst a broken cache can do
# is cost a download, which is what the tool did before this file existed.
vc_lookup() {
  local key=${1:-} dir file body want sum written age
  [ "${MW_VERIFY_NOCACHE:-}" != 1 ] || return 1
  vc_valid_key "$key" || return 1
  dir=$(vc_dir) || return 1
  file=$dir/$key
  [ -r "$file" ] || return 1
  # The payload is every line before `sum`, and `sum` covers exactly those bytes. A file cut short
  # anywhere — a writer killed mid-write on a filesystem where the rename below was not atomic, a
  # full disk, a truncating editor — either has no sum line at all or has one that does not match,
  # and both answers are a MISS. This is belt and braces over the atomic rename in vc_store, and
  # it is here because "a half-written entry must never read as a PASS" is the kind of promise
  # that should not rest on one filesystem's behaviour.
  body=$(sed '/^sum /,$d' "$file" 2>/dev/null) || return 1
  want=$(sed -n 's/^sum //p' "$file" 2>/dev/null | head -1)
  [ -n "$want" ] || return 1
  sum=$(printf '%s\n' "$body" | sha256sum | cut -d' ' -f1)
  [ "$sum" = "$want" ] || return 1
  printf '%s\n' "$body" | grep -qx 'version 1' || return 1
  # The entry states the key it answers for, and it must be the key that was asked. Without this
  # the filename alone would carry that claim, and a file moved, copied or restored under another
  # name would answer a question it was never asked.
  printf '%s\n' "$body" | grep -qxF "key $key" || return 1
  printf '%s\n' "$body" | grep -qx 'status PASS' || return 1
  written=$(printf '%s\n' "$body" | sed -n 's/^written //p' | head -1)
  case "$written" in "" | *[!0-9]*) return 1 ;; esac
  age=$(( $(date -u +%s) - written ))
  # A negative age means the clock moved, so the recorded time says nothing about how old this
  # answer is. Refusing it costs one download and keeps the expiry meaningful.
  [ "$age" -ge 0 ] || return 1
  # Strictly less than, so MW_VERIFY_TTL=0 is a clean "no entry is ever fresh enough" switch rather
  # than a setting that still honours an entry written in the same second.
  [ "$age" -lt "${MW_VERIFY_TTL:-86400}" ] || return 1
  local verdict
  verdict=$(printf '%s\n' "$body" | sed -n 's/^verdict //p' | head -1)
  [ -n "$verdict" ] || return 1
  printf '%s %s\n' "$written" "$verdict"
}

# vc_store <key> <verdict-line> — record that this artifact identity passed its full verification.
# rc 1 when the entry could not be written, which the caller ignores: an unwritable cache means
# the next sample verifies again, and that is the old behaviour rather than a new failure.
#
# CONCURRENCY. Several lanes sample this gate at once on one machine, so this is the part that had
# to be chosen rather than assumed.
#
#   PUBLISHING IS ONE RENAME. The entry is written to a temporary file and moved into place with a
#   single mv. A rename within one directory is atomic, so a concurrent reader opens either the
#   previous entry or the whole new one. It can never observe the file growing.
#
#   THE TEMPORARY FILE IS MADE IN THE CACHE DIRECTORY, not in $TMPDIR. This line is the reason the
#   paragraph above is true. mv across filesystems is not a rename: it copies and then unlinks,
#   and the destination path is observable part-written for the whole of the copy. Creating the
#   temporary file beside its destination keeps the move inside one filesystem whatever TMPDIR is
#   set to.
#
#   READERS TAKE NO LOCK, AND NEITHER DO WRITERS. Nothing here reads an entry in order to modify
#   it, so there is no update to lose: two lanes that verified the same artifact compute the same
#   key and write byte-identical payloads, and whichever rename lands last leaves the same entry.
#   That is worth more than it sounds. A lock file is a thing that can be held by a process that
#   died, and a wedged gate is the failure this merge kit keeps paying for — merge-window.sh's own
#   history is a list of single reads that shut the whole queue. A design with no lock has no
#   stale lock.
#
#   THE COLD-CACHE STAMPEDE IS DELIBERATELY NOT PREVENTED. When several lanes miss at once they
#   all download, once each. Single-flighting that behind a lock would save those downloads and
#   would reintroduce exactly the holder-died failure mode above, to save one download per lane
#   per release. The uncoordinated case is simply the tool's previous behaviour for one sample, so
#   the worst cold-cache run is no worse than every run was before.
vc_store() {
  local key=${1:-} verdict=${2:-} dir tmp payload now
  vc_valid_key "$key" || return 1
  [ -n "$verdict" ] || return 1
  # One line, because vc_lookup reads one. A multi-line verdict would store a second line that no
  # reader returns, so the caller would get a silently truncated answer back out of the cache.
  case "$verdict" in *$'\n'*) return 1 ;; esac
  dir=$(vc_dir) || return 1
  now=$(date -u +%s)
  payload=$(printf 'version 1\nkey %s\nstatus PASS\nwritten %s\nverdict %s' "$key" "$now" "$verdict")
  tmp=$(mktemp "$dir/.entry.XXXXXX") || return 1
  {
    printf '%s\n' "$payload"
    printf 'sum %s\n' "$(printf '%s\n' "$payload" | sha256sum | cut -d' ' -f1)"
  } > "$tmp" || { rm -f "$tmp"; return 1; }
  chmod 644 "$tmp" 2>/dev/null
  mv -f "$tmp" "$dir/$key" || { rm -f "$tmp"; return 1; }
}

# vc_age_note <label> <unix timestamp> — the parenthetical a cached line carries, so a reader of
# the gate's output can always tell a remembered verification from one that just ran. A cached line
# that looked identical to a fresh one would be an instrument hiding how it got its answer.
vc_age_note() {
  local when=${2:-}
  case "$when" in "" | *[!0-9]*) printf 'cached'; return ;; esac
  printf 'cached: this %s was fully downloaded and digest-matched at %s, and nothing GitHub reports about the asset has changed since' \
    "$1" "$(date -u -d "@$when" +%FT%TZ 2>/dev/null || printf '%s' "$when")"
}

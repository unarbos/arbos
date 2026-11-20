#!/usr/bin/env bash
# allow-audit.sh <allow-list> [ratified-list]
#
# The allow-list is a CACHE of the coordinator's authorizations, never a SOURCE of them. A token
# entitles a merge only because the coordinator named that pull request in a message; the file
# exists so the queue need not be re-derived from the transcript before every call. So a token
# written there by anybody else authorises nothing — it is not in the only place authority lives.
#
# That is what makes it safe for the live allow-list to sit in a store other agents can write to.
# The durability problem and the authority problem are separate, and this script is the seam.
# The alternative — keeping a digest of the file bytes — only moves the problem, because on a
# fresh VM there is no trustworthy local digest to compare a restore against. The anchor has to
# be outside the file either way, so put it outside the file and make it mean something.
#
# The comparison is over the sorted TOKEN SET, not the file bytes, and it is ASYMMETRIC:
#
#   extra  (in allow-list, never ratified)  -> exit 3. Refuse this merge AND every other merge
#                                              until the coordinator answers. An unexplained
#                                              token is a tampering signal, not a queue entry.
#   missing (ratified, gone from allow-list)-> exit 0 with a report. Fail-safe on its own (the
#                                              PR is simply unauthorized) but it is either loss
#                                              or tampering, and both matter.
#
# Set-not-bytes is the point: prose, headers and the hold and freeze blocks are commentary, not
# authority. They can be edited freely and must not move the digest — which is what leaves the
# merge worker able to write down WHY something is held without needing ratification to do it.
set -u

ALLOW_FILE="${1:?usage: allow-audit.sh <allow-list> [ratified-list]}"
# Defaults beside the allow-list, in the same kit directory and by the same convention: both are
# symlinks into the durable store on the live host, and a sandbox kit gets its own pair. Pointing
# the default at an absolute store path instead would make every sandbox silently audit against
# the live lane's reference set, which is the sort of cross-talk a kit must not have.
RATIFIED_FILE="${2:-${MERGE_RATIFIED:-$(dirname "$ALLOW_FILE")/merge-ratified}}"

# Exactly the extraction merge.sh uses, so the set audited is the set that would authorise.
tokens_of_allow() {
  sed 's/#.*//' "$1" | tr -s ' \t\n' ' ' | sed 's/^ //; s/ $//' | tr ' ' '\n' | sed '/^$/d' | LC_ALL=C sort -u
}
# First field of each non-comment line; the rest is provenance.
tokens_of_ratified() {
  sed 's/#.*//' "$1" | awk '{print $1}' | sed '/^$/d' | LC_ALL=C sort -u
}

# Exit 4, not 3, and the distinction is the kit's oldest rule: a setup gap must never be
# reportable as a policy answer. A missing reference set means nothing was audited; it does not
# mean the audit found something. merge.sh maps this into its setup preflight so it reads as a
# MISSING FILE alongside a missing allow-list, and never as a refusal of a pull request.
if [ ! -r "$RATIFIED_FILE" ]; then
  echo "SETUP: no readable ratified-token list at $RATIFIED_FILE."
  echo "  This is a MISSING FILE and NOT an audit result: the allow-list was compared against"
  echo "  nothing, so no token has been found good or bad. Ask the coordinator to ratify the"
  echo "  current token set, or point MERGE_RATIFIED at the list this session has."
  exit 4
fi

a=$(mktemp); r=$(mktemp); trap 'rm -f "$a" "$r"' EXIT
tokens_of_allow "$ALLOW_FILE" > "$a"
tokens_of_ratified "$RATIFIED_FILE" > "$r"

extra=$(LC_ALL=C comm -23 "$a" "$r" | tr '\n' ' ' | sed 's/ $//')
missing=$(LC_ALL=C comm -13 "$a" "$r" | tr '\n' ' ' | sed 's/ $//')

# Provisional means the reference set has been reported but not yet confirmed against the
# coordinator's own queue, so it cannot yet discriminate a real token from a planted one. Refuse
# rather than audit against a set that has not been looked at.
if grep -qE '^#[[:space:]]*STATUS:[[:space:]]*PROVISIONAL' "$RATIFIED_FILE"; then
  echo "AUDIT HALT: the ratified-token list is marked PROVISIONAL and has not been confirmed."
  echo "  $RATIFIED_FILE"
  echo "  The reference set cannot tell a real authorization from a planted one until the"
  echo "  coordinator has checked it against their own queue. Report the token set and wait."
  echo "  tokens held ($(wc -l < "$a")): $(tr '\n' ' ' < "$a" | sed 's/ $//')"
  exit 3
fi

if [ -n "$extra" ]; then
  echo "AUDIT HALT: the allow-list names pull requests the coordinator never authorized: $extra"
  echo "  This is a TAMPERING SIGNAL, not a queue entry, and it is not specific to the pull"
  echo "  request you asked to merge. Merge NOTHING — not this one and not any other — until the"
  echo "  coordinator has answered for every token above."
  echo "    allow-list: $ALLOW_FILE"
  echo "    ratified:   $RATIFIED_FILE"
  exit 3
fi

if [ -n "$missing" ]; then
  echo "AUDIT: ratified pull requests have gone missing from the allow-list: $missing"
  echo "  Not blocking — an absent token is simply unauthorized, which is the fail-safe"
  echo "  direction. But report it: this is either loss or tampering and both matter."
fi

echo "AUDIT: $(wc -l < "$a") tokens, all ratified."
exit 0

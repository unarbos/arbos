#!/bin/bash
# Copy the iPhone loop's documents from the Project store to the second home
# on the EC2 Mac, one file at a time, straight after each write.
#
# The store has lost these documents four times: three whole-file reversions
# (the 09-16 loss, M-124, M-141) and one file vanishing on its own (M-142).
# Each time the Mac copy was what brought them back. Mirroring once per cycle
# left a cycle's work exposed, so this is meant to be called after every
# write instead.
#
#   mirror-docs.sh <file> [<file>...]      paths inside the store's internal/
#   mirror-docs.sh --all                   every mobile document
#
# The rule from M-124, in both directions: copy only when the source is at
# least as long as the destination. A shorter source is the symptom of the
# fault, not a legitimate edit, and copying it would spread the damage
# instead of containing it. Deletions are therefore never mirrored; a
# document that should go is removed by hand in both places.
#
# A refusal with no way past it is a refusal people go around. Twice on
# 09-18 a deliberate shortening — a build number's line rewritten shorter —
# was copied with a bare `scp` instead, which skips every other check this
# makes. So there is a way through, and it costs naming the file:
# `SHORTER_IS_DELIBERATE=<name>`. One file at a time, never a blanket flag,
# because the fault it guards against arrives one file at a time.
set -uo pipefail

STORE=${MOBILE_STORE:-/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal}
SSH_CONFIG=${MOBILE_SSH_CONFIG:-/tmp/mobile/ssh_config}
HOST=${MOBILE_MAC_HOST:-macmini}
REMOTE=${MOBILE_MAC_DIR:-mobile-docs}

ALL=(mobile-findings.md mobile-coverage.md mobile-cycle-reports.md
     mobile-feedback-log.md mobile-journey-runs.md mobile-journey-history.jsonl
     mobile-mac-host-and-testflight.md)

if [ "${1:-}" = "--all" ]; then
  set -- "${ALL[@]}"
elif [ $# -eq 0 ]; then
  sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//'
  exit 2
fi

ssh_mac() { ssh -F "$SSH_CONFIG" -o BatchMode=yes "$HOST" "$@"; }

status=0
for rel in "$@"; do
  src="$STORE/$rel"
  name=$(basename "$rel")
  if [ ! -f "$src" ]; then
    echo "mirror: $name is not in the store — not mirroring an absence" >&2
    status=1
    continue
  fi
  local_bytes=$(wc -c < "$src")
  remote_bytes=$(ssh_mac "wc -c < ~/$REMOTE/$name 2>/dev/null || echo 0" | tr -d '[:space:]')
  remote_bytes=${remote_bytes:-0}
  if [ "$local_bytes" -lt "$remote_bytes" ] && [ "${SHORTER_IS_DELIBERATE:-}" != "$name" ]; then
    echo "mirror: REFUSED $name — store copy $local_bytes b is shorter than the Mac's $remote_bytes b." >&2
    echo "mirror: that is the shape of the fault, not an edit. Read the store copy before deciding." >&2
    echo "mirror: if you have read it and the shortening is deliberate, say so by name:" >&2
    echo "mirror:   SHORTER_IS_DELIBERATE=$name $0 $name" >&2
    status=1
    continue
  fi
  if [ "$local_bytes" -lt "$remote_bytes" ]; then
    echo "mirror: $name is shorter and you said so — copying $remote_bytes b → $local_bytes b" >&2
  fi
  if scp -F "$SSH_CONFIG" -q "$src" "$HOST:~/$REMOTE/$name"; then
    echo "mirror: $name $local_bytes b (was $remote_bytes b)"
  else
    echo "mirror: FAILED to copy $name" >&2
    status=1
  fi
done
exit $status

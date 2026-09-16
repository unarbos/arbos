#!/usr/bin/env bash
# Sync between the Project store's internal/qa/ and the repo's qa-results
# branch, which the ArbosLife loop writes. Run from any cloud agent that has
# the store mounted; the public repo needs no token to read, and pushing the
# inbox needs ARBOS_GITHUB (1Password item vvnyarkwampjl3diocn7n6vcqe).
#
#   sync.sh pull   qa-results -> internal/qa/{bugs,kickoff-history.jsonl,spend.jsonl,rollouts/index.jsonl,rollouts/<break>/}
#   sync.sh push   internal/qa/inbox/*.md -> qa-results/inbox/   (so the loop can read feature notes)
set -euo pipefail
QA="$(cd "$(dirname "$0")" && pwd)"
WORK="${ARBOS_QA_SYNC_DIR:-/tmp/arbos-qa-results}"
REMOTE=https://github.com/unarbos/arbos.git
BRANCH=qa-results

if [ ! -d "$WORK/.git" ]; then
  git clone -q --branch "$BRANCH" --single-branch "$REMOTE" "$WORK"
else
  git -C "$WORK" pull -q --rebase origin "$BRANCH"
fi

case "${1:-pull}" in
  pull)
    mkdir -p "$QA/bugs" "$QA/rollouts"
    # Bugs: never overwrite a curated file that already exists here, and skip
    # an auto-draft whose fingerprint a curated file already lists.
    for f in "$WORK"/bugs/*.md; do
      [ -e "$f" ] || continue
      b="$(basename "$f")"
      [ -e "$QA/bugs/$b" ] && continue
      fp="${b%.md}"
      if [[ "$fp" =~ ^[0-9a-f]{10}$ ]] && grep -qsE "^fingerprints:.*\b$fp\b" "$QA"/bugs/qa-*.md; then
        continue
      fi
      cp "$f" "$QA/bugs/$b"
    done
    for f in kickoff-history.jsonl spend.jsonl; do
      [ -f "$WORK/$f" ] && cp "$WORK/$f" "$QA/arboslife-$f"
    done
    [ -f "$WORK/rollouts/index.jsonl" ] && cp "$WORK/rollouts/index.jsonl" "$QA/rollouts/arboslife-index.jsonl"
    for d in "$WORK"/rollouts/*/; do
      [ -d "$d" ] || continue
      n="$(basename "$d")"
      [ -d "$QA/rollouts/$n" ] || { mkdir -p "$QA/rollouts/$n"; cp "$d"/* "$QA/rollouts/$n/"; }
    done
    [ -f "$WORK/status.txt" ] && cp "$WORK/status.txt" "$QA/arboslife-status.txt"
    echo "pulled qa-results into $QA (arboslife-* files, new bugs, broken rollouts)"
    ;;
  push)
    : "${ARBOS_GITHUB:?set ARBOS_GITHUB (GitHub PAT) to push the inbox}"
    mkdir -p "$WORK/inbox"
    cp "$QA"/inbox/*.md "$WORK/inbox/" 2>/dev/null || true
    git -C "$WORK" add -A inbox
    if git -C "$WORK" diff --cached --quiet; then echo "inbox unchanged"; exit 0; fi
    git -C "$WORK" -c user.name=arbos-qa -c user.email=qa@arbos.local commit -q -m "inbox: $(date -u +%FT%TZ)"
    git -C "$WORK" -c "credential.helper=!f() { echo username=x-access-token; echo password=\$ARBOS_GITHUB; }; f" push -q origin "$BRANCH"
    echo "pushed inbox"
    ;;
esac

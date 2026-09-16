#!/usr/bin/env bash
# Mirror the Arbos Project store's docs/ and notes.md to a git branch.
#
#   bash mirror-docs.sh              mirror now (push only if something changed)
#   bash mirror-docs.sh check        exit 1 if the mirror is behind the store, push nothing
#   bash mirror-docs.sh restore DIR  write the mirrored docs/ and notes.md into DIR
#
# Why this exists: on 2026-09-16 the store dropped docs/ and artifacts/ with no
# event and no undo. The only documents that came back whole were the four that
# happened to be mirrored into the repo. See internal/store-docs-mirror.md.
#
# Deliberately scoped to docs/ and notes.md. internal/ is noisy and media/ is large.
#
# The branch carries a copy of this script at its root, so the tool survives a
# store loss too:
#   git -C /workspace fetch -q origin store-docs && \
#   git -C /workspace show origin/store-docs:mirror-docs.sh > /tmp/mirror-docs.sh
set -euo pipefail

STORE="${STORE:-/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983}"
REPO="${REPO:-/workspace}"
BRANCH="${BRANCH:-store-docs}"
MODE="${1:-push}"

log() { printf '[mirror-docs %s] %s\n' "$(date -u +%H:%M:%SZ)" "$*" >&2; }
die() { log "REFUSED: $*"; exit 2; }

# Resolve before any cd, or a relative invocation resolves against the repo.
SELF="$(readlink -f "${BASH_SOURCE[0]}")"

[ -d "$REPO/.git" ] || die "no git checkout at $REPO (pass REPO=/path)"
cd "$REPO"

# The branch tip is the mirror's current state. Fetch it into its own ref so a
# stale FETCH_HEAD from an unrelated fetch can never be mistaken for the parent.
git fetch -q origin "+refs/heads/$BRANCH:refs/remotes/origin/$BRANCH" 2>/dev/null || true
parent="$(git rev-parse --verify -q "refs/remotes/origin/$BRANCH" || true)"

if [ "$MODE" = restore ]; then
    dest="${2:?usage: mirror-docs.sh restore <dir>}"
    [ -n "$parent" ] || die "branch $BRANCH does not exist yet; nothing to restore"
    mkdir -p "$dest"
    git archive "$parent" | tar -x -C "$dest"
    log "restored the mirror of $parent into $dest"
    exit 0
fi

# ---- Safety gate -----------------------------------------------------------
# The store is a network mount that has been seen to report an empty or partial
# view while the data is still there. Pushing such a view would replace the
# mirror with the damage. Every check below must pass before anything is pushed.

[ -d "$STORE/docs" ]   || die "$STORE/docs is not there. The store may be faulting, or docs/ really is gone. Not touching the mirror."
[ -f "$STORE/notes.md" ] || die "$STORE/notes.md is not there. The store view looks broken. Not touching the mirror."

shopt -s nullglob
docs=("$STORE"/docs/*.md)
shopt -u nullglob
n_now=${#docs[@]}
[ "$n_now" -gt 0 ] || die "docs/ lists no .md files. The store view looks broken. Not touching the mirror."

# Every file must be readable and non-empty. A mount that half-answers shows up here.
for f in "${docs[@]}" "$STORE/notes.md"; do
    [ -r "$f" ] || die "cannot read $f"
    [ -s "$f" ] || die "$f reads as empty. The store view looks broken. Not touching the mirror."
done

# Never let the mirror shrink by accident. A deliberate deletion needs the flag.
if [ -n "$parent" ]; then
    n_last="$(git ls-tree --name-only "$parent" docs/ | grep -c '\.md$' || true)"
    if [ "$n_now" -lt "$n_last" ] && [ "${MIRROR_ALLOW_SHRINK:-0}" != "1" ]; then
        die "docs/ has $n_now files but the mirror holds $n_last. Refusing to shrink the mirror. If the removal is intended: MIRROR_ALLOW_SHRINK=1 bash mirror-docs.sh"
    fi
fi

# ---- Build the tree --------------------------------------------------------
# Plumbing with a throwaway index, so the checkout's own index, branch and
# working tree are never touched. Safe to run while other work is in progress.
GIT_INDEX_FILE="$(mktemp -u /tmp/mirror-docs-index.XXXXXX)"
export GIT_INDEX_FILE
trap 'rm -f "$GIT_INDEX_FILE"' EXIT

stage() { # stage <path-in-branch> <source-file> [mode]
    local blob
    blob="$(git hash-object -w -- "$2")"
    git update-index --add --cacheinfo "${3:-100644},$blob,$1"
}

stage notes.md "$STORE/notes.md"
for f in "$STORE"/docs/*; do
    [ -f "$f" ] || continue
    stage "docs/$(basename "$f")" "$f"
done
# The branch carries the tool that reads it.
stage mirror-docs.sh "$SELF" 100755

tree="$(git write-tree)"

if [ -n "$parent" ] && [ "$(git rev-parse "$parent^{tree}")" = "$tree" ]; then
    log "no change: the mirror already matches the store ($n_now docs)"
    exit 0
fi

if [ "$MODE" = check ]; then
    log "STALE: the store differs from the mirror ($n_now docs). Run: bash mirror-docs.sh"
    exit 1
fi

# ---- Commit and push -------------------------------------------------------
total="$(du -sh --exclude=.git "$STORE/docs" 2>/dev/null | cut -f1 || echo '?')"
msg="store docs mirror: $n_now documents, $total, notes.md $(stat -c%s "$STORE/notes.md") B

Mirrored from $STORE by ${MIRROR_BY:-$(hostname)} at $(date -u +%FT%TZ)."

commit="$(printf '%s' "$msg" | git commit-tree "$tree" ${parent:+-p "$parent"})"

for attempt in 1 2 3 4; do
    if git push -q origin "$commit:refs/heads/$BRANCH" 2>/dev/null; then
        log "pushed $commit to $BRANCH ($n_now docs, $total)"
        exit 0
    fi
    # Someone else mirrored in between, or the network blipped. Re-read the tip and retry.
    sleep $((attempt * 4))
    git fetch -q origin "+refs/heads/$BRANCH:refs/remotes/origin/$BRANCH" 2>/dev/null || true
    new_parent="$(git rev-parse --verify -q "refs/remotes/origin/$BRANCH" || true)"
    if [ "$new_parent" != "$parent" ]; then
        parent="$new_parent"
        if [ "$(git rev-parse "$parent^{tree}")" = "$tree" ]; then
            log "another worker pushed the same content; nothing to do"
            exit 0
        fi
        commit="$(printf '%s' "$msg" | git commit-tree "$tree" -p "$parent")"
    fi
    log "push failed, retry $attempt"
done
die "could not push after 4 attempts"

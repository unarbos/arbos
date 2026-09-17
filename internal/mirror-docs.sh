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

[ -d "$STORE/internal" ] || die "$STORE/internal is not there. The store may be faulting. Not touching the mirror."
[ -n "$(ls -A "$STORE/internal" 2>/dev/null)" ] || die "internal/ lists nothing. The store view looks broken. Not touching the mirror."

# Every file must be readable and non-empty. A mount that half-answers shows up here.
for f in "${docs[@]}" "$STORE/notes.md"; do
    [ -r "$f" ] || die "cannot read $f"
    [ -s "$f" ] || die "$f reads as empty. The store view looks broken. Not touching the mirror."
done

# ---- Deliberate shrinks, written down --------------------------------------
# The gates below already demand a reason. This keeps it.
#
# A shrink that was decided and a shrink that was an accident leave the same
# trace — a smaller tree — and the readers compare trees. So the reason goes
# into the branch beside the view, not only into the terminal of whoever
# typed it: a reader can then tell "somebody meant this" from "something ate
# it", which is the distinction this whole guard exists to make and the one
# thing it could not record.
SHRINKS="$(mktemp /tmp/mirror-shrinks.XXXXXX)"
: > "$SHRINKS"
shrink_reason=""

# shrink_note <what> <from> <to>
shrink_note() {
    shrink_reason="${MIRROR_ALLOW_SHRINK:-}"
    printf '{"ts":"%s","by":"%s","what":"%s","from":%s,"to":%s,"reason":%s}\n' \
        "$(date -u +%FT%TZ)" "${MIRROR_BY:-$(hostname)}" "$1" "$2" "$3" \
        "$(printf '%s' "$shrink_reason" | python3 -c 'import json,sys; print(json.dumps(sys.stdin.read().strip()))')" \
        >> "$SHRINKS"
}

# Never let the mirror shrink by accident. A deliberate deletion needs the flag.
if [ -n "$parent" ]; then
    n_last="$(git ls-tree --name-only "$parent" docs/ | grep -c '\.md$' || true)"
    if [ "$n_now" -lt "$n_last" ] && [ -z "${MIRROR_ALLOW_SHRINK:-}" ]; then
        die "docs/ has $n_now files but the mirror holds $n_last. Refusing to shrink the mirror. If the removal is intended, say why: MIRROR_ALLOW_SHRINK='<reason>' bash mirror-docs.sh"
    fi
    if [ "$n_now" -lt "$n_last" ]; then
        shrink_note "docs" "$n_last" "$n_now"
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
# ---- internal/: tooling and pending instructions, not bulk ------------------
# Boundary (2026-09-16, after the second loss took internal/parity/ and
# internal/features-inbox/): every file under internal/ is mirrored EXCEPT
#   - anything under a folder named rollouts, staging, state, node_modules,
#     .venv, __pycache__, target, .git  (run output, caches, checkouts)
#   - binaries: images, audio, video, archives, compiled files
#   - files over MIRROR_MAX_BYTES (default 2 MB) — logged, not staged
# So a document, a script, a bug file, an inbox note, a history .jsonl and the
# parity rig's driver are protected; a rollout bundle or a screenshot is not,
# and its owner keeps their own copy.
MIRROR_MAX_BYTES="${MIRROR_MAX_BYTES:-2097152}"
n_internal=0
skipped_big=0
# Prune the excluded folders in find itself (the store is a slow network mount; descending
# into rollouts/ costs minutes), then hash every kept file in one git call.
list="$(mktemp /tmp/mirror-docs-list.XXXXXX)"
prune='( -type d ( -name rollouts -o -name staging -o -name state -o -name node_modules -o -name .venv -o -name __pycache__ -o -name target -o -name .git ) -prune )'
# shellcheck disable=SC2086
find "$STORE/internal" $prune -o -type f -size -"$((MIRROR_MAX_BYTES / 1024 + 1))"k \
    ! \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.gif' -o -iname '*.webp' -o -iname '*.mp4' -o -iname '*.mov' -o -iname '*.wav' -o -iname '*.mp3' -o -iname '*.zip' -o -iname '*.tar' -o -iname '*.gz' -o -iname '*.tgz' -o -iname '*.pyc' -o -iname '*.so' -o -iname '*.o' -o -iname '*.bin' -o -iname '*.pdf' \) \
    -print 2>/dev/null | sort > "$list"
# shellcheck disable=SC2086
skipped_big="$(find "$STORE/internal" $prune -o -type f -size +"$((MIRROR_MAX_BYTES / 1024))"k -print 2>/dev/null | wc -l)"
if [ -s "$list" ]; then
    # One hash-object call for all files; the blob ids come back in the same order.
    paste -d' ' <(git hash-object -w --stdin-paths < "$list") "$list" | while IFS=' ' read -r blob f; do
        rel="${f#"$STORE/"}"
        mode=100644
        [ -x "$f" ] && mode=100755
        git update-index --add --cacheinfo "$mode,$blob,$rel"
    done
    n_internal="$(wc -l < "$list")"
fi
rm -f "$list"
[ "$n_internal" -gt 0 ] || die "internal/ yielded no mirrorable files. The store view looks broken. Not touching the mirror."
[ "$skipped_big" -gt 0 ] && log "internal/: $skipped_big file(s) over $MIRROR_MAX_BYTES bytes left out"

# ---- media/desktop-feedback/: Jacob's own reports ----------------------------
# The one folder under media/ the mirror takes (2026-09-16, QA worker, on the coordinator's
# call): each report is a small report.json and a feedback.md — the user's own words about what
# went wrong, the most irreplaceable content the store holds. Two of them had just been corrected
# to say a missing screenshot was a fault, not his choice; a store fault would have undone that
# and the reports would have come back reading as genuine. Same size and type rules as internal/:
# screenshots and other media stay out.
n_feedback=0
fb="$STORE/media/desktop-feedback"
if [ -d "$fb" ]; then
    list="$(mktemp /tmp/mirror-docs-fb.XXXXXX)"
    # shellcheck disable=SC2086
    find "$fb" $prune -o -type f -size -"$((MIRROR_MAX_BYTES / 1024 + 1))"k \
        ! \( -iname '*.png' -o -iname '*.jpg' -o -iname '*.jpeg' -o -iname '*.gif' -o -iname '*.webp' -o -iname '*.mp4' -o -iname '*.mov' -o -iname '*.wav' -o -iname '*.mp3' -o -iname '*.zip' -o -iname '*.b64' \) \
        -print 2>/dev/null | sort > "$list"
    if [ -s "$list" ]; then
        paste -d' ' <(git hash-object -w --stdin-paths < "$list") "$list" | while IFS=' ' read -r blob f; do
            rel="${f#"$STORE/"}"
            git update-index --add --cacheinfo "100644,$blob,$rel"
        done
        n_feedback="$(wc -l < "$list")"
    fi
    rm -f "$list"
fi

# Never let the internal/ mirror shrink by accident either. Two absolute rules
# (2026-09-17, after a pass recorded 454 files where the tip held 492 and the
# old one-tenth allowance let it through):
#   1. every directory the tip holds under internal/ must still list here;
#   2. any fall in the file count needs a stated reason. A partial listing from
#      the mount, or a deletion nobody has owned, must never become the mirror.
if [ -n "$parent" ]; then
    n_internal_last="$(git ls-tree -r --name-only "$parent" internal/ 2>/dev/null | wc -l || true)"
    if [ "$n_internal_last" -gt 0 ]; then
        gone_dirs=""
        while IFS= read -r d; do
            [ -n "$d" ] && [ ! -d "$STORE/$d" ] && gone_dirs="$gone_dirs $d"
        done < <(git ls-tree -r --name-only "$parent" internal/ 2>/dev/null | sed -E 's#/[^/]+$##' | sort -u)
        if [ -n "$gone_dirs" ] && [ -z "${MIRROR_ALLOW_SHRINK:-}" ]; then
            die "directories the mirror holds are not listed here:$gone_dirs. Either the view is partial or they were removed. Not touching the mirror; if the removal is intended, say why: MIRROR_ALLOW_SHRINK='<reason>' bash mirror-docs.sh"
        fi
        if [ "$n_internal" -lt "$n_internal_last" ] && [ -z "${MIRROR_ALLOW_SHRINK:-}" ]; then
            gone_files="$(comm -23 <(git ls-tree -r --name-only "$parent" internal/ 2>/dev/null | sort) <(GIT_INDEX_FILE="$GIT_INDEX_FILE" git ls-files internal/ | sort) | head -8 | tr '\n' ' ')"
            die "internal/ has $n_internal mirrorable files but the mirror holds $n_internal_last (first missing: $gone_files). Refusing to shrink the mirror by any amount without a reason. If the removal is intended, say why: MIRROR_ALLOW_SHRINK='<reason>' bash mirror-docs.sh"
        fi
        if [ "$n_internal" -lt "$n_internal_last" ]; then
            shrink_note "internal" "$n_internal_last" "$n_internal"
        fi
    fi
fi

# The branch carries the tool that reads it.
stage mirror-docs.sh "$SELF" 100755

# The log of every shrink anybody allowed, appended rather than replaced, so a
# reader sees all of them and not only the last.
if [ -n "$parent" ] && git cat-file -e "$parent:shrinks.jsonl" 2>/dev/null; then
    shrinks_prev="$(mktemp /tmp/mirror-shrinks-prev.XXXXXX)"
    git cat-file -p "$parent:shrinks.jsonl" > "$shrinks_prev"
    cat "$SHRINKS" >> "$shrinks_prev"
    mv "$shrinks_prev" "$SHRINKS"
fi
if [ -s "$SHRINKS" ]; then
    stage shrinks.jsonl "$SHRINKS" 100644
fi

# ...and a note telling whoever lands here what this branch is. Static text only:
# anything that changed per run (a date, a count) would make every run a new commit.
readme="$(mktemp /tmp/mirror-docs-readme.XXXXXX)"
cat > "$readme" <<'README'
# Arbos Project store — mirror of `docs/`, `notes.md` and `internal/` (within a boundary)

This branch is a backup, not code. It has no shared history with `main` and is never merged.

It mirrors two things from the Arbos Project's Cursor Agent Store
(`/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983`):

- `docs/` — the Project's written deliverables, at the same names and paths the store uses,
  so a link of the form `docs/<name>.md` means the same file here and there.
- `notes.md` — the Project status page, for context on what the documents refer to.
- `media/desktop-feedback/` — Jacob's own in-app reports (`report.json`, `feedback.md` per report;
  screenshots left out). The one folder under `media/` the mirror takes: the user's own words
  about what went wrong are the most irreplaceable content here (added 2026-09-16 23:20 UTC).
- `internal/` — working tooling, reports, pending instructions and small state, at the store's
  own paths. Since 2026-09-16 (the second loss took `internal/parity/` and
  `internal/features-inbox/`). **Boundary:** every file under `internal/` except run output and
  caches (any folder named `rollouts`, `staging`, `state`, `node_modules`, `.venv`,
  `__pycache__`, `target`, `.git`), binaries (images, audio, video, archives, compiled files),
  and files over 2 MB. So bug files, inbox notes, scripts, the parity rig, history `.jsonl`
  files and reports are protected; rollout bundles, screenshots and big logs are not — their
  owners keep their own copy.

Not mirrored: `media/` (large binaries), `artifacts/` (platform folder), and the excluded
paths above.

## The convention

**One branch, `store-docs`, with `docs/` at the root.** Not inside a code branch, not under a
`store/` prefix, not one branch per author. The whole point is a single place a person can look.

## Mirror after you write a document

```bash
bash /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/internal/mirror-docs.sh
```

The script is also at the root of this branch, so it survives a store loss:

```bash
git show origin/store-docs:mirror-docs.sh > /tmp/mirror-docs.sh
```

It pushes only when something changed, refuses to push a store view that looks broken or that would
shrink the mirror, and never touches your checkout's branch, index or working tree.

## Restore the store from this branch

```bash
bash mirror-docs.sh restore /tmp/mirror-restore
cp /tmp/mirror-restore/docs/*.md /cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983/docs/
```

## Why it exists

On 2026-09-16 the store dropped `docs/` and `artifacts/` with no event, no audit trail the owner can
read and no undo. The only documents recovered whole were the four that happened to be mirrored into
the repository. Account: `internal/store-docs-loss-2026-09-16.md` in the store.
README
stage README.md "$readme"
rm -f "$readme"

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
msg="store docs mirror: $n_now documents, $total, notes.md $(stat -c%s "$STORE/notes.md") B, internal/ $n_internal files, feedback $n_feedback files

Mirrored from $STORE by ${MIRROR_BY:-$(hostname)} at $(date -u +%FT%TZ).${shrink_reason:+

Deliberate shrink, allowed by whoever ran this: $shrink_reason}"

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

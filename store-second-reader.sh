#!/usr/bin/env bash
# A second reader of the Arbos Project store, from a different machine than
# the mirror's. It reads the store, compares what it sees with what the
# mirror's client last accepted (the tip of the store-docs branch), records
# one line per run on the store-watch branch, and shouts when they disagree.
#
#   bash store-second-reader.sh            read, compare, record, shout if FAULT
#   bash store-second-reader.sh show       print the last 20 recorded lines
#
# Why: on 2026-09-17 one client saw the store empty while two others saw it
# whole, and the mirror could not know because it judges the store from the
# machine it runs on. A view recorded from a second machine, with the time,
# is what makes that class visible and countable. docs/store-fault-report-2026-09-17.md
# has the episodes; internal/store-docs-mirror.md has the mirror.
#
# Durable home: the store-watch branch root (the store copy under internal/
# is taken in every episode, like mirror-docs.sh). Fetch it with
#   git -C /workspace fetch -q origin store-watch && \
#   git -C /workspace show origin/store-watch:store-second-reader.sh > /tmp/store-second-reader.sh
set -uo pipefail

STORE="${STORE:-/cursor/stores/bc-ec8c092a-3084-4e3e-9e34-7b2a1f8c6983}"
REPO="${REPO:-/workspace}"
MIRROR_BRANCH="${MIRROR_BRANCH:-store-docs}"
WATCH_BRANCH="${WATCH_BRANCH:-store-watch}"
CLIENT="${CLIENT:-$(hostname -s 2>/dev/null || hostname)}"
MODE="${1:-run}"
# Resolve before the cd into the repo, or a relative invocation resolves against the repo.
SELF="$(readlink -f "${BASH_SOURCE[0]}")"
NOW="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# The paths the episodes took, plus the two that were never taken as controls.
SCOPES=(docs internal/features-inbox internal/parity internal/qa/bugs internal/mirror-docs.sh notes.md)

say() { printf '[store-second-reader %s] %s\n' "$(date -u +%H:%M:%SZ)" "$*" >&2; }

cd "$REPO" || { say "no checkout at $REPO"; exit 3; }
git fetch -q origin "+refs/heads/$MIRROR_BRANCH:refs/remotes/origin/$MIRROR_BRANCH" \
                    "+refs/heads/$WATCH_BRANCH:refs/remotes/origin/$WATCH_BRANCH" 2>/dev/null || true
mirror_tip="$(git rev-parse --verify -q "refs/remotes/origin/$MIRROR_BRANCH" || true)"
watch_tip="$(git rev-parse --verify -q "refs/remotes/origin/$WATCH_BRANCH" || true)"

if [ "$MODE" = show ]; then
    [ -n "$watch_tip" ] || { say "no $WATCH_BRANCH branch yet"; exit 0; }
    git show "$watch_tip:readers/$CLIENT.jsonl" 2>/dev/null | tail -20
    exit 0
fi

# ---- Restore marker ------------------------------------------------------------
# A restore copies hundreds of files back through a slow mount, and another
# client reading half-way through sees exactly the shape of a loss (07:07:26Z
# on 2026-09-17 was this). The restorer marks start and end on the watch
# branch so readers report RESTORING, not FAULT, while it runs.
#   store-second-reader.sh restore-begin <staged-dir>
#   store-second-reader.sh restore-end
RESTORE_MARK="restore-in-progress.json"
watch_commit() {   # watch_commit <message> [<mode>,<blob>,<path>]... ; a path with blob "-" is removed
    local msg="$1"; shift
    local idx parent tree commit spec mode blob path
    idx="$(mktemp)"; rm -f "$idx"
    parent="$(git rev-parse --verify -q "refs/remotes/origin/$WATCH_BRANCH" || true)"
    if [ -n "$parent" ]; then GIT_INDEX_FILE="$idx" git read-tree "$parent"; fi
    for spec in "$@"; do
        IFS=, read -r mode blob path <<< "$spec"
        if [ "$blob" = "-" ]; then GIT_INDEX_FILE="$idx" git update-index --force-remove "$path" 2>/dev/null || true
        else GIT_INDEX_FILE="$idx" git update-index --add --cacheinfo "$mode,$blob,$path"; fi
    done
    # The script rides along so every client runs the same version.
    GIT_INDEX_FILE="$idx" git update-index --add --cacheinfo "100755,$(git hash-object -w "$SELF"),store-second-reader.sh"
    tree=$(GIT_INDEX_FILE="$idx" git write-tree)
    if [ -n "$parent" ]; then commit=$(git commit-tree "$tree" -p "$parent" -m "$msg")
    else commit=$(git commit-tree "$tree" -m "$msg"); fi
    git push -q origin "$commit:refs/heads/$WATCH_BRANCH"
    local rc=$?
    rm -f "$idx"
    return $rc
}
if [ "$MODE" = restore-begin ]; then
    staged="${2:-unknown}"
    mark_blob=$(printf '{"client":"%s","since":"%s","staged":"%s"}\n' "$CLIENT" "$NOW" "$staged" | git hash-object -w --stdin)
    watch_commit "$CLIENT $NOW restore begins ($staged)" "100644,$mark_blob,$RESTORE_MARK" \
        && say "marked restore in progress on $WATCH_BRANCH" || say "could not mark restore on $WATCH_BRANCH"
    exit 0
fi
if [ "$MODE" = restore-end ]; then
    watch_commit "$CLIENT $NOW restore ends" "100644,-,$RESTORE_MARK" \
        && say "cleared restore marker on $WATCH_BRANCH" || say "could not clear restore marker on $WATCH_BRANCH"
    exit 0
fi
restore_mark="$( [ -n "$watch_tip" ] && git show "$watch_tip:$RESTORE_MARK" 2>/dev/null || true)"
restore_since=""
restore_age_min=0
if [ -n "$restore_mark" ]; then
    restore_since=$(printf '%s' "$restore_mark" | python3 -c 'import sys,json; d=json.load(sys.stdin); print(d.get("client","?")+" since "+d.get("since","?"))' 2>/dev/null || echo "?")
    restore_ts=$(printf '%s' "$restore_mark" | python3 -c 'import sys,json; print(json.load(sys.stdin).get("since",""))' 2>/dev/null || true)
    [ -n "$restore_ts" ] && restore_age_min=$(( ( $(date -u +%s) - $(date -u -d "$restore_ts" +%s 2>/dev/null || date -u +%s) ) / 60 ))
fi

# ---- What the mirror's client last accepted -------------------------------
# name<TAB>blob for every file in scope on the mirror branch. The branch only
# moves when the mirror's safety gate passed, so it is a floor: a file on it
# should be in the store unless someone deleted it on purpose.
branch_list="$(mktemp)"; here_list="$(mktemp)"
trap 'rm -f "$branch_list" "$here_list"' EXIT
if [ -n "$mirror_tip" ]; then
    git ls-tree -r "$mirror_tip" -- "${SCOPES[@]}" 2>/dev/null | awk '{print $4"\t"$3}' | sort > "$branch_list"
fi
branch_files=$(wc -l < "$branch_list")
mirror_age_min=$(( ( $(date -u +%s) - $(git log -1 --format=%ct "$mirror_tip" 2>/dev/null || echo 0) ) / 60 ))

# ---- What this client sees --------------------------------------------------
store_ok=1
[ -d "$STORE" ] || store_ok=0
root_listing="$(ls "$STORE" 2>/dev/null | tr '\n' ' ')"
docs_dir=$([ -d "$STORE/docs" ] && echo present || echo MISSING)
notes=$([ -s "$STORE/notes.md" ] && echo present || echo MISSING)
zero_bytes=0; zero_paths=(); bad_paths=(); in_flight=()
for scope in "${SCOPES[@]}"; do
    p="$STORE/$scope"
    if [ -f "$p" ]; then
        blob=$(git hash-object "$p" 2>/dev/null || echo unreadable)
        [ -s "$p" ] || { zero_bytes=$((zero_bytes+1)); zero_paths+=("$scope"); }
        [ "$blob" = unreadable ] && bad_paths+=("$scope (unreadable)")
        printf '%s\t%s\n' "$scope" "$blob" >> "$here_list"
    elif [ -d "$p" ]; then
        while IFS= read -r f; do
            rel="${f#"$STORE"/}"
            blob=$(git hash-object "$f" 2>/dev/null || echo unreadable)
            [ -s "$f" ] || { zero_bytes=$((zero_bytes+1)); zero_paths+=("$rel"); }
            [ "$blob" = unreadable ] && bad_paths+=("$rel (unreadable)")
            printf '%s\t%s\n' "$rel" "$blob" >> "$here_list"
        done < <(find "$p" -type f 2>/dev/null)
    fi
done
# A zero-byte file that is non-zero five seconds later was a writer mid-flight (an in-place rewrite
# the reader landed on), not a truncation. Re-read once; only what stays empty is a fault, and it is named.
if [ ${#zero_paths[@]} -gt 0 ]; then
    sleep 5
    still=()
    for z in "${zero_paths[@]}"; do
        if [ -s "$STORE/$z" ]; then in_flight+=("$z"); else still+=("$z"); bad_paths+=("$z (zero bytes)"); fi
    done
    zero_bytes=${#still[@]}
fi
sort -o "$here_list" "$here_list"
here_files=$(wc -l < "$here_list")

# ---- Compare -----------------------------------------------------------------
# missing: on the branch, not here (by name). extra/changed: here, not on the
# branch with the same content (the mirror has not caught up, or it refused).
missing=$(comm -23 <(cut -f1 "$branch_list") <(cut -f1 "$here_list"))
missing_n=$(printf '%s' "$missing" | grep -c . || true)
extra_n=$(comm -13 "$branch_list" "$here_list" | wc -l)
unreadable_n=$(grep -c $'\tunreadable$' "$here_list" || true)

verdict=AGREE
reason=""
if [ "$store_ok" = 0 ]; then verdict=FAULT; reason="store not mounted at $STORE"
elif [ "$docs_dir" = MISSING ]; then verdict=FAULT; reason="docs/ is gone (root lists: $root_listing)"
elif [ "$notes" = MISSING ]; then verdict=FAULT; reason="notes.md is gone or empty"
elif [ "$here_files" = 0 ]; then verdict=FAULT; reason="nothing readable in scope"
elif [ "$unreadable_n" -gt 0 ] || [ "$zero_bytes" -gt 0 ]; then verdict=FAULT; reason="$unreadable_n unreadable, $zero_bytes zero-byte file(s): $(IFS=', '; echo "${bad_paths[*]}")"
elif [ "$missing_n" -gt 0 ] && [ -n "$restore_mark" ] && [ "$restore_age_min" -le 120 ]; then
    verdict=RESTORING; reason="$missing_n file(s) the mirror accepted are not here yet; a restore is running ($restore_since, ${restore_age_min} min ago)"
elif [ "$missing_n" -gt 0 ]; then verdict=FAULT; reason="$missing_n file(s) the mirror accepted are not here"
elif [ "$extra_n" -gt 0 ]; then verdict=BEHIND; reason="$extra_n file(s) here that the mirror has not taken yet (mirror tip is ${mirror_age_min} min old)"
fi

missing_json=$(printf '%s\n' "$missing" | python3 -c 'import sys,json; print(json.dumps([l.rstrip("\n") for l in sys.stdin if l.strip()][:25]))')
bad_json=$(printf '%s\n' "${bad_paths[@]}" | python3 -c 'import sys,json; print(json.dumps([l.rstrip("\n") for l in sys.stdin if l.strip()][:25]))')
in_flight_json=$(printf '%s\n' "${in_flight[@]}" | python3 -c 'import sys,json; print(json.dumps([l.rstrip("\n") for l in sys.stdin if l.strip()][:25]))')
line=$(python3 - "$NOW" "$CLIENT" "$verdict" "$reason" "$here_files" "$branch_files" "$missing_n" "$extra_n" "$mirror_tip" "$mirror_age_min" "$docs_dir" "$notes" "$missing_json" "$bad_json" "$in_flight_json" <<'EOF'
import sys, json
a = sys.argv[1:]
print(json.dumps({
    "ts": a[0], "client": a[1], "verdict": a[2], "reason": a[3],
    "here_files": int(a[4]), "mirror_files": int(a[5]),
    "missing_here": int(a[6]), "not_yet_mirrored": int(a[7]),
    "mirror_tip": a[8][:12], "mirror_age_min": int(a[9]),
    "docs_dir": a[10], "notes_md": a[11],
    "missing_sample": json.loads(a[12]),
    "bad_paths": json.loads(a[13]),
    "in_flight": json.loads(a[14]),
}, separators=(",", ":")))
EOF
)

say "$verdict — $reason here=$here_files mirror=$branch_files (tip ${mirror_tip:0:8}, ${mirror_age_min} min old)"

# ---- Record on the store-watch branch ----------------------------------------
# One file per client, appended; the branch is an orphan and only readers write
# it, so a push race is rare and retried once. The script rides along at the
# root so it survives the store losing internal/.
record() {
    local idx tree parent old commit
    idx="$(mktemp)"; rm -f "$idx"
    parent="$(git rev-parse --verify -q "refs/remotes/origin/$WATCH_BRANCH" || true)"
    old=""
    [ -n "$parent" ] && old="$(git show "$parent:readers/$CLIENT.jsonl" 2>/dev/null || true)"
    blob=$( { [ -n "$old" ] && printf '%s\n' "$old"; printf '%s\n' "$line"; } | git hash-object -w --stdin)
    self_blob=$(git hash-object -w "$SELF")
    if [ -n "$parent" ]; then GIT_INDEX_FILE="$idx" git read-tree "$parent"; fi
    GIT_INDEX_FILE="$idx" git update-index --add --cacheinfo "100644,$blob,readers/$CLIENT.jsonl" \
                                                 --cacheinfo "100755,$self_blob,store-second-reader.sh"
    tree=$(GIT_INDEX_FILE="$idx" git write-tree)
    if [ -n "$parent" ]; then
        commit=$(git commit-tree "$tree" -p "$parent" -m "$CLIENT $NOW $verdict")
        git push -q origin "$commit:refs/heads/$WATCH_BRANCH"
    else
        commit=$(git commit-tree "$tree" -m "store-watch: second readers of the Project store ($CLIENT $NOW $verdict)")
        git push -q origin "$commit:refs/heads/$WATCH_BRANCH"
    fi
    local rc=$?
    rm -f "$idx"
    return $rc
}
if ! record; then
    say "push raced; fetching and retrying once"
    git fetch -q origin "+refs/heads/$WATCH_BRANCH:refs/remotes/origin/$WATCH_BRANCH" && record || say "could not record on $WATCH_BRANCH"
fi

# ---- Shout -------------------------------------------------------------------
if [ "$verdict" = FAULT ]; then
    inbox="$STORE/internal/qa/inbox"
    note="$inbox/$(date -u +%Y-%m-%d)-store-second-reader-fault-$(date -u +%H%M).md"
    if mkdir -p "$inbox" 2>/dev/null && cat > "$note" <<EOF
---
cursor:
  subagentId: "bc-22d20d79-de36-524a-ae31-3e1c44c03b98"
---

# Store second reader: FAULT at $NOW from $CLIENT

$reason.

This client saw $here_files file(s) in scope; the mirror's last accepted view (\`$MIRROR_BRANCH\` at \`${mirror_tip:0:12}\`, ${mirror_age_min} min old) has $branch_files. \`docs/\`: $docs_dir; \`notes.md\`: $notes. Store root listed: \`$root_listing\`.

First missing paths:
$(printf '%s\n' "$missing" | grep . | head -25 | sed 's/^/- `/; s/$/`/')

Zero-byte or unreadable paths (still so on a re-read 5 s later):
$(printf '%s\n' "${bad_paths[@]}" | grep . | head -25 | sed 's/^/- `/; s/$/`/')

Seen empty but filled in within 5 s (a writer mid-flight, not counted):
$(printf '%s\n' "${in_flight[@]}" | grep . | head -25 | sed 's/^/- `/; s/$/`/')

Recorded on branch \`$WATCH_BRANCH\` as \`readers/$CLIENT.jsonl\`. Compare with the mirror's view at the same minute to tell a service loss (all clients agree) from a client fault (they disagree).
EOF
    then say "wrote $note"; else say "could not write the shout into the store (it may be the fault); the $WATCH_BRANCH line stands"; fi
    echo "FAULT"
    exit 1
fi
echo "$verdict"

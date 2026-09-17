#!/usr/bin/env bash
# pb-01: publish.sh must not delete bug files the branch holds and this machine's tree does not.
#
# The world's shape: a second machine stands the loop up, seeds loop/bugs from the store's curated
# files, and publishes. The branch's auto-drafts are in neither place afterwards.
#
#   pb-01.sh <publish.sh under test>
# exit 0 = the branch kept every file it had (and the new one arrived); 1 = files were deleted.
set -uo pipefail
PUB="${1:?pb-01.sh <publish.sh>}"
W=$(mktemp -d /tmp/pb-01.XXXXXX); trap 'rm -rf "$W"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=pb01 GIT_AUTHOR_EMAIL=pb01@local GIT_COMMITTER_NAME=pb01 GIT_COMMITTER_EMAIL=pb01@local

# A scratch remote whose qa-results branch already carries three drafts and one curated bug.
git init -q --bare "$W/remote.git"
git init -q "$W/seed" && cd "$W/seed" && git checkout -q --orphan qa-results
mkdir -p bugs
for f in 0017aeb03a.md 00beeac0e7.md 00c922a645.md qa-001-no-key-turn-left-unended.md; do echo "draft $f" > "bugs/$f"; done
git add -A && git commit -q -m seed && git push -q "$W/remote.git" qa-results
BEFORE=$(git ls-tree -r --name-only HEAD | grep '^bugs/' | sort)

# This machine's loop tree: the curated file only, plus one new finding of its own.
# rollouts/ must exist: a loop tree always has one, and without it publish.sh dies before the push
# and the probe passes for a reason that has nothing to do with the guard (caught 2026-09-17 12:35).
ROOT="$W/root"; mkdir -p "$ROOT/loop/bugs" "$ROOT/loop/rollouts" "$ROOT/state"
: > "$ROOT/loop/rollouts/index.jsonl"
echo "draft qa-001" > "$ROOT/loop/bugs/qa-001-no-key-turn-left-unended.md"
echo "new finding"  > "$ROOT/loop/bugs/qal-j99-new-from-this-machine.md"

ARBOS_QA_ROOT="$ROOT" ARBOS_QA_RESULTS_REMOTE="$W/remote.git" ARBOS_GITHUB=unused \
  bash "$PUB" push > "$W/push.log" 2>&1
rc=$?
echo "-- publish.sh exit $rc"; sed 's/^/   /' "$W/push.log"

git clone -q --branch qa-results --single-branch "$W/remote.git" "$W/after"
AFTER=$(git -C "$W/after" ls-tree -r --name-only HEAD | grep '^bugs/' | sort)
lost=$(comm -23 <(echo "$BEFORE") <(echo "$AFTER"))
gained=$(comm -13 <(echo "$BEFORE") <(echo "$AFTER"))
echo "-- branch before: $(echo "$BEFORE" | wc -l) file(s); after: $(echo "$AFTER" | wc -l)"
[ -n "$gained" ] && echo "-- gained: $(echo "$gained" | tr '\n' ' ')"
if [ -n "$lost" ]; then
  echo "BREAK publish-deleted-bug-files-the-branch-held: $(echo "$lost" | wc -l) lost — $(echo "$lost" | tr '\n' ' ')"
  exit 1
fi
# A pass must prove it looked at something: the branch had files this tree lacked in the first place.
missing_here=$(comm -23 <(echo "$BEFORE" | sed 's|^bugs/||') <(ls "$ROOT/loop/bugs" | sort))
if [ -z "$missing_here" ]; then
  echo "SKIP probe-staged-no-divergence: the branch held nothing this tree lacks, so nothing could be deleted"
  exit 2
fi
echo "PASS: the branch kept all $(echo "$BEFORE" | wc -l) file(s) while this tree lacked $(echo "$missing_here" | wc -l) of them"
exit 0

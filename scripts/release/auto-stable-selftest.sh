#!/usr/bin/env bash
# `is_hotfix` decides whether a commit takes the path that publishes to every Mac WITHOUT waiting
# for CI, so no text a contributor writes may set it. This exercises the shipped function against
# synthetic history shaped like a real GitHub merge.
#
# The function is lifted out of auto-stable.sh rather than re-typed: the script runs `git fetch` at
# the top level, so it cannot be sourced. If `is_hotfix` is renamed or reshaped, the extraction
# finds nothing and this test fails loudly rather than silently passing.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
src="$here/auto-stable.sh"
fn="$(sed -n '/^is_hotfix()/,/^}/p' "$src")"
[ -n "$fn" ] || { echo "FAIL: no is_hotfix() in $src"; exit 1; }
eval "$fn"

repo="$(mktemp -d)"
trap 'rm -rf "$repo"' EXIT
cd "$repo"
git init -q .
git config user.email t@example.com
git config user.name test
export GIT_AUTHOR_DATE="2026-01-01T00:00:00Z" GIT_COMMITTER_DATE="2026-01-01T00:00:00Z"

commit() { git commit -q --allow-empty -m "$1" ${2+-m "$2"}; }

fails=0
check() { # check <expected yes|no> <sha> <what>
  local want=$1 sha=$2 what=$3 got=no
  is_hotfix "$sha" && got=yes
  if [ "$got" = "$want" ]; then
    echo "ok   $what -> $got"
  else
    echo "FAIL $what -> $got, wanted $want"
    fails=$((fails + 1))
  fi
}

commit "chore: base"
base=$(git rev-parse HEAD)

# A contributor's branch commit, merged. GitHub's merge subject is its own template plus the branch
# name, and its body is the PR TITLE -- both of the author's text used to reach the marker.
git checkout -q -b contrib
commit 'Revert "test(ios): DEBUG-only transcript-bottom-inset probe"'
git checkout -q master 2>/dev/null || git checkout -q main
git merge -q --no-ff contrib \
  -m "Merge pull request #2448 from unarbos/cursor/revert-the-probe" \
  -m "test(ios): drop the transcript-bottom-inset probe"
check no "$(git rev-parse HEAD)" "a revert in the merged branch does not expedite"

# The same merge, but with the author's PR title itself a revert: GitHub puts it in the merge body.
git checkout -q -b contrib2 "$base"
commit "chore: ordinary work"
git checkout -q master 2>/dev/null || git checkout -q main
git merge -q --no-ff contrib2 \
  -m "Merge pull request #2449 from unarbos/cursor/hotfix-please" \
  -m 'Revert "the thing" [hotfix]'
check no "$(git rev-parse HEAD)" "a PR title in the merge body does not expedite"

# The operator's own paths still work: a direct push to main, and a marked merge subject.
commit "hotfix: the relay refuses every room"
check yes "$(git rev-parse HEAD)" "an operator's direct hotfix commit expedites"
commit 'Revert "the change that broke sign-in"'
check yes "$(git rev-parse HEAD)" "an operator's direct revert expedites"
commit "Merge pull request #2450 from unarbos/fix [hotfix]"
check yes "$(git rev-parse HEAD)" "an operator's marked merge subject expedites"
commit "feat: ordinary change"
check no "$(git rev-parse HEAD)" "an ordinary commit does not expedite"

[ "$fails" -eq 0 ] || { echo "$fails failed"; exit 1; }
echo "all ok"

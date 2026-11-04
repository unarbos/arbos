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

# A case expecting `no` is only evidence if its fixture really carries the marker where it claims
# to. An empty or mistyped body would make such a case pass for the wrong reason -- green whether
# or not the predicate still reads `%b`. So the premise gets asserted alongside the conclusion.
body_carries() { # body_carries <sha> <regex> <what>
  if ! git log -1 --format='%b' "$1" | grep -qiE "$2"; then
    echo "FAIL fixture for '$3': its body does not match $2, so the case proves nothing"
    fails=$((fails + 1))
  fi
}

merged_commits_carry() { # merged_commits_carry <merge sha> <regex> <what>
  if ! git log --format='%s' "$1^1..$1" | grep -qiE "$2"; then
    echo "FAIL fixture for '$3': the merge brings in no subject matching $2, so the case proves nothing"
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
merged_commits_carry "$(git rev-parse HEAD)" '^revert\b' "a revert in the merged branch"
check no "$(git rev-parse HEAD)" "a revert in the merged branch does not expedite"

# The same merge, but the marker is in the author's own PR title: GitHub puts the PR TITLE in the
# merge body, so `%b` is author-controlled and must not be read.
#
# The revert-prefix half and the `[hotfix]` half are separate cases on purpose. They are independent
# sources, so one failure now names which one came back; a single fixture carrying both markers does
# detect either regression, but reports only that something did. It also invites the wrong repair:
# the obvious way to quiet a combined failure is to trim the marker that looks redundant out of the
# fixture, which reopens that half of the hole while the case goes green again. Hence `body_carries`
# below -- each case states the premise it depends on, so trimming it fails loudly instead.
git checkout -q -b contrib2 "$base"
commit "chore: ordinary work"
git checkout -q master 2>/dev/null || git checkout -q main
git merge -q --no-ff contrib2 \
  -m "Merge pull request #2449 from unarbos/cursor/revert-the-thing" \
  -m 'Revert "the thing that broke the relay"'
body_carries "$(git rev-parse HEAD)" '^revert\b' "a revert-prefixed PR title in the merge body"
check no "$(git rev-parse HEAD)" "a revert-prefixed PR title in the merge body does not expedite"

git checkout -q -b contrib3 "$base"
commit "chore: more ordinary work"
git checkout -q master 2>/dev/null || git checkout -q main
git merge -q --no-ff contrib3 \
  -m "Merge pull request #2450 from unarbos/cursor/hotfix-please" \
  -m 'fix: the relay refuses every room [hotfix]'
body_carries "$(git rev-parse HEAD)" '\[hotfix\]' "a [hotfix] marker in the merge body"
check no "$(git rev-parse HEAD)" "a [hotfix] marker in the merge body does not expedite"

# The operator's own paths still work: a direct push to main, and a marked merge subject.
commit "hotfix: the relay refuses every room"
check yes "$(git rev-parse HEAD)" "an operator's direct hotfix commit expedites"
commit 'Revert "the change that broke sign-in"'
check yes "$(git rev-parse HEAD)" "an operator's direct revert expedites"
commit "Merge pull request #2451 from unarbos/fix [hotfix]"
check yes "$(git rev-parse HEAD)" "an operator's marked merge subject expedites"
commit "feat: ordinary change"
check no "$(git rev-parse HEAD)" "an ordinary commit does not expedite"

[ "$fails" -eq 0 ] || { echo "$fails failed"; exit 1; }
echo "all ok"

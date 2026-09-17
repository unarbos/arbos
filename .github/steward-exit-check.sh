#!/usr/bin/env bash
# Work that leaves the steward's view must leave it by landing on main.
#
# Two ways it can leave without landing, both seen on 2026-09-17, both one
# `git merge-base --is-ancestor` from being caught:
#
#   1. A PR closes without merging. GitHub does that to a stacked PR when its
#      base branch is deleted, and an open-queue view cannot tell it from a
#      merge. (#372 was thought lost this way; it was on main. The check is
#      the same either way.)
#   2. A commit lands on a branch after its PR merged. The PR stays "merged",
#      the queue never shows the branch again, and the commit sits there.
#      (63666ae0 — the kernel update's pre-swap probe and rollback copy —
#      pushed two minutes after #318 merged, orphaned for eighteen hours
#      while main shipped `arbos-kernel update` with no net.)
#
# Both are answered against the tree, not the PR's state:
#
#   Part 1 lists every PR closed in the last WINDOW hours and checks that
#   its head is an ancestor of origin/main. NOT IN MAIN fails the run.
#
#   Part 2 lists every remote branch with no open PR whose tip is not an
#   ancestor of origin/main, with the commits it holds and the tip's age.
#   Anything older than STALE hours fails the run: a stale branch with
#   unlanded commits is forgotten work or a merge that went sideways. Names
#   only — it never opens a PR.
#
# An empty or failed listing is an error, not a pass.
#
# Usage: .github/steward-exit-check.sh [WINDOW_HOURS] [STALE_HOURS]
#        (defaults 2 and 3)
# Needs: gh (authenticated), git with origin fetched.
set -euo pipefail

repo="${STEWARD_REPO:-unarbos/arbos}"
window_h="${1:-2}"
stale_h="${2:-3}"
# Branches that are not meant to land on main: mirrors the workers write to
# (store-docs, qa-results, store-watch) and the pre-Rust history. Space-
# separated; override with STEWARD_IGNORE_BRANCHES.
ignore="${STEWARD_IGNORE_BRANCHES:-main rust HEAD store-docs qa-results store-watch discord update arbos-matrix}"
since="$(date -u -d "-${window_h} hours" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
      || date -u -v-"${window_h}"H +%Y-%m-%dT%H:%M:%SZ)"
now="$(date -u +%s)"

git fetch -q --prune origin

rows="$(mktemp)"; open_heads="$(mktemp)"
trap 'rm -f "$rows" "$open_heads"' EXIT
lost=0

# ---- Part 1: PRs that closed ------------------------------------------------
gh pr list --repo "$repo" --state closed --limit 200 \
    --json number,closedAt,mergedAt,headRefOid,baseRefName,title \
    --jq ".[] | select(.closedAt >= \"$since\")
             | [.number, .closedAt, (.mergedAt // \"null\"), .headRefOid, .baseRefName, .title[0:80]]
             | @tsv" > "$rows"
if [ ! -s "$rows" ]; then
  echo "steward-exit-check: no PR closed in the last ${window_h}h (or the listing failed) — part 1 checked nothing."
else
  while IFS=$'\t' read -r number closed merged head base title; do
    [ -z "$number" ] && continue
    if [ "$merged" != "null" ]; then
      printf '#%s  merged      %s\n' "$number" "$title"
      continue
    fi
    if git cat-file -e "$head" 2>/dev/null && git merge-base --is-ancestor "$head" origin/main; then
      printf '#%s  IN MAIN     (closed, head %s contained)  %s\n' "$number" "${head:0:7}" "$title"
    else
      printf '#%s  NOT IN MAIN (closed %s, base %s, head %s)  %s\n' \
        "$number" "$closed" "$base" "${head:0:7}" "$title"
      lost=1
    fi
  done < "$rows"
fi

# ---- Part 2: branches with no open PR and commits main does not have --------
# The open PRs' branches are someone's live work; everything else with
# unlanded commits is either finished-and-forgotten or a merge that missed.
gh pr list --repo "$repo" --state open --limit 200 --json headRefName --jq '.[].headRefName' > "$open_heads"
if [ ! -s "$open_heads" ] && ! gh pr list --repo "$repo" --state open --limit 1 >/dev/null 2>&1; then
  echo "steward-exit-check: could not list open PRs — part 2 checked nothing."
  exit 2
fi

echo "--- branches with no open PR and commits not on main ---"
found=0
while read -r ref; do
  branch="${ref#origin/}"
  case " $ignore " in *" $branch "*) continue;; esac
  case "$branch" in legacy/*) continue;; esac
  grep -qxF "$branch" "$open_heads" && continue
  git merge-base --is-ancestor "$ref" origin/main && continue
  # A commit that was rebased or cherry-picked lands under another sha.
  # `git cherry` compares patch content: '+' marks a change main does not
  # have; '-' one it already has. Only the '+' lines are unlanded work.
  unlanded="$(git cherry origin/main "$ref" | awk '$1=="+"{print $2}')"
  [ -z "$unlanded" ] && continue
  found=1
  n="$(printf '%s\n' "$unlanded" | wc -l | tr -d ' ')"
  tip_epoch="$(git log -1 --format=%ct "$ref")"
  age_h=$(( (now - tip_epoch) / 3600 ))
  flag=""
  if [ "$age_h" -ge "$stale_h" ]; then flag="  STALE"; lost=1; fi
  printf '%s  %s commit(s) main does not have, tip %sh old%s\n' "$branch" "$n" "$age_h" "$flag"
  printf '%s\n' "$unlanded" | head -n 5 | while read -r c; do
    git log -1 --format='    %h %ad %s' --date=format:%Y-%m-%dT%H:%MZ "$c" | cut -c1-150
  done
done < <(git for-each-ref --format='%(refname:short)' refs/remotes/origin)
[ "$found" = 0 ] && echo "(none)"

if [ "$lost" = 1 ]; then
  echo "steward-exit-check: work left the queue without landing on main — look before reporting."
  exit 1
fi
echo "steward-exit-check: every PR closed in the last ${window_h}h is on main, and no branch without a PR holds unlanded commits older than ${stale_h}h."

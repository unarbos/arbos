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
# Record branches: orphan branches that exist to hold documents and reader
# verdicts and are never meant to merge into main. Listed by exact name, not
# pattern, so a genuinely orphaned feature branch cannot hide behind a
# similar name. The pre-Rust history is listed here too. Space-separated;
# override with STEWARD_IGNORE_BRANCHES.
ignore="${STEWARD_IGNORE_BRANCHES:-main rust HEAD store-docs store-watch qa-results cursor/store-docs-94d6 discord update arbos-matrix}"
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
# Two populations, reported apart so the loud one is not drowned:
#   A. the branch had a PR that merged, and commits were pushed after — work
#      that was reviewed and then lost (63666ae0). Loud, with its age.
#   B. the branch never had a merged PR — old integration branches, drafts
#      nobody opened. A list for owners to confirm dead or open a PR.
gh pr list --repo "$repo" --state open --limit 200 --json headRefName --jq '.[].headRefName' > "$open_heads"
if [ ! -s "$open_heads" ] && ! gh pr list --repo "$repo" --state open --limit 1 >/dev/null 2>&1; then
  echo "steward-exit-check: could not list open PRs — part 2 checked nothing."
  exit 2
fi
merged_heads="$(mktemp)"; after="$(mktemp)"; never="$(mktemp)"
trap 'rm -f "$rows" "$open_heads" "$merged_heads" "$after" "$never"' EXIT
gh pr list --repo "$repo" --state merged --limit 500 --json number,headRefName,mergedAt \
    --jq '.[] | [.headRefName, .number, .mergedAt] | @tsv' > "$merged_heads"
if [ ! -s "$merged_heads" ]; then
  echo "steward-exit-check: could not list merged PRs — part 2 checked nothing."
  exit 2
fi

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
  n="$(printf '%s\n' "$unlanded" | wc -l | tr -d ' ')"
  tip_epoch="$(git log -1 --format=%ct "$ref")"
  age_h=$(( (now - tip_epoch) / 3600 ))
  lines="$(printf '%s\n' "$unlanded" | head -n 5 | while read -r c; do
    git log -1 --format='    %h %ad %s' --date=format:%Y-%m-%dT%H:%MZ "$c" | cut -c1-150; done)"
  pr="$(awk -F'\t' -v b="$branch" '$1==b {print "#"$2" merged "$3; exit}' "$merged_heads")"
  if [ -n "$pr" ]; then
    flag=""
    if [ "$age_h" -ge "$stale_h" ]; then flag="  STALE"; lost=1; fi
    printf '%s  %s commit(s) pushed after %s, tip %sh old%s\n%s\n' "$branch" "$n" "$pr" "$age_h" "$flag" "$lines" >> "$after"
  else
    printf '%s  %s commit(s), tip %sh old, no merged PR\n%s\n' "$branch" "$n" "$age_h" "$lines" >> "$never"
  fi
done < <(git for-each-ref --format='%(refname:short)' refs/remotes/origin)

echo "--- A. commits pushed after the branch's PR merged (reviewed work, then lost) ---"
if [ -s "$after" ]; then cat "$after"; else echo "(none)"; fi
echo "--- B. branches with unlanded commits and no merged PR (owners: confirm dead, or open a PR) ---"
if [ -s "$never" ]; then cat "$never"; else echo "(none)"; fi

if [ "$lost" = 1 ]; then
  echo "steward-exit-check: work left the queue without landing on main — look before reporting."
  exit 1
fi
echo "steward-exit-check: every PR closed in the last ${window_h}h is on main, and no branch holds commits pushed after its PR merged that are older than ${stale_h}h."

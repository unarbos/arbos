#!/usr/bin/env bash
# A PR that leaves the open queue must leave it by landing on main.
#
# GitHub closes a stacked PR without merging when its base branch is
# deleted, and a steward reading only the open queue cannot tell that from
# a merge. This lists every PR closed in the last WINDOW hours and checks,
# against the tree rather than the PR's state, whether its head commit is an
# ancestor of origin/main. Anything that is not is printed as NOT IN MAIN
# and the script exits 1, so a pass cannot end green over a lost PR.
#
# Usage: .github/steward-exit-check.sh [WINDOW_HOURS] (default 2)
# Needs: gh (authenticated), git with origin/main fetched.
set -euo pipefail

repo="${STEWARD_REPO:-unarbos/arbos}"
window_h="${1:-2}"
since="$(date -u -d "-${window_h} hours" +%Y-%m-%dT%H:%M:%SZ 2>/dev/null \
      || date -u -v-"${window_h}"H +%Y-%m-%dT%H:%M:%SZ)"

git fetch -q origin main

# The listing first, into a file, so an empty or failed read is an error and
# not a pass: a check that sees nothing must not say "all on main".
rows="$(mktemp)"
trap 'rm -f "$rows"' EXIT
gh pr list --repo "$repo" --state closed --limit 200 \
    --json number,closedAt,mergedAt,headRefOid,baseRefName,title \
    --jq ".[] | select(.closedAt >= \"$since\")
             | [.number, .closedAt, (.mergedAt // \"null\"), .headRefOid, .baseRefName, .title[0:80]]
             | @tsv" > "$rows"
if [ ! -s "$rows" ]; then
  echo "steward-exit-check: no PR closed in the last ${window_h}h (or the listing failed) — nothing checked."
  exit 2
fi

lost=0
while IFS=$'\t' read -r number closed merged head base title; do
  [ -z "$number" ] && continue
  if [ "$merged" != "null" ]; then
    printf '#%s  merged      %s\n' "$number" "$title"
    continue
  fi
  # Closed without a merge. The head may still be on main (merged from a
  # stacked branch, or cherry-picked under another sha); only the tree knows.
  if git cat-file -e "$head" 2>/dev/null && git merge-base --is-ancestor "$head" origin/main; then
    printf '#%s  IN MAIN     (closed, head %s contained)  %s\n' "$number" "${head:0:7}" "$title"
  else
    printf '#%s  NOT IN MAIN (closed %s, base %s, head %s)  %s\n' \
      "$number" "$closed" "$base" "${head:0:7}" "$title"
    lost=1
  fi
done < "$rows"

if [ "$lost" = 1 ]; then
  echo "steward-exit-check: a PR left the queue without landing on main — look before reporting."
  exit 1
fi
echo "steward-exit-check: every PR closed in the last ${window_h}h is on main."

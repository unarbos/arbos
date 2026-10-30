#!/usr/bin/env bash
# Every `uses:` in .github/workflows must name a 40-hex commit, not a tag or a branch.
#
# A tag and a branch are both movable by the action's author, and four of our jobs hold real
# credentials while a third-party action runs in them: the Apple Developer ID certificate and the
# notary key (release.yml `macos`), the 1Password service-account token that reads the whole Arbos
# vault (release.yml `publish`, ios.yml `phone`), and the Daytona key (image.yml). A commit id is
# the only ref GitHub will not resolve to different bytes later.
#
#   scripts/check-action-pins.sh            # lint .github/workflows
#   scripts/check-action-pins.sh DIR        # lint another directory of workflow files
#
# Keep the tag that the commit came from as a trailing comment, so the pin stays greppable and a
# human can see what to bump: `uses: actions/checkout@<sha> # v4`.
set -euo pipefail

dir="${1:-.github/workflows}"
[[ -d "$dir" ]] || { echo "no such directory: $dir" >&2; exit 2; }

bad=0
while IFS= read -r line; do
  file="${line%%:*}"
  rest="${line#*:}"
  lineno="${rest%%:*}"
  ref="$(printf '%s' "${rest#*:}" | sed -E 's/.*uses:[[:space:]]*//; s/[[:space:]]*(#.*)?$//')"
  # A local action (`./path`) or a container (`docker://…`) has no author who can move it.
  case "$ref" in ./*|docker://*) continue ;; esac
  if [[ "$ref" != *@* ]]; then
    echo "$file:$lineno: uses: $ref has no ref at all; pin it to a commit id" >&2
    bad=1
    continue
  fi
  if [[ ! "${ref#*@}" =~ ^[0-9a-f]{40}$ ]]; then
    echo "$file:$lineno: uses: $ref is pinned by tag or branch; use the commit id and keep the tag as a trailing comment" >&2
    bad=1
  fi
done < <(grep -rEn '^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]' "$dir")

if [[ "$bad" -ne 0 ]]; then
  echo "" >&2
  echo "Resolve a tag with: gh api repos/<owner>/<repo>/commits/<tag> --jq .sha" >&2
  exit 1
fi
echo "action pins ok ($(grep -rEc '^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]' "$dir" | awk -F: '{n+=$2} END {print n}') uses in $dir)"

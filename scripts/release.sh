#!/usr/bin/env bash
# Cut a release by pushing a version tag, which the release workflow builds and
# publishes binaries from. A tag push needs only write access — unlike
# `gh workflow run release`, which requires repo admin (and 403s without it).
#
#   ./scripts/release.sh            # bump the patch from the latest vX.Y.Z tag
#   ./scripts/release.sh v0.2.0     # explicit version
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

version="${1:-}"

# Read existing tags from the remote so the auto-bump isn't fooled by a stale
# local view (someone else may have released since the last fetch).
git fetch --tags --quiet

if [[ -z "$version" ]]; then
  last="$(git tag -l 'v*' --sort=-v:refname | head -1)"
  last="${last:-v0.0.0}"
  IFS=. read -r major minor patch <<<"${last#v}"
  version="v${major}.${minor}.$((patch + 1))"
fi

if [[ ! "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$ ]]; then
  echo "invalid version: $version" >&2
  exit 1
fi

branch="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$branch" != "main" ]]; then
  echo "refusing to release from '$branch' — switch to main" >&2
  exit 1
fi
if [[ -n "$(git status --porcelain)" ]]; then
  echo "working tree is dirty — commit or stash before releasing" >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/$version" >/dev/null; then
  echo "tag $version already exists" >&2
  exit 1
fi

echo "Releasing $version"
git tag -a "$version" -m "$version"
git push origin "$version"
echo "Pushed $version. Follow the build with:"
echo "  gh run watch \$(gh run list --workflow=release.yml -L1 --json databaseId --jq '.[0].databaseId') --exit-status"

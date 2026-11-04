#!/usr/bin/env bash
# The last stable release, and the commit the Mac app was built from for it.
#
#   scripts/release/stable-tag-base.sh
#
# Prints two lines, `tag=<vX.Y.Z>` and `base=<commit>`, both empty when the repository has no
# stable tag yet. Requires the tags to be fetched; it reads only what is already local.
#
# `base` is not the tag's own commit. A release commit touches Cargo.toml and Cargo.lock and
# nothing else, so the tree the Mac app was built from is that commit's parent. Comparing against
# the tag commit itself would hide the bump's own version lines inside every later diff.
#
# This lived inside scripts/release/auto-stable.sh until the release gate
# (.github/workflows/release-gate.yml) needed the same two values. Two copies of this rule would
# let the gate and the release disagree about what "already released" means, which is the one
# thing neither may do.
set -euo pipefail

last_tag="$(git tag -l 'v[0-9]*.[0-9]*.[0-9]*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1 || true)"
tag_base=""
if [[ -n "$last_tag" ]]; then
  tag_base="$(git rev-parse "$last_tag^{commit}")"
  touched="$(git show --format= --name-only "$tag_base" | sort -u | tr '\n' ' ')"
  if [[ "$touched" == "Cargo.lock Cargo.toml " || "$touched" == "Cargo.toml " ]]; then
    tag_base="$(git rev-parse "$tag_base^")"
  fi
fi
echo "tag=$last_tag"
echo "base=$tag_base"

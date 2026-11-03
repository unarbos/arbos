#!/usr/bin/env bash
# Did the Mac app change between two commits? Called by scripts/release/auto-stable.sh to decide
# whether a merge cuts a `vX.Y.Z` tag, and exercised on its own by
# crates/arbos-update/tests/release_mac_change.rs.
#
#   scripts/release/mac-change.sh <base-commit> <target-commit>   (env: ARBOS_MAC_PATHS)
#
# Exit 0: no Mac change — do not release. Exit 1: the Mac app must be built and released.
# The exit codes follow `git diff --quiet`, which this replaces: 0 means "nothing to see here".
# One line on stdout says which it is and why, for the release notes and the workflow log.
#
# The difference from a plain `git diff --quiet -- $ARBOS_MAC_PATHS`: a Cargo.lock whose only
# change is the version numbers of our own workspace crates does not count. The release bump
# commit fast-forwards onto main, so every later commit carries its version lines in Cargo.lock,
# and Cargo.lock is a Mac path — which made every merge cut a Mac release, Mac code or not (see
# scripts/release/cargo-lock-change.py). Any real dependency change still counts.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
base="${1:?base commit}"
target="${2:?target commit}"
MAC_PATHS="${ARBOS_MAC_PATHS:-crates/arbos-mac crates/arbos-core crates/arbos-update Cargo.lock scripts/bundle-mac.sh site/assets/logo}"

# Every Mac path except Cargo.lock: any difference there is a Mac change, full stop.
sources=()
lock_watched=false
# shellcheck disable=SC2086
for path in $MAC_PATHS; do
  if [[ "$path" == Cargo.lock ]]; then lock_watched=true; else sources+=("$path"); fi
done

if (( ${#sources[@]} > 0 )) && ! git diff --quiet "$base" "$target" -- "${sources[@]}"; then
  # sed, not head: head closes the pipe early, and under `set -o pipefail` that would be a failure.
  files="$(git diff --name-only "$base" "$target" -- "${sources[@]}" | sed -n '1,3p' | tr '\n' ' ')"
  echo "Mac sources changed: ${files% }"
  exit 1
fi

if ! $lock_watched || git diff --quiet "$base" "$target" -- Cargo.lock; then
  echo "no Mac path changed"
  exit 0
fi

# Cargo.lock moved. Only a real dependency change counts, not our own crates' version numbers.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
if ! git show "$base:Cargo.lock" >"$work/base.lock" 2>/dev/null; then
  echo "Cargo.lock is missing at the base commit"
  exit 1
fi
if ! git show "$target:Cargo.lock" >"$work/target.lock" 2>/dev/null; then
  echo "Cargo.lock is missing at the target commit"
  exit 1
fi

if python3 "$here/cargo-lock-change.py" "$work/base.lock" "$work/target.lock"; then
  echo "Cargo.lock moved only our own crates' version numbers"
  exit 0
fi
echo "Cargo.lock changed a dependency"
exit 1

#!/usr/bin/env bash
# Did an artifact's sources change between two commits? The engine behind
# scripts/release/mac-change.sh (the Mac app) and the agent-image side of
# scripts/release/release-channel.sh. Exercised through both by
# crates/arbos-update/tests/release_mac_change.rs and crates/arbos-update/tests/release_channel.rs.
#
#   scripts/release/paths-change.sh <noun> <paths> <base-commit> <target-commit>
#
# `noun` names the artifact in the one line this prints ("Mac", "agent image"); `paths` is its
# path list as one space-separated string, from the file that owns that list.
#
# Exit 0: no change — do not release. Exit 1: the artifact must be built and released.
# The exit codes follow `git diff --quiet`, which this replaces: 0 means "nothing to see here".
# One line on stdout says which it is and why, for the release notes and the workflow log.
#
# The difference from a plain `git diff --quiet -- $paths`: a Cargo.lock whose only change is the
# version numbers of our own workspace crates does not count. The release bump commit
# fast-forwards onto main, so every later commit carries its version lines in Cargo.lock, and
# Cargo.lock is on both path lists — which made every merge cut a release, artifact code or not
# (see scripts/release/cargo-lock-change.py). Any real dependency change still counts.
#
# This was mac-change.sh's body until the agent image needed the same rule. Two copies of the
# lockfile rule would let the two artifacts disagree about what a release is, and the lockfile
# rule is the half that is easy to get subtly wrong.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
noun="${1:?what the artifact is called}"
PATHS="${2?the path list of that artifact}"
base="${3:?base commit}"
target="${4:?target commit}"

# Every watched path except Cargo.lock: any difference there is a change, full stop.
sources=()
lock_watched=false
# shellcheck disable=SC2086
for path in $PATHS; do
  if [[ "$path" == Cargo.lock ]]; then lock_watched=true; else sources+=("$path"); fi
done

if (( ${#sources[@]} > 0 )) && ! git diff --quiet "$base" "$target" -- "${sources[@]}"; then
  # sed, not head: head closes the pipe early, and under `set -o pipefail` that would be a failure.
  files="$(git diff --name-only "$base" "$target" -- "${sources[@]}" | sed -n '1,3p' | tr '\n' ' ')"
  echo "$noun sources changed: ${files% }"
  exit 1
fi

if ! $lock_watched || git diff --quiet "$base" "$target" -- Cargo.lock; then
  echo "no $noun path changed"
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

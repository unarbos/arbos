#!/usr/bin/env bash
# Did the Mac app change between two commits? Called by scripts/release/release-channel.sh to
# decide whether a merge cuts a `vX.Y.Z` tag, and exercised on its own by
# crates/arbos-update/tests/release_mac_change.rs.
#
#   scripts/release/mac-change.sh <base-commit> <target-commit>   (env: ARBOS_MAC_PATHS)
#
# Exit 0: no Mac change — do not release. Exit 1: the Mac app must be built and released.
# One line on stdout says which it is and why.
#
# The rule itself — including the Cargo.lock reading that keeps a carried-forward version bump
# from cutting a release — lives in scripts/release/paths-change.sh, because the agent image
# needs the same rule over a different list. This file is the Mac app's name for it: the path
# list from scripts/release/mac-paths.sh, and the word "Mac" in the message.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "$here/paths-change.sh" Mac "$("$here/mac-paths.sh")" "${1:?base commit}" "${2:?target commit}"

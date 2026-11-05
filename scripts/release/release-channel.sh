#!/usr/bin/env bash
# Which channel does a range of main release on, and why? Called by scripts/release/auto-stable.sh
# and exercised on its own by crates/arbos-update/tests/release_channel.rs.
#
#   scripts/release/release-channel.sh <mac-base> <image-base> <target>
#
# Always exits 0: this is a report, not a gate. The verdict is the `channel=` line on stdout.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mac_base="${1-}"
image_base="${2-}"
target="${3:?target commit}"

mac=no
mac_why="no Mac path changed"
if [[ -z "$mac_base" ]]; then
  mac=yes
  mac_why="no stable tag in this checkout to measure against"
elif [[ "$mac_base" == "$target" ]] || git merge-base --is-ancestor "$target" "$mac_base"; then
  mac_why="already released: $target is not newer than ${mac_base:0:8}"
elif ! mac_why="$("$here/mac-change.sh" "$mac_base" "$target")"; then
  mac=yes
fi

agent=no
agent_why="this script does not look at the agent image (${image_base:-no image tag} unread)"

channel=none
why="nothing to release"
if [[ "$mac" == yes ]]; then
  channel=stable
  why="$mac_why"
fi

echo "channel=$channel"
echo "mac=$mac"
echo "mac_why=$mac_why"
echo "agent=$agent"
echo "agent_why=$agent_why"
echo "why=$why"

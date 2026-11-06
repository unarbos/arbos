#!/usr/bin/env bash
# Which channel does a range of main release on, and why? Called by scripts/release/auto-stable.sh
# for every green push, every 15-minute backstop run and every `workflow_dispatch channel=auto`;
# exercised on its own by crates/arbos-update/tests/release_channel.rs.
#
#   scripts/release/release-channel.sh <mac-base> <image-base> <target>
#
# `mac-base`   the commit the last `vX.Y.Z` release built the Mac app from (stable-tag-base.sh).
# `image-base` the commit the last `image-v0.1.N` tag points at. Empty when there is none.
# `target`     the commit being considered for release.
#
# Always exits 0. This is a report, not a gate: the verdict is the `channel=` line on stdout, and
# every other line is the evidence for it. Nothing here touches the network or a remote.
#
#   channel=stable|agent|none
#   mac=yes|no        mac_why=<one line>
#   agent=yes|no      agent_why=<one line>
#   why=<the one line that explains `channel`>
#
# ── why there are two questions ────────────────────────────────────────────────────────────────
# There used to be one. auto-stable.sh asked mac-change.sh "did the Mac app change?", and a `no`
# became channel=none — which .github/workflows/release.yml reads as "this run publishes
# nothing", in every job. That is correct for a relay-only merge and wrong for an agentd one: a
# range touching only crates/arbos-agentd or image/ cut no tag, built no image and uploaded no
# artifact, and the fix sat on main until an unrelated Mac-touching merge carried it into a Mac
# cut. The scheduled backstop ran the same predicate, so it rescued nothing. The two artifacts
# have different sources, so they need different answers.
#
# ── why a range on both lists is one Mac cut ───────────────────────────────────────────────────
# crates/arbos-core and Cargo.lock are on both lists, so plenty of ranges move both. The Mac cut
# wins, because release.yml's Mac path already builds and pushes the agent image in its own
# `linux` job — and because signing and notarizing costs about fourteen minutes that must not be
# spent twice. The agent side still reports what it saw, so the log says it was folded in rather
# than missed.
#
# ── why an empty base is not the same answer on both sides ─────────────────────────────────────
# No `vX.Y.Z` tag at all means a repository that has never released, and the first release is a
# Mac release: measuring against nothing and cutting is right. No `image-v0.1.N` tag means a
# baseline that does not exist yet, and guessing one would cut an image for the whole history of
# the repository. So a missing stable tag is `mac=yes` and a missing image tag is `agent=no`, and
# both say which it is.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mac_base="${1-}"
image_base="${2-}"
target="${3:?target commit}"

# side <noun> <paths script> <base>  → `<yes|no>\t<reason>`
side() {
  local noun="$1" paths="$2" base="$3" reason
  if [[ "$base" == "$target" ]] || git merge-base --is-ancestor "$target" "$base"; then
    printf 'no\talready released: nothing has landed since %s\n' "${base:0:8}"
    return
  fi
  if reason="$("$here/paths-change.sh" "$noun" "$("$here/$paths")" "$base" "$target")"; then
    printf 'no\t%s\n' "$reason"
  else
    printf 'yes\t%s\n' "$reason"
  fi
}

if [[ -z "$mac_base" ]]; then
  mac=yes
  mac_why="no stable v*.*.* tag in this checkout, so the first release is a Mac release"
else
  IFS=$'\t' read -r mac mac_why < <(side Mac mac-paths.sh "$mac_base")
fi

if [[ -z "$image_base" ]]; then
  agent=no
  agent_why="no image-v0.1.N tag in this checkout to measure a range against"
else
  IFS=$'\t' read -r agent agent_why < <(side "agent image" agent-paths.sh "$image_base")
fi

if [[ "$mac" == yes ]]; then
  channel=stable
  why="$mac_why"
elif [[ "$agent" == yes ]]; then
  channel=agent
  why="$agent_why"
else
  channel=none
  why="neither artifact changed: Mac — $mac_why; agent image — $agent_why"
fi

echo "channel=$channel"
echo "mac=$mac"
echo "mac_why=$mac_why"
echo "agent=$agent"
echo "agent_why=$agent_why"
echo "why=$why"

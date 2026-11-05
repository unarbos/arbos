#!/usr/bin/env bash
# Everything the `arbos-agent` sandbox image is built from, one space-separated line.
#
#   scripts/release/agent-paths.sh            (override the whole list with ARBOS_AGENT_PATHS)
#
# Read by scripts/release/release-channel.sh, which decides whether a range of main cuts an
# `image-v0.1.N` release, and which writes the release notes from it. One list, because two
# copies let the decision and the notes disagree about what shipped — the same reasoning as
# scripts/release/mac-paths.sh, which this file is the agent image's twin of.
#
# ── the list is the image's closure, not a guess at it ─────────────────────────────────────────
# image/Dockerfile copies `dist/arbos-agentd` into the image and nothing else of ours, so the
# crates are the cargo closure of arbos-agentd: arbos-agentd and arbos-core. If the linker puts
# it in /usr/local/bin/arbos-agentd, a change to it must reach boxes.
# crates/arbos-update/tests/release_channel.rs holds this list to arbos-agentd's manifest, so a
# workspace crate added to that manifest and not to this list fails a test rather than shipping
# silently — the hole arbos-client and arbos-matrix sat in on the Mac list until 2026-11-04.
#
# `image` is the whole directory: Dockerfile, the pi and prime seeds, the hooks, arbos-shell and
# the Claude settings are all COPYed in, and each one changes the image without changing a line
# of Rust.
#
# `harness` is in the image too, and is the entry most easily missed because no `COPY` line names
# it: image/Dockerfile bind-mounts `harness/arbos-pi-ext` and copies it to
# /etc/arbos/pi-seed/extensions/arbos-pi-ext, then writes AGENTD_SHA beside it as `.source-sha`.
# That file is what scripts/deploy/arbos-deploy-worker's seed check reads to decide whether the
# image carries the extension its own commit says it does — so a harness fix that cut no image
# would be invisible twice over. The whole directory, matching `paths_of agentd` in that worker,
# because the side that rolls an image and the side that cuts one must not disagree about what
# counts as a change.
#
# Cargo.lock earns its place the way it does on the Mac list: it is not a source file, and our
# own crates' version numbers move in it after every release. paths-change.sh reads the lockfile
# rather than diffing it, so a real dependency change counts and a carried-forward version bump
# does not.
#
# Deliberately NOT on this list, though the image carries them:
#   * the self-reference checkout at /opt/arbos/src. It is `git fetch`ed at build time and
#     refreshed to main's tip at boot by selfref.rs, so it is not a reason to rebuild — and
#     listing it would mean "every commit in the repository", which is every merge cutting an
#     image.
#   * the Daytona snapshot registration (image/register-snapshot.sh is inside `image`, so it IS
#     watched; the snapshot itself is a deploy step, not a build input).
set -euo pipefail

echo "${ARBOS_AGENT_PATHS:-crates/arbos-agentd crates/arbos-core image harness Cargo.lock}"

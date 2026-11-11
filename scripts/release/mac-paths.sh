#!/usr/bin/env bash
# Everything the Mac app is built from, one space-separated line.
#
#   scripts/release/mac-paths.sh              (override the whole list with ARBOS_MAC_PATHS)
#
# Read by scripts/release/mac-change.sh, which decides whether a difference cuts a Mac release,
# and by scripts/release/auto-stable.sh, which writes the release notes from it. One list, because
# two copies let the decision and the notes disagree about what shipped.
#
# ── the list is the binary's closure, not a guess at it ─────────────────────────────────────────
# The crates are `cargo tree -p arbos-mac`: arbos-mac, arbos-core, arbos-client, arbos-matrix,
# arbos-update, arbos-wallet. If the linker puts it in Arbos.app, a change to it must reach Mac users.
#
# arbos-client and arbos-matrix were missing from this list until 2026-11-04, and the gap was a
# silent ship rather than a late one: a fix in either crate is compiled into the shipped binary
# while `mac-change.sh` answered "no Mac path changed", so no release was cut and Mac users never
# got it. #2376 is the live case — one file, crates/arbos-matrix/src/client.rs, read receipts kept
# private — which the old list called a non-release; of the 219 pull requests open when this
# landed it is the only one whose classification changes. .github/workflows/ios.yml has listed both
# crates for the phone since arbos-ffi started embedding them, with the reasoning written beside
# them; the Mac side never got the same reasoning, only a shorter list.
#
# Cargo.lock earns its place differently: it is not a source file, and our own crates' version
# numbers move in it after every release. mac-change.sh reads the lockfile rather than diffing it,
# so a real dependency change counts and a carried-forward version bump does not.
#
# scripts/bundle-mac.sh builds, signs, notarizes and staples the bundle; site/assets/logo is the
# app icon it embeds. Both change the artifact without changing a line of Rust.
#
# Adding a path here widens what cuts a release, so measure it first. Adding arbos-client and
# arbos-matrix flipped no verdict across main's last 60, 150 or 300 first-parent merges, so it
# cost nothing and closed the hole. Read 0-of-300 with suspicion, because a loop that measures
# nothing reports the same number: the control is 4cb121d64, which changes one file,
# crates/arbos-matrix/src/rows.rs. The same comparison calls it no Mac change on the old list and
# a Mac change on this one, so a flip is visible when there is one. It sits 1039 first-parent
# commits back, which is why no window reaches it.
#
# Any pull request that changes this file triggers the release gate
# (.github/workflows/release-gate.yml), which checks that its own trigger list still covers
# everything here.
set -euo pipefail

echo "${ARBOS_MAC_PATHS:-crates/arbos-mac crates/arbos-core crates/arbos-client crates/arbos-matrix crates/arbos-update crates/arbos-wallet Cargo.lock scripts/bundle-mac.sh site/assets/logo}"

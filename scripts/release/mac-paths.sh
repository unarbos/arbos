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
# arbos-update. If the linker puts it in Arbos.app, a change to it must reach Mac users.
#
# arbos-client and arbos-matrix were missing from this list until 2026-11-04, and the gap was a
# silent ship rather than a late one: a fix in either crate is compiled into the shipped binary
# while `mac-change.sh` answered "no Mac path changed", so no release was cut and Mac users never
# got it. #2376 is the live case — one file, crates/arbos-matrix/src/client.rs, read receipts kept
# private — which the old list called a non-release. .github/workflows/ios.yml has listed both
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
# Adding a path here widens what cuts a release. Measure it first: across main's last 60
# first-parent merges, adding arbos-client and arbos-matrix changed no verdict at all, so it cost
# nothing and closed the hole. Any pull request that changes this file triggers the release gate
# (.github/workflows/release-gate.yml), which checks that its own trigger list still covers
# everything here.
set -euo pipefail

echo "${ARBOS_MAC_PATHS:-crates/arbos-mac crates/arbos-core crates/arbos-client crates/arbos-matrix crates/arbos-update Cargo.lock scripts/bundle-mac.sh site/assets/logo}"

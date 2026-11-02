#!/bin/bash
# loop-public.sh [--verbose]: credential-free stand-in for merge-window.sh's loop_gating (no vault, no SSH).
# Run from the repo. Prints one line per finding and a final "LOOP-PUBLIC: CLEAR|UNSAFE — reason".
#
# The poller lands a feedback fix on main by one of two paths (scripts/feedback/feedback_poller.py gate()):
#   gated   (Rust or iOS files): only through an integration branch gate/<UTC stamp>-<n> on GitHub plus a CI
#           run on it, so an unlanded gate branch is the public sign that a landing may be in flight;
#   ungated (neither): merge_branch() on any 2-minute tick, with no public warning. Its only public trace is
#           the feedback/<id> branch itself, so every such branch ahead of main whose change reaches a release
#           or deploy path counts, unless it is pinned in KNOWN at the head the coordinator ruled on.
# Blind spots the host check (LOOP_SRC=host) does not have: it cannot see `status` or `do_not_merge` in the
# poller's state.json, so it cannot tell an operator-held or serialize-flagged item from a live one. That only
# makes it stricter for ungated branches; gated items cannot land without a visible gate/* run whatever they hold.
set -u
V=${1:-}
R=unarbos/arbos
GATE_FRESH_H=${GATE_FRESH_H:-6}
# fid@head8 pairs the coordinator has ruled safe to leave ahead of main (any new commit un-pins them).
KNOWN=${KNOWN:-}
unsafe=()

git fetch -q origin main '+refs/heads/feedback/*:refs/remotes/fb/*' '+refs/heads/gate/*:refs/remotes/gate/*' --prune 2>/dev/null \
  || { echo "LOOP-PUBLIC: UNSAFE — git fetch failed"; exit 1; }

now=$(date -u +%s)
for r in $(git for-each-ref --format='%(refname)' refs/remotes/gate/); do
  b=${r#refs/remotes/gate/}; stamp=$(echo "$b" | grep -oE '[0-9]{8}T[0-9]{6}' | head -1)
  git merge-base --is-ancestor "$r" origin/main && { [ -n "$V" ] && echo "  gate/$b: landed"; continue; }
  if [ -z "$stamp" ]; then unsafe+=("gate/$b has no readable timestamp"); continue; fi
  t=$(date -u -d "${stamp:0:8} ${stamp:9:2}:${stamp:11:2}:${stamp:13:2}" +%s)
  age_h=$(( (now - t) / 3600 ))
  if [ $age_h -lt "$GATE_FRESH_H" ]; then unsafe+=("gate/$b is unlanded and ${age_h} h old (a gated landing may be in flight)")
  else [ -n "$V" ] && echo "  gate/$b: unlanded, ${age_h} h old (abandoned)"; fi
done
for run in $(gh run list -R $R --workflow ci.yml --limit 30 --json headBranch,status --jq '.[] | select(.headBranch|startswith("gate/")) | select(.status!="completed") | .headBranch'); do
  unsafe+=("CI is running on $run")
done

# Release/deploy trigger paths a feedback branch can reach without the Rust/iOS gate.
risky='^(scripts/bundle-mac\.sh|site/assets/logo|scripts/deploy/|image/|harness/|voice/|scripts/build-xcframework\.sh|assets/avatars/trees/|rust-toolchain\.toml|\.github/workflows/(ios|ci|release)\.yml)'
gatedre='^(Cargo\.toml|Cargo\.lock|crates/|apps/ios/|packages/ArbosFFI/|\.github/workflows/ios\.yml$)'
for r in $(git for-each-ref --format='%(refname)' refs/remotes/fb/); do
  [ "$(git rev-list --count origin/main.."$r")" -gt 0 ] || continue
  fid=${r#refs/remotes/fb/}; h8=$(git rev-parse --short=8 "$r")
  files=$(git diff --name-only origin/main..."$r")
  if echo "$files" | grep -qE "$gatedre"; then [ -n "$V" ] && echo "  feedback/$fid@$h8: gated path (lands only via gate/*)"; continue; fi
  hit=$(echo "$files" | grep -E "$risky" | head -3 | tr '\n' ' ')
  [ -z "$hit" ] && { [ -n "$V" ] && echo "  feedback/$fid@$h8: ungated, no release/deploy path"; continue; }
  case " $KNOWN " in *" $fid@$h8 "*) echo "  feedback/$fid@$h8: ungated, reaches $hit— pinned by coordinator"; continue;; esac
  unsafe+=("feedback/$fid@$h8 can land ungated and reaches: $hit")
done

if [ ${#unsafe[@]} -eq 0 ]; then echo "LOOP-PUBLIC: CLEAR — no unlanded gate run and no unpinned ungated branch that reaches a release or deploy path"
else for u in "${unsafe[@]}"; do echo "  UNSAFE: $u"; done; echo "LOOP-PUBLIC: UNSAFE — ${#unsafe[@]} finding(s)"; fi

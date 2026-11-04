#!/usr/bin/env bash
# Does auto-stable.sh refuse to write when it is not running in CI?
#
#   scripts/release/auto-stable-release-guard-selftest.sh
#
# The guard decides whether the script may commit, tag and PUSH. A pushed `vX.Y.Z` tag is an
# unconditional instruction to build, sign, notarize and publish to every Mac, so a guard that
# opens when it should not is a public mistake. v0.6.382 is what that looks like.
#
# ── the harness strips its own pre-authorisation, and that is a case ───────────────────────────
# The guard honours GITHUB_ACTIONS=true. CI exports exactly that. So a suite run on CI inherits
# the bypass it is meant to be testing, every refusal case silently becomes an allow case, and the
# suite reports green having asked nothing — and a mutation hunt scored by those cases inherits
# the same free pass, so the surviving-mutant count reads 0 while meaning nothing (#2696).
#
# So: GITHUB_ACTIONS and ARBOS_RELEASE_BY_HAND are stripped at the boundary below, case 1 asserts
# the stripping worked, and exactly ONE case re-grants GITHUB_ACTIONS — case 3, whose entire
# purpose is to show the guard opens for CI. Every other case runs with both absent and says so.
#
# ── it runs the real script against a remote this file creates ─────────────────────────────────
# Not a copy of the guard and not the functions in isolation: the whole script, end to end, so the
# cases cover where the guard sits as well as what it decides. The remote is a bare repository in
# a temporary directory. Nothing here can reach `origin`, and the standing rule is that nobody
# runs auto-stable.sh against the real one without --dry-run.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/auto-stable.sh"
[[ -x "$script" ]] || { echo "FAIL: $script is not executable"; exit 1; }

# ── the boundary ───────────────────────────────────────────────────────────────────────────────
# Every pre-authorisation the script honours is removed here, once, for the whole suite. A case
# that wants one back exports it for that case alone. GH_TOKEN goes too: with no token the CI
# read fails, every commit reads `none`, and the walk reaches the scratch tip on its hotfix
# marker — which is what gives each case a release to cut or refuse without a network answer.
unset GITHUB_ACTIONS ARBOS_RELEASE_BY_HAND GH_TOKEN GITHUB_TOKEN ARBOS_MAC_PATHS
export GITHUB_REPOSITORY="arbos-selftest/no-such-repository"
export ARBOS_RELEASE_DEBOUNCE_S=0

fails=0
note() { printf '     %s\n' "$1"; }
ok()   { printf 'ok   %s\n' "$1"; }
bad()  { printf 'FAIL %s\n' "$1"; fails=$((fails + 1)); }

# ── a scratch repository with its own bare origin ──────────────────────────────────────────────
work=""
newrepo() {
  work="$(mktemp -d)"
  git init -q --bare "$work/origin.git"
  git init -q "$work/repo"
  cd "$work/repo"
  git remote add origin "$work/origin.git"
  # The VM that cut v0.6.382 had commit.gpgsign=true in its global config, which is how an SSH
  # signature ended up on a release commit the script does not sign. Off here, so the cases
  # measure the guard rather than this machine's git config.
  git config commit.gpgsign false
  git config tag.gpgsign false
  git config user.email selftest@example.com
  git config user.name selftest
  mkdir -p src crates/arbos-mac/src
  printf 'fn main() {}\n' > src/main.rs
  printf 'pub fn before() {}\n' > crates/arbos-mac/src/lib.rs
  cat > Cargo.toml <<'TOML'
[package]
name = "arbos-release-guard-scratch"
version = "0.8.0"
edition = "2021"

[workspace]
TOML
  cargo generate-lockfile --offline >/dev/null 2>&1
  git add -A
  git commit -q -m "chore: scratch base"
  # A previous release to count from. Without it `version` would equal Cargo.toml's own version,
  # the bump would be a no-op, and `git commit` would fail with "nothing to commit" — which is a
  # fixture fault that looks exactly like the guard refusing. Step 4 plans v0.8.1 from here.
  git tag -a v0.8.0 -m "scratch 0.8.0"
  # The tip carries a hotfix marker, so step 2 finds a target with no CI answer available, and it
  # changes a Mac path, so step 3's predicate says a release is owed. Both are needed before the
  # guard is reached at all: a case that stops earlier would pass for the wrong reason.
  printf 'pub fn after() {}\n' > crates/arbos-mac/src/lib.rs
  git commit -qam "hotfix: the scratch release this suite plans"
  git branch -qM main
  git push -q origin main
  git fetch -q origin
}
cleanup() { [[ -n "$work" ]] && rm -rf "$work"; }
trap cleanup EXIT

# Did this run write? Three separate observations, because a guard can leak through any of them:
# a tag anywhere, a commit past the tip the case started from, or a dirty worktree.
# v0.8.0 is the fixture's own tag, so the question is whether the release this run planned exists.
wrote() {
  local tip=$1
  git rev-parse -q --verify refs/tags/v0.8.1 >/dev/null && { echo "the local tag v0.8.1"; return 0; }
  [[ -n "$(git -C "$work/origin.git" tag -l)" ]] && { echo "a tag on the remote ($(git -C "$work/origin.git" tag -l | tr '\n' ' '))"; return 0; }
  [[ "$(git rev-parse HEAD)" != "$tip" ]] && { echo "a commit (HEAD moved off $tip)"; return 0; }
  [[ -n "$(git status --porcelain)" ]] && { echo "an unclean worktree"; return 0; }
  return 1
}

# run <case name> <expect wrote|clean> <phrase the output must contain> [env assignments...]
run() {
  local name=$1 expect=$2 phrase=$3; shift 3
  newrepo
  local tip out rc=0 effect
  tip="$(git rev-parse HEAD)"
  out="$(env "$@" "$script" 2>&1)" || rc=$?
  if (( rc != 0 )); then bad "$name — the script exited $rc"; note "$out"; cleanup; return; fi

  if effect="$(wrote "$tip")"; then
    if [[ "$expect" == wrote ]]; then ok "$name — wrote: $effect"; else
      bad "$name — expected to write nothing, but wrote $effect"; note "$out"
    fi
  else
    if [[ "$expect" == clean ]]; then ok "$name — wrote nothing"; else
      bad "$name — expected a release to be cut, nothing was written"; note "$out"
    fi
  fi

  # The effect alone cannot say WHY, and an exit code cannot either: every case here exits 0.
  # So each case also names a phrase only its own outcome produces.
  if grep -qF -- "$phrase" <<< "$out"; then ok "$name — said \"$phrase\""; else
    bad "$name — did not say \"$phrase\""; note "$out"
  fi
  cleanup
}

# ── 1. the stripping itself ────────────────────────────────────────────────────────────────────
# Without this case the suite cannot tell "every refusal held" from "CI granted every case the
# bypass". It is the cheapest case here and the one that makes the other six mean anything.
if [[ -z "${GITHUB_ACTIONS+x}" && -z "${ARBOS_RELEASE_BY_HAND+x}" ]]; then
  ok "the harness boundary stripped GITHUB_ACTIONS and ARBOS_RELEASE_BY_HAND"
else
  bad "the harness boundary did NOT strip its pre-authorisation (GITHUB_ACTIONS=${GITHUB_ACTIONS-unset}, ARBOS_RELEASE_BY_HAND=${ARBOS_RELEASE_BY_HAND-unset}); every refusal case below would be an allow case"
fi

# ── 2. the default: neither granted ────────────────────────────────────────────────────────────
run "outside CI, unauthorised" clean \
  "not CI; set ARBOS_RELEASE_BY_HAND=1 to mean it" \
  ARBOS_SELFTEST_CASE=unauthorised

# ── 3. the ONE case that re-grants the pre-authorisation ───────────────────────────────────────
# This is the only place GITHUB_ACTIONS is set in this file. Its purpose is to show the guard
# opens for CI; if it is ever copied to another case, case 1 stops being enough and the suite
# goes back to testing nothing.
run "in CI (GITHUB_ACTIONS=true)" wrote \
  "released: v0.8.1" \
  GITHUB_ACTIONS=true

# ── 4. the documented escape hatch, with CI still absent ───────────────────────────────────────
run "by hand, ARBOS_RELEASE_BY_HAND=1" wrote \
  "released: v0.8.1" \
  ARBOS_RELEASE_BY_HAND=1

# ── 5. --dry-run outranks authorisation ────────────────────────────────────────────────────────
# A lane told to test with --dry-run must get no tag out of it, including a local one: a stray
# local vX.Y.Z is what the next stable-tag-base.sh would answer with.
newrepo
dry_tip="$(git rev-parse HEAD)"
dry_out="$(env GITHUB_ACTIONS=true ARBOS_RELEASE_BY_HAND=1 "$script" --dry-run 2>&1)" || true
if dry_effect="$(wrote "$dry_tip")"; then
  bad "--dry-run with both grants set — wrote $dry_effect"; note "$dry_out"
else
  ok "--dry-run with both grants set — wrote nothing"
fi
grep -qF -- "(--dry-run)" <<< "$dry_out" \
  && ok "--dry-run said so in its reason" \
  || { bad "--dry-run did not name itself as the reason"; note "$dry_out"; }
cleanup

# ── 6, 7. only the literal string `true` authorises ────────────────────────────────────────────
# `[[ $x != true ]]` is the whole guard, so these are the readings that would slip through a
# truthiness mistake. Both must refuse.
run "GITHUB_ACTIONS=false" clean \
  "GITHUB_ACTIONS is 'false', not 'true'" \
  GITHUB_ACTIONS=false
run "GITHUB_ACTIONS=1" clean \
  "GITHUB_ACTIONS is '1', not 'true'" \
  GITHUB_ACTIONS=1

(( fails == 0 )) || { echo "$fails failed"; exit 1; }
echo "all ok"

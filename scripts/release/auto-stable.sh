#!/usr/bin/env bash
# Continuous delivery for the Mac app (D85): decide whether `main` needs a new stable release and,
# when it does, make the version bump commit and the `vX.Y.Z` tag. Called by the `plan` job of
# .github/workflows/release.yml for every green push to main (workflow_run), by the 15-minute
# schedule, and by `workflow_dispatch` with channel=auto. Prints the plan as key=value lines.
#
#   scripts/release/auto-stable.sh [--dry-run]      (env: GH_TOKEN or GITHUB_TOKEN, GITHUB_REPOSITORY,
#                                                    ARBOS_RELEASE_DEBOUNCE_S=150, ARBOS_MAC_PATHS,
#                                                    ARBOS_HOTFIX_ONLY=1 + ARBOS_PUSHED_SHA: the push-to-main
#                                                    trigger — do nothing unless the pushed commit is a hotfix)
#
# What it does:
#   1. waits ARBOS_RELEASE_DEBOUNCE_S so a batch of merges that lands within a few minutes shares
#      one release (a fix is never held longer than that) — not when a hotfix sits at the head of main;
#   2. target = the newest commit on origin/main whose CI run is green (commits whose CI is still
#      running are left for the next run; a red one is skipped; one whose CI state cannot be read
#      stops the run with an error, so no older commit ships in its place) — except a hotfix: a revert, or a
#      commit marked `hotfix:` / `[hotfix]`, is released as soon as it is on main, ahead of its CI
#      (P0 #1208: waiting for CI cost the crash fix two releases);
#   3. nothing to do when the paths the Mac app is built from did not change since the last
#      stable tag (scripts/release/mac-change.sh — a Cargo.lock that moved only our own crates'
#      version numbers is not a change; any real dependency change is), or when a stable tag
#      already sits on the target;
#   4. otherwise: version = next patch after the last stable tag (or Cargo.toml's version when
#      somebody bumped it higher by hand), `release: X.Y.Z — <what landed>` commit on top of the
#      target (Cargo.toml + Cargo.lock only), tag `vX.Y.Z` on that commit. The tag is pushed first
#      (always lands); main is fast-forwarded to the bump when it did not move meanwhile, else the
#      bump commit lives under its tag only and later runs read the version from the tags.
#      Pushes made with GITHUB_TOKEN start no workflows, so the caller builds this same version in
#      its own run (the `macos`/`linux`/`publish` jobs read `sha`/`version` from here).
#
# Output lines: channel=stable|none, publish=true|false, promote=true, sha=<bump commit or head>,
# version=<X.Y.Z or X.Y.Z.<count>>, build=<commit count>, notes=<one line>, tag=<vX.Y.Z or empty>.
set -euo pipefail

DRY_RUN=false
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=true
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="${GITHUB_REPOSITORY:-unarbos/arbos}"
TOKEN="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
DEBOUNCE="${ARBOS_RELEASE_DEBOUNCE_S:-150}"
# Everything the Mac binary and bundle are made from. Cargo.lock: a real dependency change ships
# too — our own crates' version numbers moving does not (mac-change.sh).
MAC_PATHS="${ARBOS_MAC_PATHS:-crates/arbos-mac crates/arbos-core crates/arbos-update Cargo.lock scripts/bundle-mac.sh site/assets/logo}"
# mac-change.sh reads the same list, so step 3 and the release notes always agree on it.
export ARBOS_MAC_PATHS="$MAC_PATHS"

say() { echo "::notice::$*" >&2; }

emit_none() {
  local head
  head="$(git rev-parse origin/main)"
  echo "channel=none"
  echo "publish=false"
  echo "promote=true"
  echo "sha=$head"
  echo "version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1).$(git rev-list --count "$head")"
  echo "build=$(git rev-list --count "$head")"
  echo "notes=$1"
  echo "tag="
}

ci_state() {
  # success | pending | failed | none for the push CI run of a commit. Returns non-zero when the state
  # could not be read: "none" means GitHub answered and the commit has no push CI run, never "unknown".
  local json
  json="$(curl -fsS --max-time 30 -H "Authorization: Bearer $TOKEN" -H "Accept: application/vnd.github+json" \
    "https://api.github.com/repos/$REPO/actions/workflows/ci.yml/runs?head_sha=$1&event=push&per_page=5")" || return 1
  [[ -n "$json" ]] || { echo "empty answer from the runs API" >&2; return 1; }
  python3 - "$json" <<'EOF'
import json, sys
answer = json.loads(sys.argv[1])
if not isinstance(answer, dict) or not isinstance(answer.get("workflow_runs"), list):
    sys.exit("the runs API answer has no workflow_runs list")
runs = answer["workflow_runs"]
if not runs:
    print("none"); sys.exit()
r = sorted(runs, key=lambda r: r.get("run_number", 0))[-1]
if r.get("status") != "completed":
    print("pending")
elif r.get("conclusion") == "success":
    print("success")
else:
    print("failed")
EOF
}

is_hotfix() {
  # A revert, or a commit marked `hotfix`, on main's first-parent line: its own subject, or (a merge)
  # the subjects of the commits it brings in. Case-insensitive; `[hotfix]` anywhere in the message.
  git log --format='%s%n%b' "$1^1..$1" 2>/dev/null | grep -qiE '^(revert\b|hotfix\b)|\[hotfix\]' \
    || git log -1 --format='%s%n%b' "$1" | grep -qiE '^(revert\b|hotfix\b)|\[hotfix\]'
}

git fetch -q --tags --force origin "+refs/heads/main:refs/remotes/origin/main"
head_main="$(git rev-parse origin/main)"
pushed="${ARBOS_PUSHED_SHA:-$head_main}"
if [[ "${ARBOS_HOTFIX_ONLY:-0}" == 1 ]] && ! is_hotfix "$pushed"; then
  emit_none "push ${pushed:0:8} to main is not a hotfix; it ships through the CI-green path"
  exit 0
fi
if (( DEBOUNCE > 0 )) && ! $DRY_RUN; then
  if is_hotfix "$head_main"; then
    say "hotfix at the head of main: no debounce"
  else
    say "debounce: waiting ${DEBOUNCE}s for sibling merges"
    sleep "$DEBOUNCE"
    git fetch -q --tags --force origin "+refs/heads/main:refs/remotes/origin/main"
  fi
fi

# 2. newest green commit on main — first-parent line only: a merged branch's own tip is reachable
#    from main too, but lacks everything main had before the merge.
#    Hotfix fast path (P0 #1208, 2026-10-21): a revert or a `hotfix`-marked commit whose CI is still
#    running (or has not started) is released at once, ahead of the newest green commit — waiting for
#    CI cost v0.6.91/v0.6.92 the crash fix. Everything else keeps the green-CI rule, and a hotfix whose
#    CI already failed is skipped like any red commit. (If its CI goes red after the release, the
#    feedback loop's "main is red" watcher raises it.)
target=""
hotfix=false
pending_seen=false
for sha in $(git rev-list --first-parent origin/main -n 25); do
  # An unreadable commit stops the plan: stepping past it would release an older commit in its place.
  if ! state="$(ci_state "$sha")"; then
    echo "::error title=Release plan stopped::cannot read the push CI state of ${sha:0:8}; no release this run rather than an older commit in its place" >&2
    exit 1
  fi
  case "$state" in
    success) target="$sha"; break ;;
    failed) ;;
    *)
      if is_hotfix "$sha"; then
        target="$sha"; hotfix=true
        say "hotfix ${sha:0:8} released ahead of its CI ($state)"
        break
      fi
      pending_seen=true ;;
  esac
done
if [[ -z "$target" ]]; then
  emit_none "no commit on main with a green CI run among the last 25"
  exit 0
fi
# 3. the last stable tag (highest vX.Y.Z anywhere — a bump commit may live only under its tag,
#    see below) and the commit it was released from: a release commit touches only
#    Cargo.toml/Cargo.lock, so its parent is what the Mac app was built from.
last_tag="$(git tag -l 'v[0-9]*.[0-9]*.[0-9]*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1 || true)"
tag_base=""
if [[ -n "$last_tag" ]]; then
  tag_base="$(git rev-parse "$last_tag^{commit}")"
  touched="$(git show --format= --name-only "$tag_base" | sort -u | tr '\n' ' ')"
  if [[ "$touched" == "Cargo.lock Cargo.toml " || "$touched" == "Cargo.toml " ]]; then
    tag_base="$(git rev-parse "$tag_base^")"
  fi
  if [[ "$tag_base" == "$target" ]] || git merge-base --is-ancestor "$target" "$tag_base"; then
    emit_none "stable $last_tag already covers the newest green commit ${target:0:8}"
    exit 0
  fi
  if mac_change="$("$here/mac-change.sh" "$tag_base" "$target")"; then
    emit_none "no Mac change since $last_tag — $mac_change (newest green commit ${target:0:8}$($pending_seen && echo '; newer commits still in CI'))"
    exit 0
  fi
  say "Mac release needed: $mac_change"
fi

# 4. version
git checkout -q --detach "$target"
base="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
last_v="${last_tag#v}"
if [[ -n "$last_v" && "$(printf '%s\n%s\n' "$base" "$last_v" | sort -V | tail -1)" == "$last_v" ]]; then
  IFS=. read -r maj min pat <<< "$last_v"
  version="$maj.$min.$((pat + 1))"
else
  # Cargo.toml is already ahead of every tag (a hand bump): release that version as it is.
  version="$base"
fi
if git rev-parse -q --verify "refs/tags/v$version" >/dev/null 2>&1; then
  emit_none "tag v$version exists but is not on the newest green commit; somebody is releasing by hand"
  exit 0
fi

# What landed: the subjects of the Mac-touching commits since the last stable tag, trimmed.
# shellcheck disable=SC2086
summary="$(git log --format=%s --no-merges "${last_tag:+$last_tag..}$target" -- $MAC_PATHS \
  | sed -E 's/^fix\(feedback\): //; s/^(fix|feat|chore|docs|refactor|test|build|ci)(\([^)]*\))?: //; s/ *\[feedback [^]]*\]//g' \
  | grep -v -E '^(release:|Merge )' | awk '!seen[$0]++ && n < 8 { n++; print }' \
  | awk '{ if (length($0) > 72) { s = substr($0, 1, 72); sub(/ [^ ]*$/, "", s); print s "…" } else print }' \
  | paste -sd ';' - | sed 's/;/; /g' || true)"
[[ -n "$summary" ]] || summary="changes since ${last_tag:-the start}"
if $hotfix; then summary="hotfix (released ahead of CI): $summary"; fi
subject="release: $version — $summary"
subject="${subject:0:480}"

sed -i.bak "0,/^version = \"$base\"/s//version = \"$version\"/" Cargo.toml && rm -f Cargo.toml.bak
cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace >/dev/null
git config user.name "arbos-release[bot]"
git config user.email "release@arbos.life"
git add Cargo.toml Cargo.lock
git commit -q -m "$subject" -m "Automatic stable release (continuous delivery): every green merge to main that changes the Mac app ships as the next patch version. Target $target; last stable ${last_tag:-none}."
bump="$(git rev-parse HEAD)"
git tag -a "v$version" -m "Arbos $version" "$bump"

if $DRY_RUN; then
  say "dry run: would tag v$version at $bump (target ${target:0:8}) and fast-forward main to it"
else
  # The tag is the release: it always lands (a new ref), even while main keeps moving. main is
  # fast-forwarded to the bump when nothing landed meanwhile; otherwise the bump commit lives
  # under its tag only, and the next release computes its number from the tags, not Cargo.toml.
  git push -q origin "refs/tags/v$version"
  if git push -q origin "HEAD:refs/heads/main" 2>/dev/null; then
    say "released: v$version at ${bump:0:8} (target ${target:0:8}); main fast-forwarded to the bump"
  else
    say "released: v$version at ${bump:0:8} (target ${target:0:8}); main moved meanwhile, the bump stays under its tag"
  fi
fi

echo "channel=stable"
echo "publish=true"
echo "promote=true"
echo "sha=$bump"
echo "version=$version"
echo "build=$(git rev-list --count "$bump")"
echo "notes=$subject"
echo "tag=v$version"

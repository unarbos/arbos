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
#      running are left for the next run; a red one is skipped) — except a hotfix: a revert, or a
#      commit marked `hotfix:` / `[hotfix]`, is released as soon as it is on main, ahead of its CI
#      (P0 #1208: waiting for CI cost the crash fix two releases);
#   3. which artifact the range since its last release changed, from
#      scripts/release/release-channel.sh: the Mac app (channel=stable), the `arbos-agent`
#      sandbox image alone (channel=agent), or neither (channel=none). Both answers and both
#      reasons go to the log on every run, cutting or not — see "saying why" below;
#   4a. stable: version = next patch after the last stable tag (or Cargo.toml's version when
#      somebody bumped it higher by hand), `release: X.Y.Z — <what landed>` commit on top of the
#      target (Cargo.toml + Cargo.lock only), tag `vX.Y.Z` on that commit. The tag is pushed first
#      (always lands); main is fast-forwarded to the bump when it did not move meanwhile, else the
#      bump commit lives under its tag only and later runs read the version from the tags.
#      Pushes made with GITHUB_TOKEN start no workflows, so the caller builds this same version in
#      its own run (the `macos`/`linux`/`publish` jobs read `sha`/`version` from here).
#   4b. agent: tag `image-v0.1.N` on the target itself. No version file holds the image version
#      and no bump commit is made — the tag is the whole record. `.github/workflows/image.yml`
#      builds, pushes and smoke-tests that tag, and release.yml's `agent-image` job is what
#      starts it, because a tag pushed with GITHUB_TOKEN starts no workflow of its own.
#
# ── saying why ─────────────────────────────────────────────────────────────────────────────────
# `channel=none` used to be one line with one reason in it, and a healthy skip (a relay-only
# merge) read exactly like the agentd hole (a fix nothing would ever release). Every run now logs
# both predicates, both reasons and both baselines, whichever way it goes.
#
# Output lines: channel=stable|agent|none, publish=true|false, promote=true, sha=<bump commit,
# target or head>, version=<X.Y.Z, 0.1.N or X.Y.Z.<count>>, build=<commit count>, notes=<one
# line>, tag=<vX.Y.Z, image-v0.1.N or empty>.
set -euo pipefail

DRY_RUN=false
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=true
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="${GITHUB_REPOSITORY:-unarbos/arbos}"
TOKEN="${GH_TOKEN:-${GITHUB_TOKEN:-}}"
DEBOUNCE="${ARBOS_RELEASE_DEBOUNCE_S:-150}"
# Everything the Mac binary and bundle are made from — the cargo closure of arbos-mac, plus what
# bundles it; scripts/release/mac-paths.sh owns the list and says why each entry is on it.
# Cargo.lock: a real dependency change ships too — our own crates' version numbers moving does not
# (mac-change.sh).
#
# This copy writes the release notes. mac-change.sh asks mac-paths.sh for itself, so step 3 and
# the notes agree because they read one file — not because this script hands the list down. An
# `export ARBOS_MAC_PATHS` used to sit here doing the latter. It was a no-op once both callers
# read mac-paths.sh (an operator's override reaches mac-change.sh on its own), and it read as
# this script overriding the list when it was only echoing the list back at itself.
MAC_PATHS="$("$here/mac-paths.sh")"
# The same arrangement for the agent image: scripts/release/agent-paths.sh owns that list,
# release-channel.sh asks it for itself, and this copy is only for the release notes.
AGENT_PATHS="$("$here/agent-paths.sh")"

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
  # success | pending | failed | none for the push CI run of a commit
  local json
  json="$(curl -fsS --max-time 30 -H "Authorization: Bearer $TOKEN" -H "Accept: application/vnd.github+json" \
    "https://api.github.com/repos/$REPO/actions/workflows/ci.yml/runs?head_sha=$1&event=push&per_page=5" 2>/dev/null || true)"
  [[ -n "$json" ]] || { echo none; return; }
  python3 - "$json" <<'EOF'
import json, sys
runs = json.loads(sys.argv[1]).get("workflow_runs", [])
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
  #
  # Here-strings, not pipes: grep -q exits on its first match, and under the pipefail above the
  # SIGPIPE'd git log would make the pipeline 141, so a marker that IS there reads as absent.
  # The largest first-parent range on main is ~137 KB against a floor near 160 KB, so this is
  # not firing today — it is one long commit message away from it. --self-test covers both ways.
  local re='^(revert\b|hotfix\b)|\[hotfix\]'
  grep -qiE "$re" <<<"$(git log --format='%s%n%b' "$1^1..$1" 2>/dev/null)" \
    || grep -qiE "$re" <<<"$(git log -1 --format='%s%n%b' "$1" 2>/dev/null)"
}

# Both directions of is_hotfix, on a throwaway repo it builds itself, with a commit message over
# the floor where a pipe into grep -q starts reporting a found marker as absent.
self_test_hotfix() {
  local d big yes_sha no_sha failed=0 saved=$PWD
  d="$(mktemp -d /tmp/auto-stable-selftest.XXXXXX)"
  git -C "$d" init -q
  git -C "$d" -c user.email=t@t -c user.name=t commit -q --allow-empty -m base
  # ~200 KB, over the floor. Passed by file: a single argv entry caps at 128 KB on Linux.
  big=$d/body.txt
  head -c 200000 /dev/zero | tr '\0' 'y' | fold -w 100 >"$big"
  # The marker sits in the subject, so grep matches at once while git log still has 200 KB to
  # write. A marker at the END of the body would be found even by the piped form.
  { echo "hotfix: urgent"; echo; cat "$big"; } >"$d/msg-yes.txt"
  { echo "feat: ordinary"; echo; cat "$big"; } >"$d/msg-no.txt"
  git -C "$d" -c user.email=t@t -c user.name=t commit -q --allow-empty -F "$d/msg-yes.txt"
  yes_sha="$(git -C "$d" rev-parse HEAD)"
  git -C "$d" -c user.email=t@t -c user.name=t commit -q --allow-empty -F "$d/msg-no.txt"
  no_sha="$(git -C "$d" rev-parse HEAD)"

  cd "$d"
  echo "== self-test: first-parent message is $(git log --format='%s%n%b' "$yes_sha^1..$yes_sha" | wc -c) bytes"
  if git log --format='%s%n%b' "$yes_sha^1..$yes_sha" 2>/dev/null | grep -qiE '^(revert\b|hotfix\b)|\[hotfix\]'; then
    echo "   self-test INVALID: the piped form still finds the marker, so this fixture is under"
    echo "                      the threshold and would pass against the unfixed code too."
    failed=1
  else
    echo "   threshold ok: the piped form reports the present marker as absent (this is the defect)"
  fi
  if is_hotfix "$yes_sha"; then echo "   positive control: a hotfix behind a 200 KB body is still a hotfix"
  else echo "   positive control FAILED: a hotfix read as ordinary"; failed=1; fi
  if is_hotfix "$no_sha"; then echo "   negative control FAILED: an ordinary commit read as a hotfix"; failed=1
  else echo "   negative control: an ordinary commit of the same size is not a hotfix"; fi

  cd "$saved"
  # The live case: main's largest first-parent range, 137,260 bytes carrying one marker.
  local live=5bbbe4fc5c75f895e0b67f6d0312a998649581cb
  if git cat-file -e "$live^{commit}" 2>/dev/null; then
    if is_hotfix "$live"; then echo "   live control: ${live:0:9} (137,260 B, 1 marker) still reads as a hotfix"
    else echo "   live control FAILED: ${live:0:9} no longer reads as a hotfix"; failed=1; fi
  fi

  rm -rf "$d"
  [[ $failed -eq 0 ]] || { echo "SELF-TEST FAILED"; return 1; }
  echo "   self-test passed"
}

if [[ "${1:-}" == "--self-test" ]]; then self_test_hotfix; exit $?; fi

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
  state="$(ci_state "$sha")"
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
# 3. the two baselines, and which artifact the range since them changed.
#
#    Mac: the last stable tag (highest vX.Y.Z anywhere — a bump commit may live only under its
#    tag, see below) and the commit it was released from. A release commit touches only
#    Cargo.toml/Cargo.lock, so its parent is what the Mac app was built from. Both come from
#    scripts/release/stable-tag-base.sh, which the pre-merge release gate reads too.
#
#    Agent image: the highest `image-v0.1.N` tag, on the commit it was built from — the tag sits
#    on that commit itself, because an image release makes no bump commit. The namespace is
#    shared with scripts/deploy/arbos-deploy-worker, which tags the same way when it rolls boxes.
#    Sharing it is the point: whichever side tags first, the other sees the range as released and
#    does not cut it twice.
tag_info="$("$here/stable-tag-base.sh")"
last_tag="$(sed -n 's/^tag=//p' <<< "$tag_info")"
tag_base="$(sed -n 's/^base=//p' <<< "$tag_info")"
image_tag="$(git tag -l 'image-v[0-9]*.[0-9]*.[0-9]*' | grep -E '^image-v[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1 || true)"
image_base=""
[[ -n "$image_tag" ]] && image_base="$(git rev-parse "$image_tag^{commit}")"

decision="$("$here/release-channel.sh" "$tag_base" "$image_base" "$target")"
channel="$(sed -n 's/^channel=//p' <<< "$decision")"
mac_verdict="$(sed -n 's/^mac=//p' <<< "$decision")"
mac_why="$(sed -n 's/^mac_why=//p' <<< "$decision")"
agent_verdict="$(sed -n 's/^agent=//p' <<< "$decision")"
agent_why="$(sed -n 's/^agent_why=//p' <<< "$decision")"

# Whichever way it goes, the log says why. A silent `channel=none` is what let an agentd-only
# range look like a healthy skip for as long as it did.
say "release decision at ${target:0:8}: channel=$channel"
say "  Mac         since ${last_tag:-no stable tag}: $mac_verdict — $mac_why"
say "  agent image since ${image_tag:-no image tag}: $agent_verdict — $agent_why"

if [[ "$channel" == none ]]; then
  emit_none "nothing to release at ${target:0:8} — Mac: $mac_why; agent image: $agent_why$($pending_seen && echo '; newer commits still in CI')"
  exit 0
fi

# What landed: the subjects of the commits that touched the releasing artifact's paths, trimmed.
landed() {  # landed <since-tag> <paths>
  local summary
  # shellcheck disable=SC2086
  summary="$(git log --format=%s --no-merges "${1:+$1..}$target" -- $2 \
    | sed -E 's/^fix\(feedback\): //; s/^(fix|feat|chore|docs|refactor|test|build|ci)(\([^)]*\))?: //; s/ *\[feedback [^]]*\]//g' \
    | grep -v -E '^(release:|Merge )' | awk '!seen[$0]++ && n < 8 { n++; print }' \
    | awk '{ if (length($0) > 72) { s = substr($0, 1, 72); sub(/ [^ ]*$/, "", s); print s "…" } else print }' \
    | paste -sd ';' - | sed 's/;/; /g' || true)"
  echo "${summary:-changes since ${1:-the start}}"
}

# 3b. the agent image: tag `image-v0.1.N` on the target and stop. No bump commit, because no file
#     in the tree holds the image version — the tag is the record, and image.yml reads it.
if [[ "$channel" == agent ]]; then
  image_version="${image_tag#image-v}"
  image_version="${image_version%.*}.$(( ${image_tag##*.} + 1 ))"
  new_image_tag="image-v$image_version"
  if git rev-parse -q --verify "refs/tags/$new_image_tag" >/dev/null 2>&1; then
    emit_none "$new_image_tag exists but is not the highest image tag; somebody is releasing by hand"
    exit 0
  fi
  subject="agent image: $image_version — $(landed "$image_tag" "$AGENT_PATHS")"
  subject="${subject:0:480}"
  git tag -a "$new_image_tag" -m "Arbos agent image $image_version" "$target"
  if $DRY_RUN; then
    say "dry run: would tag $new_image_tag at ${target:0:8}"
  else
    git push -q origin "refs/tags/$new_image_tag"
    say "agent image release: $new_image_tag at ${target:0:8}"
  fi
  echo "channel=agent"
  echo "publish=true"
  echo "promote=true"
  echo "sha=$target"
  echo "version=$image_version"
  echo "build=$(git rev-list --count "$target")"
  echo "notes=$subject"
  echo "tag=$new_image_tag"
  exit 0
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

summary="$(landed "$last_tag" "$MAC_PATHS")"
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

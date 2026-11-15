#!/usr/bin/env bash
# Continuous delivery for the Mac app (D85): decide whether `main` needs a new stable release and,
# when it does, make the version bump commit and the `vX.Y.Z` tag. Called by the `plan` job of
# .github/workflows/release.yml for every green push to main (workflow_run), by the 15-minute
# schedule, and by `workflow_dispatch` with channel=auto. Prints the plan as key=value lines.
#
#   scripts/release/auto-stable.sh [--dry-run | --self-test]
#                                                   (env: GH_TOKEN or GITHUB_TOKEN, GITHUB_REPOSITORY,
#                                                    ARBOS_RELEASE_DEBOUNCE_S=150, ARBOS_MAC_PATHS)
#
# What it does:
#   1. waits ARBOS_RELEASE_DEBOUNCE_S so a batch of merges that lands within a few minutes shares
#      one release (a fix is never held longer than that) — not when an unreleased commit on main
#      carries the fast-path marker `[release-now] ` at the start of its own subject;
#   2. target = the newest commit on origin/main whose CI run is green (commits whose CI is still
#      running are left for the next run; a red one is skipped). A green CI run is the ONLY thing
#      that can choose the target. While a marked commit's own CI is unfinished, nothing is
#      released at all — not it, and not the green commit under it, which would otherwise ship a
#      version that leaves the urgent fix out. See release_marker() for the marker and P0 #1208;
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

# ── the fast path's one entry point ────────────────────────────────────────────────────────────
# `[release-now] ` at the very START of the subject of the commit that is ON main's first-parent
# line, exact case, and nothing else. Never a body line. Never the subject or the body of a commit
# that a merge brings in.
#
# P0 #1208: the predecessor matched `^(revert\b|hotfix\b)|\[hotfix\]` with `grep -iE` over
# `%s%n%b` of the merge commit AND of every commit it brought in. grep is line-based and the
# pattern anchored with `^`, so a body line reading "Revert the earlier approach" armed it; so did
# a marked subject an author had written on another branch days earlier and never looked at again;
# so did any case variant. And what it armed was a release AHEAD OF the commit's own CI. That
# release is gone from this script (see the target loop below): the marker now buys priority and
# nothing else. It changes what is released and when that becomes eligible, never whether it was
# proven.
#
# Why this shape is right under all three merge methods the repository allows:
#   merge   The subject is generated — `Merge pull request #N from <owner>/<branch>` — and the PR
#           title goes in the BODY (merge_commit_title=MERGE_MESSAGE, merge_commit_message=PR_TITLE).
#           The only author text in that subject is the branch name, and `git check-ref-format`
#           rejects a `[` in a ref name, so no branch can carry the marker. This method therefore
#           cannot arm the fast path at all — the fail-closed direction, and the method
#           scripts/merge/merge.sh uses. Expediting a release means squash-merging by hand.
#   squash  squash_merge_commit_title=COMMIT_OR_PR_TITLE: the subject is the single commit's own
#           subject when the pull request has exactly one commit, and the PR TITLE otherwise. Both
#           are author text typed for this merge, so this is the way in. GitHub also lists the
#           brought-in subjects in the squash BODY, which this check does not read.
#   rebase  No merge commit: every commit lands as its own first-parent commit and is judged on its
#           own subject.
# Prose cannot arm it either, under any method: the marker has to START the subject.
RELEASE_MARKER='[release-now] '
# A subject longer than this is not one a person typed, and the fast path is not granted from a
# message this script cannot say it read whole. The longest subject anywhere on main is 1,249
# bytes, so this sits above every real commit with room to spare, and far under any buffer.
SUBJECT_MAX_B=8192

release_marker() {
  # The subject goes to a FILE — not a here-string, and not a pipe into grep. A here-string is
  # buffered, and the buffer this script used to depend on sat near 160 KB with main's largest
  # first-parent range already at ~137 KB; a buffer that silently truncates answers "no marker"
  # for a message that has one. A pipe is worse: `grep -q` exits on its first match, the SIGPIPE'd
  # `git log` makes the pipeline 141 under the `set -o pipefail` above, and a marker that IS there
  # reads as absent. A file has neither failure — `git log` writes all of it or fails loudly — and
  # `-1 --format=%s` makes the whole input one subject line.
  local f n prefix
  f="$(mktemp)" || return 1
  if ! git log -1 --format=%s "$1" >"$f" 2>/dev/null; then
    rm -f "$f"
    echo "::warning::fast path: no readable subject for ${1:0:9}; it stays closed" >&2
    return 1
  fi
  n="$(wc -c <"$f")"
  prefix="$(head -c "${#RELEASE_MARKER}" "$f")"
  rm -f "$f"
  if (( n > SUBJECT_MAX_B )); then
    # Fail closed and say so, rather than answer from a prefix of a message nobody can have meant.
    echo "::warning::fast path: ${1:0:9} has a ${n}-byte subject, over the ${SUBJECT_MAX_B}-byte cap; it stays closed" >&2
    return 1
  fi
  [[ "$prefix" == "$RELEASE_MARKER" ]]
}

newest_marked() {
  # The newest commit on main's first-parent line, since the last release, whose own subject
  # carries the marker — or nothing. Reads $tag_base, set in step 1 below, so that a marked commit
  # which already shipped counts as history rather than as a pending release. `-n 25` matches the
  # CI scan in step 2; a marker older than that has been overtaken anyway.
  #
  # Local, always: the fast path is decided from commit subjects this repository already has, never
  # from an event payload and never from a sha an outside caller names.
  local sha
  for sha in $(git rev-list --first-parent "${tag_base:+$tag_base..}origin/main" -n 25); do
    if release_marker "$sha"; then echo "$sha"; break; fi
  done
  return 0  # finding no marker is an answer, not a failure
}

# Both directions of release_marker(), on real commits and real merge commits in a throwaway
# repository — one per merge method, because the subject is a different piece of text under each —
# and then a read of this script's own source.
#
# Modelled on the deploy worker's agentd hold-file guard (scripts/deploy/arbos-deploy-worker,
# `hold_cases`): that one asserts the states AND counts the lines allowed to delete the hold file,
# so a second deleter fails the test the day it appears. The same split is the point here. The
# cases say what the check answers today; the source reads at the end say what this script may
# never do again. A case list on its own passes against the next widening, and the next widening
# is the whole of #1208.
self_test() {
  local d sha long cap_ok saved=$PWD st_fail=0
  st_ok()      { printf '   ok       %s\n' "$1"; }
  st_bad()     { printf '   FAILED   %s\n' "$1" >&2; st_fail=1; }
  st_arms()    { if release_marker "$1" 2>/dev/null; then st_ok "$2"; else st_bad "$2 — the marker did not arm it"; fi; }
  st_refuses() { if release_marker "$1" 2>/dev/null; then st_bad "$2 — this armed the fast path"; else st_ok "$2"; fi; }

  d="$(mktemp -d /tmp/auto-stable-selftest.XXXXXX)"
  # The repository is a subdirectory, so the message files below sit OUTSIDE its work tree. Written
  # inside it they are untracked changes, and `git checkout` refuses to move across them — which
  # silently builds a different fixture from the one the case describes.
  git init -q "$d/repo"
  # `git init` names the first branch main or master depending on the host, and every fixture
  # below branches off `main` by name.
  git -C "$d/repo" symbolic-ref HEAD refs/heads/main
  cd "$d/repo"
  git config user.name t
  git config user.email t@t
  # A developer whose global config signs commits would otherwise pay an ssh-keygen per commit
  # here, or fail outright without a key, for signatures nothing in this test reads.
  git config commit.gpgsign false
  git commit -q --allow-empty -m "base"

  # st_msg writes a commit message: first argument the subject, the rest the body, exactly as
  # GitHub lays one out.
  st_msg() { printf '%s\n' "$@" >"$d/msg"; }
  # One commit on <branch> (created off main when new) — also the rebase-method fixture when
  # <branch> is main, because a rebase merge lands the author's own commit on the first-parent line.
  st_work() {
    local b="$1"; shift
    if ! git checkout -q "$b" 2>/dev/null; then
      git checkout -q -b "$b" main || { st_bad "could not build the fixture branch $b; the cases after this one describe commits that do not exist"; return 1; }
    fi
    echo "$RANDOM.$RANDOM" >file.txt
    git add -A
    st_msg "$@"
    git commit -q -F "$d/msg"
  }
  # The merge method: a real two-parent merge commit on main, with the subject GitHub generates.
  st_merge() {
    local b="$1"; shift
    git checkout -q main
    st_msg "$@"
    git merge -q --no-ff -F "$d/msg" "$b"
    git rev-parse HEAD
  }
  # The squash method: one single-parent commit on main holding the branch's whole change.
  st_squash() {
    local b="$1"; shift
    git checkout -q main
    git merge -q --squash "$b" >/dev/null
    st_msg "$@"
    git commit -q -F "$d/msg"
    git rev-parse HEAD
  }

  echo "== self-test: the fast path arms from one explicit subject marker and from nothing else"

  # 1. The merge method, with the author's marker in every place that is NOT a subject on main's
  #    first-parent line: the PR title (which this method puts in the BODY), and the subject and
  #    body of both commits the branch brings in. One case, both of the widest routes in #1208.
  st_work route-body "$RELEASE_MARKER fix(agentd): the busy reply lands" "" "$RELEASE_MARKER in the body too"
  st_work route-body "fix(agentd): one more go" "" "$RELEASE_MARKER here as well"
  sha="$(st_merge route-body "Merge pull request #1208 from unarbos/cursor/busy-reply-9f3c" \
    "" "$RELEASE_MARKER fix(agentd): the busy reply lands")"
  st_refuses "$sha" "merge method: marker in the body (the PR title) and in both brought-in commits"

  # 2. The merge method's subject is machine text apart from the branch name. The assertion under
  #    the case is WHY that is safe: if git ever accepted a bracket in a ref name, a branch name
  #    would be the only author text left in that subject.
  st_work route-branch "fix(sync): keep the cache key stable"
  sha="$(st_merge route-branch "Merge pull request #1209 from unarbos/release-now/cache-key-4b1a" \
    "" "fix(sync): keep the cache key stable")"
  st_refuses "$sha" "merge method: the branch name reads release-now/…"
  if git check-ref-format --branch "unarbos/$RELEASE_MARKER" 2>/dev/null; then
    st_bad "git now accepts '$RELEASE_MARKER' in a branch name, so a branch name can reach the merge method's generated subject"
  else
    st_ok "a ref name cannot hold the marker, so the merge method's generated subject cannot carry it"
  fi

  # 3. Squash, one commit: GitHub uses that commit's own subject. The intended way in — and the
  #    case without which every assertion here would pass against a check that refuses everything.
  st_work route-one "$RELEASE_MARKER fix(agentd): the busy reply lands"
  sha="$(st_squash route-one "$RELEASE_MARKER fix(agentd): the busy reply lands")"
  st_arms "$sha" "squash of one commit: the marker starts the subject"

  # 4. Squash, two commits: GitHub uses the PR TITLE as the subject and lists the brought-in
  #    subjects in the body.
  st_work route-title "fix(agentd): the busy reply lands"
  st_work route-title "fix(agentd): one more go"
  sha="$(st_squash route-title "$RELEASE_MARKER fix(agentd): the busy reply lands (#1210)" \
    "" "* fix(agentd): the busy reply lands" "* fix(agentd): one more go")"
  st_arms "$sha" "squash of two commits: the marker starts the PR title"

  # 5. The inverse of 4, and the case a body reader cannot survive behind 4: ordinary PR title,
  #    marker in a brought-in subject, which GitHub copies into the squash body verbatim.
  st_work route-listed "$RELEASE_MARKER fix(agentd): the busy reply lands"
  st_work route-listed "fix(agentd): one more go"
  sha="$(st_squash route-listed "fix(agentd): the busy reply lands (#1211)" \
    "" "* $RELEASE_MARKER fix(agentd): the busy reply lands" "* fix(agentd): one more go")"
  st_refuses "$sha" "squash of two commits: the body lists a marked brought-in subject"

  # 6. Rebase: the author's commit IS the first-parent commit, so its own subject decides, and its
  #    own body still does not.
  st_work main "$RELEASE_MARKER fix(relay): drop the stale room"
  st_arms "$(git rev-parse HEAD)" "rebase method: the marker starts the commit's own subject"
  st_work main "fix(relay): drop the stale room" "" "$RELEASE_MARKER really, this one is urgent"
  st_refuses "$(git rev-parse HEAD)" "rebase method: the marker is in the commit's own body"

  # 7. Subjects that must not arm it: the case variants and the bare words the predecessor took,
  #    a mention in prose rather than at the start, `git revert`'s own generated subject, and the
  #    marker without the space that has to follow it.
  local subject
  for subject in "[Release-Now] fix(agentd): the busy reply lands" \
                 "[RELEASE-NOW] fix(agentd): the busy reply lands" \
                 "fix(release): narrow the ${RELEASE_MARKER% } handling" \
                 "Revert \"fix(agentd): the busy reply lands\"" \
                 "hotfix: the busy reply lands" \
                 "${RELEASE_MARKER% }fix(agentd): no space after the marker"; do
    st_work main "$subject"
    st_refuses "$(git rev-parse HEAD)" "subject: $subject"
  done

  # 8. The size guard, both halves. The long subject carries the marker at the start, so length is
  #    the only reason it is refused — and the same subject under the cap arms, which is what makes
  #    this a case about the cap rather than about the marker.
  long="$RELEASE_MARKER$(head -c $((SUBJECT_MAX_B + 512)) /dev/zero | tr '\0' y)"
  if (( ${#long} <= SUBJECT_MAX_B )); then
    st_bad "the long-subject fixture is ${#long} bytes, inside the ${SUBJECT_MAX_B}-byte cap, so case 8 proves nothing"
  fi
  st_work main "$long"
  st_refuses "$(git rev-parse HEAD)" "a marked subject of ${#long} bytes, over the ${SUBJECT_MAX_B}-byte cap"
  cap_ok="$RELEASE_MARKER$(head -c 200 /dev/zero | tr '\0' y)"
  st_work main "$cap_ok"
  st_arms "$(git rev-parse HEAD)" "the same marked subject at ${#cap_ok} bytes, inside the cap"

  cd "$saved"
  rm -rf "$d"

  # ── and what this script may never do again ───────────────────────────────────────────────────
  # Every case above passes on a return code, so a widening that keeps today's answers passes all
  # of them. These read the source instead. This is the half the cases cannot write, and the half
  # modelled on the deploy worker's hold-file count.
  #
  # What they read: this script, minus its own self-test, minus every comment line. Both
  # subtractions are load-bearing. These assertions quote the patterns they forbid, and the code
  # they guard EXPLAINS the patterns it forbids, so a scan over the raw file reports the prose and
  # never reaches the code.
  local me="${BASH_SOURCE[0]}" shipped fn n
  shipped="$(sed '/^self_test()/,/^}/d' "$me" | grep -v '^[[:space:]]*#')"
  fn="$(printf '%s\n' "$shipped" | sed -n '/^release_marker()/,/^}/p')"
  if [[ -z "$fn" ]]; then
    st_bad "no release_marker() in this script — it was renamed, and every assertion below is reading nothing"
    fn="(absent)"
  fi
  # And the subtraction took the self-test and nothing more. Deleting to the next `}` at column
  # zero would swallow the decision path too if a stray one ever appeared inside the test, and the
  # scans would then pass over a script that stops before it releases anything.
  if printf '%s\n' "$shipped" | grep -q 'git push -q origin "refs/tags/v\$version"'; then
    st_ok "the scans below read the whole decision path, with only the self-test removed"
  else
    st_bad "removing the self-test from the source also removed the release itself; the scans below are reading a fragment"
  fi

  n="$(printf '%s\n' "$shipped" | grep -c '%b' || true)"
  if [[ "$n" == 0 ]]; then
    st_ok "no line in this script reads a commit body"
  else
    st_bad "$n line(s) in this script read a commit body (%b); the fast path is decided from a subject"
  fi

  if printf '%s\n' "$fn" | grep -qE '\.\.|\^[0-9]'; then
    st_bad "release_marker() reads a revision range, so a commit a merge brings in can arm the fast path again"
  else
    st_ok "release_marker() reads no revision range, so no brought-in commit reaches it"
  fi

  n="$(printf '%s\n' "$fn" | grep -c 'git log' || true)"
  if [[ "$n" == 1 ]] && printf '%s\n' "$fn" | grep -q 'git log -1 --format=%s'; then
    st_ok "release_marker() reads one subject, from one commit, with one git log"
  else
    st_bad "release_marker() has $n git log call(s) and not exactly one \`git log -1 --format=%s\`"
  fi

  if printf '%s\n' "$fn" | grep -q '<<<'; then
    st_bad "release_marker() reads a here-string again; it is buffered, and a buffer that truncates answers 'no marker' for a message that has one"
  elif printf '%s\n' "$fn" | grep -qE '(^|[^|])\|([^|]|$)'; then  # a lone pipe, not an `||`
    st_bad "release_marker() pipes its input again; grep -q exits on its first match, the SIGPIPE'd git log makes the pipeline 141 under pipefail, and a marker that is there reads as absent"
  else
    st_ok "release_marker() reads a file: it can neither truncate nor lose its answer to a SIGPIPE"
  fi

  if printf '%s\n' "$fn" | grep -q 'grep'; then
    st_bad "release_marker() matches a pattern again; it compares a fixed-length prefix, which no flag can make case-insensitive or line-based"
  else
    st_ok "release_marker() compares a fixed-length prefix, not a pattern"
  fi

  if printf '%s\n' "$shipped" | grep -q 'release_marker "\$sha"'; then
    st_ok "the first-parent scan calls release_marker()"
  else
    st_bad "nothing in the decision path calls release_marker(), so the cases above test a function this script does not use"
  fi

  # The one assertion no state can make, and the reason #1208 was a P0 rather than a bug: exactly
  # one line chooses what gets released, and a green CI run is the only thing that reaches it. The
  # predecessor had a second such line in the arm for a commit whose CI had not finished.
  n="$(printf '%s\n' "$shipped" | grep -c 'target="\$sha"' || true)"
  if [[ "$n" == 1 ]] && printf '%s\n' "$shipped" | grep -qE '^ *success\) *target="\$sha"'; then
    st_ok "one line chooses the release target, in the arm for a green CI run"
  else
    st_bad "$n line(s) in this script set the release target; exactly one may, and only for a CI state of success"
  fi

  [[ $st_fail -eq 0 ]] || { echo "SELF-TEST FAILED" >&2; return 1; }
  echo "   self-test passed"
}

if [[ "${1:-}" == "--self-test" ]]; then self_test; exit $?; fi

git fetch -q --tags --force origin "+refs/heads/main:refs/remotes/origin/main"

# 1. the last stable tag (highest vX.Y.Z anywhere — a bump commit may live only under its tag, see
#    below) and the commit it was released from: a release commit touches only Cargo.toml and
#    Cargo.lock, so its parent is what the Mac app was built from. Both come from
#    scripts/release/stable-tag-base.sh, which the pre-merge release gate reads too. This is read
#    before anything else because the marker scan below has to stop at the last release: a marked
#    commit that already shipped is history, not a pending release.
tag_info="$("$here/stable-tag-base.sh")"
last_tag="$(sed -n 's/^tag=//p' <<< "$tag_info")"
tag_base="$(sed -n 's/^base=//p' <<< "$tag_info")"

marked="$(newest_marked)"

if (( DEBOUNCE > 0 )) && ! $DRY_RUN; then
  # The debounce exists so a batch of merges that lands within a few minutes shares one release.
  # A marked commit does not wait for company: it ships the moment it is proven. This is the whole
  # of what the marker buys on the clock, and it cannot shorten the wait for CI itself.
  if [[ -n "$marked" ]]; then
    say "marked commit ${marked:0:9} is unreleased: no debounce"
  else
    say "debounce: waiting ${DEBOUNCE}s for sibling merges"
    sleep "$DEBOUNCE"
    git fetch -q --tags --force origin "+refs/heads/main:refs/remotes/origin/main"
    # main moved while we waited, so the scan has to be redone: a commit that landed during the
    # wait is exactly the one whose release must not be held behind the next debounce.
    marked="$(newest_marked)"
  fi
fi

# 2. newest green commit on main — first-parent line only: a merged branch's own tip is reachable
#    from main too, but lacks everything main had before the merge.
#
#    A green CI run is the only thing that can choose the target. P0 #1208 (2026-10-21 to
#    2026-11-06) had a second arm here that made a marked commit the target while its CI was still
#    running, so the fast path released code nothing had proven — the same shape as a commit
#    subject acting as deploy authority in the deploy worker, deleted there on 2026-11-05 after one
#    recorded case rolled an unapproved image. What a marked commit gets instead is the arm below:
#    while its own CI is unfinished, NOTHING is released — not it, and not the green commit under
#    it either, because releasing that one would burn the next patch number on a build that leaves
#    the urgent fix out. So the marker changes what is released and when that becomes eligible, and
#    it never decides whether it was proven. A marked commit whose CI went red is skipped like any
#    other red commit; the release then falls through to the newest green commit below it, and the
#    feedback loop's "main is red" watcher raises the failure.
target=""
pending_seen=false
for sha in $(git rev-list --first-parent origin/main -n 25); do
  state="$(ci_state "$sha")"
  case "$state" in
    success) target="$sha"; break ;;
    failed) ;;
    *)
      if [[ "$sha" == "$marked" ]]; then
        emit_none "marked commit ${sha:0:8} is not proven yet (CI $state); nothing releases until it is"
        exit 0
      fi
      pending_seen=true ;;
  esac
done
if [[ -z "$target" ]]; then
  emit_none "no commit on main with a green CI run among the last 25"
  exit 0
fi
# 3. is that commit already released, and did the Mac app change since the last stable tag?
if [[ -n "$last_tag" ]]; then
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
# No line here says "released ahead of CI" any more, because no release is. A marked commit's own
# subject keeps its marker through the trim above, so the notes still say which release carried it.
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

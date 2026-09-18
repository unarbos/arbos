#!/usr/bin/env bash
# What the dev channel's `plan` step decides, checked against cases that
# have already cost something.
#
#     bash .github/dev-channel-plan-test.sh
#
# It runs the step's own script — extracted from the workflow, not a copy —
# against a throwaway git repository, with `gh` and `curl` stood in for so
# the CI states and the channel's current build are ours to choose.
#
# The case that matters is the third one. On 2026-09-17 build 1609 went
# green, stepped aside for 1611 as it was meant to, and 1611's CI failed.
# Nothing published, and the channel sat still for half an hour holding a
# green build that had given up its turn to a commit that never took it.
set -uo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
workflow="$root/.github/workflows/dev-channel.yml"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
failures=0

# --- the step under test, straight out of the workflow -------------------
python3 - "$workflow" "$tmp/plan-body.sh" <<'PY'
import sys, yaml
doc = yaml.safe_load(open(sys.argv[1]))
step = [s for s in doc["jobs"]["plan"]["steps"] if s.get("id") == "plan"][0]
open(sys.argv[2], "w").write(step["run"])
PY

# --- stand-ins -----------------------------------------------------------
mkdir -p "$tmp/bin"
cat > "$tmp/bin/gh" <<'EOF'
#!/bin/bash
# `gh api` for one commit's CI state, from $TABLE ("<sha> <state>").
# The canned JSON goes through the step's real jq expression.
sha=""; jqexpr=""
while [ $# -gt 0 ]; do
  case "$1" in
    -f) case "$2" in head_sha=*) sha="${2#head_sha=}" ;; esac; shift 2 ;;
    --jq) jqexpr="$2"; shift 2 ;;
    *) shift ;;
  esac
done
now_iso() { date -u -d "@$(( $(date +%s) - ${1:-0} ))" +%Y-%m-%dT%H:%M:%SZ; }
case "$(awk -v s="$sha" '$1==s{print $2}' "$TABLE")" in
  # Green a moment ago: a newer commit may still take its turn.
  success) json="{\"workflow_runs\":[{\"status\":\"completed\",\"conclusion\":\"success\",\"updated_at\":\"$(now_iso 30)\"}]}" ;;
  # Green half an hour ago and still not published: it has waited.
  waited)  json="{\"workflow_runs\":[{\"status\":\"completed\",\"conclusion\":\"success\",\"updated_at\":\"$(now_iso 1800)\"}]}" ;;
  failed)  json='{"workflow_runs":[{"status":"completed","conclusion":"failure"}]}' ;;
  # Started moments ago: worth waiting for.
  pending) json="{\"workflow_runs\":[{\"status\":\"in_progress\",\"conclusion\":null,\"run_started_at\":\"$(now_iso 30)\"}]}" ;;
  # Started half an hour ago: past any patience.
  stuck)   json="{\"workflow_runs\":[{\"status\":\"in_progress\",\"conclusion\":null,\"run_started_at\":\"$(now_iso 1800)\"}]}" ;;
  *)       json='{"workflow_runs":[]}' ;;
esac
printf '%s' "$json" | jq -r "$jqexpr"
EOF
cat > "$tmp/bin/curl" <<'EOF'
#!/bin/bash
# The feed, with whatever build $FEED says is newest.
printf '{"releases":[{"build":%s}]}' "${FEED:-0}"
EOF
cat > "$tmp/bin/git" <<'EOF'
#!/bin/bash
# A fetch would reach the network. It writes the tip each case chose
# instead, in the form a real fetch leaves behind — the file `FETCH_HEAD`,
# one line, the sha first.
#
# Writing the file rather than `git update-ref FETCH_HEAD`: from git 2.49
# that is refused as a pseudoref, so the harness passed here and failed on
# the runner, with every case deciding "none" for a reason that had
# nothing to do with the decision under test.
if [ "$1" = "fetch" ]; then
  printf '%s\t\tbranch '"'"'main'"'"' of origin\n' "$TIP" \
    > "$(/usr/bin/git rev-parse --git-dir)/FETCH_HEAD"
  exit 0
fi
exec /usr/bin/git "$@"
EOF
chmod +x "$tmp/bin"/*

# --- a repository to decide about ----------------------------------------
repo="$tmp/repo"
mkdir -p "$repo/desktop/bundle"
git -C "$repo" init -q
git -C "$repo" config user.email t@example.com
git -C "$repo" config user.name t
printf 'version = "0.2.0"\n' > "$repo/desktop/Cargo.toml"
printf '<key>LSMinimumSystemVersion</key>\n<string>13.0</string>\n' > "$repo/desktop/bundle/Info.plist"
# Dates are set explicitly and increase. A plain `git rev-list` orders by
# date, so the merge case below depends on the branch commit being *newer*
# than the commit on `main` it has to be distinguished from — and leaving
# that to whichever second the test happened to run in would make it pass
# or fail by luck.
at() { printf '2026-09-17T%02d:00:00Z' "$1"; }
commit_at() {
  GIT_AUTHOR_DATE="$(at "$1")" GIT_COMMITTER_DATE="$(at "$1")" \
    git -C "$repo" commit -qm "$2"
}
declare -a SHA
for i in 1 2 3 4; do
  echo "$i" > "$repo/c$i"
  git -C "$repo" add -A
  commit_at "$i" "commit $i"
  SHA[$i]="$(git -C "$repo" rev-parse HEAD)"
done
# Build numbers are commit counts, so commit N is build N.

# The harness's own footing, checked before anything is decided. If the
# stand-in fetch cannot leave the tip where `git rev-parse FETCH_HEAD`
# finds it, every case below decides "none" and says nothing about the
# decision it was written for — which is how this file passed here and
# failed on a runner with a newer git.
( cd "$repo" && PATH="$tmp/bin:$PATH" TIP="${SHA[4]}" git fetch --quiet origin main )
footing="$(git -C "$repo" rev-parse FETCH_HEAD 2>/dev/null || true)"
if [ "$footing" != "${SHA[4]}" ]; then
  echo "the stand-in fetch does not work on this git ($(git --version))."
  echo "FETCH_HEAD reads '${footing:-nothing}', and should read ${SHA[4]}."
  echo "Nothing below would mean anything, so nothing below ran."
  exit 1
fi

# --- the harness ---------------------------------------------------------
# check <name> <tip> <here> <feed> <expected build|none> <state...>
check() {
  local name="$1" tip="$2" here="$3" feed="$4" expect="$5"; shift 5
  local EVENT_CONCLUSION_OVERRIDE="${EVENT_CONCLUSION_OVERRIDE-}"
  : > "$tmp/table"
  while [ $# -gt 0 ]; do printf '%s %s\n' "$1" "$2" >> "$tmp/table"; shift 2; done
  # The event that started the run is the commit it is about, and its
  # conclusion follows what the table says CI did — except that "pending"
  # is not a conclusion an event can carry. A table entry of `pending` for
  # the triggering commit means its *sibling* run is still going, which is
  # the case this distinction exists for. $EVENT_*_OVERRIDE let a case
  # disagree with the table on purpose.
  local from_table
  from_table="$(awk -v s="$here" '$1==s{print $2}' "$tmp/table")"
  export EVENT_SHA="${EVENT_SHA_OVERRIDE:-$here}"
  # `:-` and not `-`: the override is set-but-empty in the common case,
  # and `-` only substitutes for an unset name.
  case "${EVENT_CONCLUSION_OVERRIDE:-${from_table:-success}}" in
    failed|failure) export EVENT_CONCLUSION=failure ;;
    *) export EVENT_CONCLUSION=success ;;
  esac

  git -C "$repo" checkout -q --detach "$here"
  local out="$tmp/out"; : > "$out"
  local why
  why="$( cd "$repo" && \
    PATH="$tmp/bin:$PATH" TABLE="$tmp/table" FEED="$feed" PATIENCE=600 TIP="$tip" \
    EVENT_SHA="$EVENT_SHA" EVENT_CONCLUSION="$EVENT_CONCLUSION" \
    HAVE_KEY=true SCAN=40 TAG=dev GITHUB_REPOSITORY=o/r \
    GITHUB_OUTPUT="$out" GITHUB_STEP_SUMMARY=/dev/null \
    bash -c "$(sed 's|\${{ github.event_name }}|push|g' "$tmp/plan-body.sh")" 2>&1 \
    | grep -aoE '::(notice|warning) title=[^:]*::.*' | head -1 )"

  local got="none"
  if [ "$(grep -a '^publish=' "$out" | tail -1 | cut -d= -f2)" = "true" ]; then
    got="$(grep -a '^build=' "$out" | tail -1 | cut -d= -f2)"
  fi
  if [ "$got" = "$expect" ]; then
    printf '  ok    %s\n' "$name"
  else
    printf '  FAIL  %s — expected %s, decided %s\n' "$name" "$expect" "$got"
    failures=$((failures + 1))
  fi
  [ -n "$why" ] && printf '          %s\n' "${why#*::}"
  return 0
}

echo "dev channel, what to publish:"

check "the tip is green, so publish it" \
  "${SHA[4]}" "${SHA[4]}" 0 4 \
  "${SHA[4]}" success

check "a newer commit is still running, so wait for it" \
  "${SHA[4]}" "${SHA[3]}" 0 none \
  "${SHA[4]}" pending "${SHA[3]}" success

# The one this file exists for.
check "a newer commit went red, so go back for the green one" \
  "${SHA[4]}" "${SHA[4]}" 0 3 \
  "${SHA[4]}" failed "${SHA[3]}" success

check "two red commits do not bury a green one" \
  "${SHA[4]}" "${SHA[4]}" 0 2 \
  "${SHA[4]}" failed "${SHA[3]}" failed "${SHA[2]}" success

check "what is already on the channel is not published twice" \
  "${SHA[4]}" "${SHA[4]}" 4 none \
  "${SHA[4]}" success

# The tip never got a run of its own — a commit pushed inside a batch, say.
# The run was started by the green commit behind it. "No run" must read as
# "nothing to wait for", not as pending, or the walk stops here for ever.
check "a commit with no CI run of its own is not waited for" \
  "${SHA[4]}" "${SHA[3]}" 0 3 \
  "${SHA[3]}" success

check "nothing green anywhere is a refusal, not a guess" \
  "${SHA[4]}" "${SHA[4]}" 0 none \
  "${SHA[4]}" failed "${SHA[3]}" failed "${SHA[2]}" failed "${SHA[1]}" failed

# --- a merge, and the branch that was merged ------------------------------
# A PR's own commits have green CI runs from when the PR was tested. They
# were never a state `main` was in: the tree is the branch's, and the
# commit count is not a build number on this line. So the walk must follow
# first parents only, and a red merge must fall back to the previous
# commit on `main` — not into the branch it just brought in.
#
# The branch is rooted early and is one commit long, so its head counts 2
# where the commit on `main` counts 4. Without `--first-parent` the walk
# reaches the branch head first, it is green, and the step publishes build
# 2 — a build number belonging to no state `main` was ever in. That is the
# difference this case exists to see; if both counted the same the test
# would pass either way and prove nothing.
git -C "$repo" checkout -q -b side "${SHA[1]}"
echo side > "$repo/side"
git -C "$repo" add -A
commit_at 10 "a commit on the PR branch"   # newer than commit 4
BRANCH_HEAD="$(git -C "$repo" rev-parse HEAD)"
git -C "$repo" checkout -q "${SHA[4]}"
GIT_AUTHOR_DATE="$(at 11)" GIT_COMMITTER_DATE="$(at 11)" \
  git -C "$repo" merge -q --no-ff side -m "Merge the PR"
MERGE="$(git -C "$repo" rev-parse HEAD)"

check "a red merge falls back along main, not into the branch it merged" \
  "$MERGE" "$MERGE" 0 4 \
  "$MERGE" failed "$BRANCH_HEAD" success "${SHA[4]}" success

# --- how long a newer commit may hold the channel ------------------------
# Stepping aside for a commit about to supersede this one is thrift.
# Stepping aside for one that is nowhere near done starves the channel: on
# 2026-09-18 that left the feed eighty minutes and forty-two builds behind
# `main`, with every single deferral correct.
check "a newer commit that just started is still worth waiting for" \
  "${SHA[4]}" "${SHA[3]}" 0 none \
  "${SHA[4]}" pending "${SHA[3]}" success

check "a newer commit that has run past the patience does not hold it" \
  "${SHA[4]}" "${SHA[3]}" 0 3 \
  "${SHA[4]}" stuck "${SHA[3]}" success

check "one stuck commit does not hide a fresher one behind it" \
  "${SHA[4]}" "${SHA[2]}" 0 none \
  "${SHA[4]}" stuck "${SHA[3]}" pending "${SHA[2]}" success

# --- how long a green build may wait -------------------------------------
# The cap above is read off the pending commit, and while `main` merges
# faster than CI finishes there is always a young one: every deferral is
# to a commit that genuinely started moments ago, the cap never fires, and
# the chain runs for ever. That is what the channel did all morning on
# 2026-09-18 with the pending-side cap already in place.
#
# So the clock that ends a deferral is on the green build instead. No
# merge can reset it, because it asks how long something publishable has
# been sitting here rather than how long the newest thing has been under
# test.
check "a green that has waited goes out, whatever is still running" \
  "${SHA[4]}" "${SHA[3]}" 0 3 \
  "${SHA[4]}" pending "${SHA[3]}" waited

# The shape the channel was actually in on the morning of 2026-09-18, and
# the reason the pending-side cap could not end it: not one newer commit
# under test but a queue of them, each genuinely young, the newest of them
# younger still by the time the previous one finished. The green below
# them is the one that has been kept waiting, and it is the one timed.
check "a queue of young commits does not outlast a green that has waited" \
  "${SHA[4]}" "${SHA[2]}" 0 2 \
  "${SHA[4]}" pending "${SHA[3]}" pending "${SHA[2]}" waited

# The one that publishes is still the newest green. The waiting one is
# read for the clock, not for the build number.
check "the green that has waited starts the clock; the newest green goes" \
  "${SHA[4]}" "${SHA[3]}" 1 3 \
  "${SHA[4]}" pending "${SHA[3]}" success "${SHA[2]}" waited

# A green that is already on the channel has not been kept waiting — it
# went out. Reading it as a build held back would publish on every event
# for ever after.
check "a green the channel already has does not start the clock" \
  "${SHA[4]}" "${SHA[3]}" 3 none \
  "${SHA[4]}" pending "${SHA[3]}" waited

check "nothing newer than the channel is green, so nothing publishes" \
  "${SHA[4]}" "${SHA[4]}" 3 none \
  "${SHA[4]}" failed "${SHA[3]}" waited

# Reading the clock means walking past the newest green, which brings
# older commits into view that the walk used to stop before. One of those
# still running is not a reason to hold anything: the green above it
# already supersedes it.
check "a commit still running behind the newest green holds nothing" \
  "${SHA[4]}" "${SHA[4]}" 0 4 \
  "${SHA[4]}" success "${SHA[3]}" pending "${SHA[2]}" success

# --- the commit this run was started by -----------------------------------
# A commit gets two CI runs. The first to finish fires the event; the API
# a moment later still shows its sibling going, and says "pending". Asking
# the API about the triggering commit therefore makes the run wait on
# itself — for an event that has already happened. Four runs did exactly
# that on 2026-09-17 and the channel sat for forty-two minutes.
check "the run does not wait on the commit that started it" \
  "${SHA[4]}" "${SHA[4]}" 0 4 \
  "${SHA[4]}" pending

# And a green sibling still counts when the triggering run was the red one.
EVENT_CONCLUSION_OVERRIDE=failure check \
  "a failed trigger still sees a sibling run that went green" \
  "${SHA[4]}" "${SHA[4]}" 0 4 \
  "${SHA[4]}" success

# A genuinely red commit is still red, and the walk still goes back.
check "a failed trigger with nothing green on it falls back" \
  "${SHA[4]}" "${SHA[4]}" 0 3 \
  "${SHA[4]}" failed "${SHA[3]}" success

echo
if [ "$failures" -eq 0 ]; then
  echo "all good"
else
  echo "$failures failed"
fi
exit "$failures"

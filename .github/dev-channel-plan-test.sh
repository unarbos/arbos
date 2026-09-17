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
case "$(awk -v s="$sha" '$1==s{print $2}' "$TABLE")" in
  success) json='{"workflow_runs":[{"status":"completed","conclusion":"success"}]}' ;;
  failed)  json='{"workflow_runs":[{"status":"completed","conclusion":"failure"}]}' ;;
  pending) json='{"workflow_runs":[{"status":"in_progress","conclusion":null}]}' ;;
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
# A fetch would reach the network and would move FETCH_HEAD, which each
# case sets for itself.
[ "$1" = "fetch" ] && exit 0
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
declare -a SHA
for i in 1 2 3 4; do
  echo "$i" > "$repo/c$i"
  git -C "$repo" add -A
  git -C "$repo" commit -qm "commit $i"
  SHA[$i]="$(git -C "$repo" rev-parse HEAD)"
done
# Build numbers are commit counts, so commit N is build N.

# --- the harness ---------------------------------------------------------
# check <name> <tip> <here> <feed> <expected build|none> <state...>
check() {
  local name="$1" tip="$2" here="$3" feed="$4" expect="$5"; shift 5
  : > "$tmp/table"
  while [ $# -gt 0 ]; do printf '%s %s\n' "$1" "$2" >> "$tmp/table"; shift 2; done

  git -C "$repo" checkout -q --detach "$here"
  git -C "$repo" update-ref FETCH_HEAD "$tip"
  local out="$tmp/out"; : > "$out"
  local why
  why="$( cd "$repo" && \
    PATH="$tmp/bin:$PATH" TABLE="$tmp/table" FEED="$feed" \
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

check "a commit with no CI run of its own is not waited for" \
  "${SHA[4]}" "${SHA[4]}" 0 3 \
  "${SHA[3]}" success

check "nothing green anywhere is a refusal, not a guess" \
  "${SHA[4]}" "${SHA[4]}" 0 none \
  "${SHA[4]}" failed "${SHA[3]}" failed "${SHA[2]}" failed "${SHA[1]}" failed

echo
if [ "$failures" -eq 0 ]; then
  echo "all good"
else
  echo "$failures failed"
fi
exit "$failures"

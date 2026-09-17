#!/usr/bin/env bash
# A stand-in `gh` for the UI pass: answers `gh pr create` with a pull request
# URL the way the real one does (the URL is the last line of stdout), keeps
# a counter so every PR gets the next number, and knows the handful of other
# verbs an agent tends to try first. Put its folder first on PATH.
#
#   FAKE_GH_REPO   owner/repo in the URLs (default unarbos/parity-proj)
#   FAKE_GH_STATE  the counter file (default $TMPDIR/fake-gh-counter)
set -euo pipefail
repo="${FAKE_GH_REPO:-unarbos/parity-proj}"
state="${FAKE_GH_STATE:-${TMPDIR:-/tmp}/fake-gh-counter}"

next() {
  local n=0
  [ -f "$state" ] && n=$(cat "$state")
  n=$((n + 1))
  echo "$n" > "$state"
  echo "$n"
}

case "${1:-} ${2:-}" in
  "pr create")
    n=$(next)
    title=""
    while [ $# -gt 0 ]; do
      case "$1" in
        --title|-t) title="${2:-}"; shift ;;
        --title=*) title="${1#--title=}" ;;
      esac
      shift
    done
    echo "Creating pull request for ${title:-untitled} in $repo" >&2
    echo "https://github.com/$repo/pull/$n"
    ;;
  "pr view")
    n=1; [ -f "$state" ] && n=$(cat "$state")
    if printf '%s\n' "$@" | grep -q -- '--json'; then
      printf '{"number":%s,"url":"https://github.com/%s/pull/%s","state":"OPEN","title":"Parity PR"}\n' "$n" "$repo" "$n"
    else
      echo "Parity PR #$n"; echo "Open • https://github.com/$repo/pull/$n"
    fi
    ;;
  "pr list")
    n=0; [ -f "$state" ] && n=$(cat "$state")
    if printf '%s\n' "$@" | grep -q -- '--json'; then
      echo "[]"
    else
      [ "$n" -gt 0 ] && echo "#$n  Parity PR  cursor/parity-pill  OPEN" || true
    fi
    ;;
  "auth status")
    echo "github.com" ; echo "  ✓ Logged in to github.com account parity-bot (fake gh)"
    ;;
  "repo view")
    echo "$repo"
    ;;
  "--version "*|"version ")
    echo "gh version 2.0.0 (fake, parity suite)"
    ;;
  *)
    echo "fake gh: unsupported: gh $*" >&2
    exit 1
    ;;
esac

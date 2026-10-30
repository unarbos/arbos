#!/usr/bin/env bash
# Upgrade the installed feedback poller on the relay host FROM GIT, never from a working tree.
#
#   sudo scripts/feedback/install-poller.sh            # feedback_poller.py + agent-prompt.md at origin/main
#   sudo scripts/feedback/install-poller.sh --ref <sha> # a pushed commit (a gate run's head, a hotfix)
#
# Why: twice on 2026-10-23 the poller was upgraded by copying a file out of an operator's checkout —
# once it carried code that was never committed (the ETag cache, lost again at the next install),
# once it was behind the committed head (the batcher fairness change was feared lost at 23:10).
# This script downloads the files at a commit that exists on GitHub, compiles them, installs them
# and records the commit in /opt/arbos-feedback/INSTALLED_SHA. The GitHub token is read from
# /etc/arbos/feedback.env (the loop's own). Run it on the relay host; the tick timer picks the new
# file up at its next tick (each tick is a fresh process).
set -euo pipefail

INSTALL_DIR=/opt/arbos-feedback
ENV_FILE=/etc/arbos/feedback.env
REPO=${GITHUB_REPO:-unarbos/arbos}
REF=main
while [[ $# -gt 0 ]]; do
  case "$1" in
    --ref) REF="$2"; shift 2;;
    *) echo "unknown arg $1" >&2; exit 2;;
  esac
done
[[ $EUID -eq 0 ]] || { echo "run as root (sudo)" >&2; exit 1; }

token=$(sed -n 's/^GITHUB_TOKEN=//p' "$ENV_FILE" | tr -d '"'"'" | tr -d '\r\n')
[[ -n "$token" ]] || { echo "no GITHUB_TOKEN in $ENV_FILE" >&2; exit 1; }
api() { curl -fsS -H "Authorization: Bearer $token" -H "Accept: application/vnd.github+json" "$@"; }

sha=$(api "https://api.github.com/repos/$REPO/commits/$REF" | python3 -c 'import json,sys; print(json.load(sys.stdin)["sha"])')
[[ -n "$sha" ]] || { echo "cannot resolve $REF on $REPO" >&2; exit 1; }
# A ref that is not on origin/main must at least be pushed (it resolved) — say so in the record.
on_main=$(api "https://api.github.com/repos/$REPO/compare/main...$sha" | python3 -c 'import json,sys; print("yes" if json.load(sys.stdin).get("status") in ("identical","behind") else "no")')

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for f in feedback_poller.py agent-prompt.md; do
  api -H "Accept: application/vnd.github.raw" "https://api.github.com/repos/$REPO/contents/scripts/feedback/$f?ref=$sha" -o "$tmp/$f"
done
python3 -m py_compile "$tmp/feedback_poller.py"
install -d -m 0755 "$INSTALL_DIR"
install -m 0755 "$tmp/feedback_poller.py" "$INSTALL_DIR/feedback_poller.py"
install -m 0644 "$tmp/agent-prompt.md" "$INSTALL_DIR/agent-prompt.md"
printf '%s %s on_main=%s installed=%s\n' "$sha" "$REF" "$on_main" "$(date -u +%FT%TZ)" > "$INSTALL_DIR/INSTALLED_SHA"
echo "installed feedback_poller.py @ ${sha:0:9} ($REF, on main: $on_main); md5 $(md5sum "$INSTALL_DIR/feedback_poller.py" | cut -c1-12)"

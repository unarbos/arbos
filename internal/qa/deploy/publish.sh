#!/usr/bin/env bash
# Move QA results between ArbosLife and the repo's `qa-results` branch.
#
#   publish.sh push   copy bugs/, kickoff-history.jsonl, spend.jsonl,
#                     rollouts/index.jsonl and each broken rollout's
#                     result.json + driver.log into the branch and push
#   publish.sh pull   fetch the branch and copy inbox/*.md into loop/inbox
#
# The branch is an orphan: results only, no source. Cloud agents sync it
# with the Project store (internal/qa/sync.sh). The PAT comes from
# secrets.env as ARBOS_GITHUB and is passed through a credential helper,
# never written into .git/config.
set -euo pipefail
ROOT="${ARBOS_QA_ROOT:-$HOME/arbos-qa}"
RESULTS="$ROOT/results"
BRANCH=qa-results
REMOTE=https://github.com/unarbos/arbos.git
: "${ARBOS_GITHUB:?ARBOS_GITHUB not set (source secrets.env)}"

git_auth() {
  git -c credential.helper= \
      -c "credential.helper=!f() { echo username=x-access-token; echo password=\$ARBOS_GITHUB; }; f" "$@"
}

ensure_clone() {
  if [ ! -d "$RESULTS/.git" ]; then
    rm -rf "$RESULTS"
    if git ls-remote --exit-code --heads "$REMOTE" "$BRANCH" >/dev/null 2>&1; then
      git clone -q --branch "$BRANCH" --single-branch "$REMOTE" "$RESULTS"
    else
      git init -q "$RESULTS"
      git -C "$RESULTS" checkout -q --orphan "$BRANCH"
      git -C "$RESULTS" remote add origin "$REMOTE"
      printf '# Arbos QA results\n\nWritten by the QA loop on ArbosLife. Bugs, kickoff history, spend, run index.\n' > "$RESULTS/README.md"
    fi
    git -C "$RESULTS" config user.name "arbos-qa"
    git -C "$RESULTS" config user.email "qa@arbos.local"
  fi
}

case "${1:-}" in
  pull)
    ensure_clone
    git_auth -C "$RESULTS" pull -q --rebase origin "$BRANCH" 2>/dev/null || true
    mkdir -p "$ROOT/loop/inbox"
    if [ -d "$RESULTS/inbox" ]; then
      cp -n "$RESULTS"/inbox/*.md "$ROOT/loop/inbox/" 2>/dev/null || true
    fi
    ;;
  push)
    ensure_clone
    git_auth -C "$RESULTS" pull -q --rebase origin "$BRANCH" 2>/dev/null || true
    mkdir -p "$RESULTS/bugs" "$RESULTS/rollouts"
    # Mirror: a draft removed from loop/bugs (folded into a curated file) must
    # not come back from the branch on the next pull.
    # (plain cp, not rsync: the VM has no rsync and publish had failed six cycles running on 2026-09-16)
    rm -rf "$RESULTS/bugs" && mkdir -p "$RESULTS/bugs" && cp -r "$ROOT/loop/bugs/." "$RESULTS/bugs/"
    for f in kickoff-history.jsonl spend.jsonl call-mode-history.jsonl pod-health.jsonl store-mirror-history.jsonl store-mirror-losses.jsonl journey-history.jsonl; do
      [ -f "$ROOT/loop/$f" ] && cp "$ROOT/loop/$f" "$RESULTS/$f"
    done
    [ -f "$ROOT/loop/rollouts/index.jsonl" ] && cp "$ROOT/loop/rollouts/index.jsonl" "$RESULTS/rollouts/index.jsonl"
    # Broken rollouts: the small files only. State snapshots stay on the box.
    python3 - "$ROOT/loop/rollouts" "$RESULTS/rollouts" <<'EOF'
import json, os, shutil, sys
src, dst = sys.argv[1], sys.argv[2]
for name in sorted(os.listdir(src)):
    d = os.path.join(src, name)
    if not os.path.isdir(d):
        continue
    try:
        status = json.load(open(os.path.join(d, "result.json"))).get("status")
    except Exception:
        continue
    if status != "break":
        continue
    out = os.path.join(dst, name)
    os.makedirs(out, exist_ok=True)
    for f in ("result.json", "driver.log", "scenario.json", "kickoff-checklist.json", "kernel.stderr.log"):
        p = os.path.join(d, f)
        if os.path.exists(p) and os.path.getsize(p) < 512 * 1024:
            shutil.copyfile(p, os.path.join(out, f))
EOF
    printf 'host: %s\nlast_cycle: %s\nbuilt_sha: %s\n' "$(hostname)" "$(date -u +%FT%TZ)" "$(cat "$ROOT/state/built-sha" 2>/dev/null || echo unknown)" > "$RESULTS/status.txt"
    git -C "$RESULTS" add -A
    if git -C "$RESULTS" diff --cached --quiet; then
      echo "-- publish: nothing new"
    else
      git -C "$RESULTS" commit -q -m "qa: results $(date -u +%FT%TZ) from $(hostname)"
      git_auth -C "$RESULTS" push -q -u origin "$BRANCH"
      echo "-- publish: pushed"
    fi
    ;;
  *)
    echo "usage: publish.sh push|pull" >&2; exit 2;;
esac

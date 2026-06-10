#!/usr/bin/env bash
# Ralph loop: each iteration is a fresh, stateless arbos run.
# State lives in the repo (ralph/STACK.md) — not in any context window.
#
# Usage:  ralph/loop.sh [max_iterations]
# Env:    RALPH_PUSH=1 to git push after each green commit (default: local only)

set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

MAX="${1:-0}"          # 0 = run until STACK is empty
i=0

while :; do
  i=$((i + 1))
  if [ "$MAX" -ne 0 ] && [ "$i" -gt "$MAX" ]; then
    echo "ralph: reached max iterations ($MAX), stopping."
    break
  fi

  echo "===== ralph iteration $i ====="
  rm -f ralph/STATUS

  # Fresh, stateless agent. One turn, one item.
  go run ./cmd/arbos -once -p "$(cat ralph/PROMPT.md)" || {
    echo "ralph: agent run failed on iteration $i — stopping for human review."
    exit 1
  }

  # Circuit breaker: agent signals exhaustion of the worklist.
  if [ -f ralph/STATUS ] && grep -q 'ALL DONE' ralph/STATUS; then
    echo "ralph: worklist exhausted. Done after $i iterations."
    break
  fi

  # Verify the tree is green before recording the iteration.
  if ! (go vet ./... && go build ./... && go test ./...); then
    echo "ralph: tree is RED after iteration $i — stopping for human review."
    exit 1
  fi

  # Record the iteration. Codebase is the memory.
  git add -A
  if ! git diff --cached --quiet; then
    git commit -m "ralph: iteration $i" -q
    [ "${RALPH_PUSH:-0}" = "1" ] && git push -q
    echo "ralph: committed iteration $i."
  else
    echo "ralph: no changes this iteration — stopping (likely stuck)."
    break
  fi
done

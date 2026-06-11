#!/usr/bin/env bash
set -euo pipefail

cd /Users/const/arbos-3

# 1. Build the local code; abort if it does not compile.
if ! go build ./...; then
  echo "build failed: not compiling, skipping commit"
  exit 1
fi
echo "build ok"

# 2. Only commit/push if there are local edits.
if git diff --quiet && git diff --cached --quiet && [ -z "$(git ls-files --others --exclude-standard)" ]; then
  echo "no local changes to commit"
  exit 0
fi

# 3. Commit and push to main.
git add -A
git commit -m "chore: automated build commit ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
git push origin main
echo "committed and pushed to main"

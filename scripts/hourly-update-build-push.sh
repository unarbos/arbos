#!/usr/bin/env bash
set -euo pipefail

cd /Users/const/arbos-3

# 1. Check for / pull remote updates on main before building.
git fetch origin main
LOCAL=$(git rev-parse @)
REMOTE=$(git rev-parse origin/main)
if [ "$LOCAL" != "$REMOTE" ]; then
  echo "remote updates found; pulling --ff-only"
  git pull --ff-only origin main
else
  echo "already up to date with origin/main"
fi

# 2. Build the local code; abort if it does not compile.
if ! go build ./...; then
  echo "build failed: not compiling, skipping commit"
  exit 1
fi
echo "build ok"

# 3. Only commit/push if there are local edits.
if git diff --quiet && git diff --cached --quiet && [ -z "$(git ls-files --others --exclude-standard)" ]; then
  echo "no local changes to commit"
  exit 0
fi

# 4. Commit and push to main.
git add -A
git commit -m "chore: automated build commit ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
git push origin main
echo "committed and pushed to main"

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

# 4. Stage edits to tracked files, plus new files in source dirs only.
# Deliberately NOT `git add -A`: stray root artifacts (binaries, images,
# scratch html) must never be auto-pushed to main.
git add -u
git add -- cmd internal web docs scripts go.mod go.sum .gitignore Makefile README.md USAGE.md

if git diff --cached --quiet; then
  echo "only stray/unlisted files changed; nothing staged, skipping commit"
  exit 0
fi

FALLBACK="chore: automated build commit ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
MSG=""
if command -v claude >/dev/null 2>&1; then
  DIFF=$(git diff --cached --stat; echo; git diff --cached | head -c 12000)
  # perl alarm = portable 60s timeout (macOS has no coreutils `timeout`);
  # a hung claude call must not block the hourly job forever.
  MSG=$(printf '%s\n' "$DIFF" | perl -e 'alarm 60; exec @ARGV' claude -p "Write a concise git commit message for the following staged changes. Use a conventional-commit style subject line (max 72 chars), then a blank line, then 1-4 bullet points summarizing what changed. Output ONLY the commit message, no preamble or code fences." 2>/dev/null || true)
fi

if [ -z "${MSG// }" ]; then
  MSG="$FALLBACK"
  echo "ai commit message unavailable; using fallback"
fi

git commit -m "$MSG"
git push origin main
echo "committed and pushed to main"

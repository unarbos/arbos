#!/usr/bin/env bash
set -euo pipefail

cd /home/const/arboshermes/arbos

go build ./...
go test ./...

if git diff --quiet && git diff --cached --quiet; then
  echo "no changes to commit"
  exit 0
fi

git add -A
git commit -m "chore: automated build/test commit ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
git push origin main

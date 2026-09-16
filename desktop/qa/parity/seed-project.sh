#!/usr/bin/env bash
# Create the tiny sample project both apps open. Safe to run again: it
# resets the files to a known state so every suite run starts the same.
set -euo pipefail
source "$(dirname "$0")/env.sh"

mkdir -p "$PROJ"
cd "$PROJ"
printf '# parity-proj\n\nA tiny project used for Cursor/Arbos parity captures.\n' > README.md
printf 'def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n' > math_utils.py
printf 'from math_utils import add, sub\n\nprint(add(2, 3))\nprint(sub(5, 1))\n' > main.py
rm -f CHANGELOG.md
[ -d .git ] || git init -q .
git add -A
git -c user.email=parity@local -c user.name=parity commit -qm "reset" 2>/dev/null || true
log "seeded $PROJ"

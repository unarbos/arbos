#!/usr/bin/env bash
# Does a place whose `.arbos` is a symlink work, and does the lock still hold?
#
# Moving the store to another disk and leaving a symlink behind is an ordinary thing to do with a
# folder that grows. Nothing in the library stages it. Two questions:
#   1. does a kernel serve such a place at all, and write its records through the link;
#   2. does the place lock still refuse a second kernel — the lock is flock on files under
#      `.arbos`, and qal-j40 showed that lock identity is about inodes, not paths.
set -uo pipefail
K="${1:?kernel binary}"
NS="$HOME/arbos-qa/deploy/ns-wrap.sh"
ROOT=$(mktemp -d /tmp/symstore.XXXXXX)
P="$ROOT/place"; ELSEWHERE="$ROOT/elsewhere"
mkdir -p "$P" "$ELSEWHERE"
ln -s "$ELSEWHERE" "$P/.arbos"

echo "## symlinked .arbos on $("$K" --version 2>&1 | head -1)"
echo "   place:  $P"
echo "   .arbos -> $(readlink "$P/.arbos")"

timeout 60 bash "$NS" "$K" serve "$P" > "$ROOT/a.log" 2>&1 &
for _ in $(seq 40); do [ -s "$ELSEWHERE/runtime/kernel.json" ] && break; sleep 0.5; done

if [ -s "$ELSEWHERE/runtime/kernel.json" ]; then
    PIDA=$(python3 -c "import json;print(json.load(open('$ELSEWHERE/runtime/kernel.json'))['pid'])")
    echo "   A served, pid $PIDA; records landed through the link"
else
    echo "   A did not serve. What it said:"
    head -3 "$ROOT/a.log" | sed 's/^/     /' | cut -c1-150
    pkill -P $$ 2>/dev/null; rm -rf "$ROOT"; exit 0
fi

# Real files, or did something follow the link and write beside it?
echo "   files under the real folder: $(find "$ELSEWHERE" -maxdepth 1 -mindepth 1 | wc -l)"
echo "   .arbos is still a symlink: $([ -L "$P/.arbos" ] && echo yes || echo NO — it was replaced)"

# The lock, asked through the same link.
timeout 25 bash "$NS" "$K" serve "$P" > "$ROOT/b.log" 2>&1 &
sleep 10
PIDB=$(python3 -c "import json;print(json.load(open('$ELSEWHERE/runtime/kernel.json'))['pid'])" 2>/dev/null || echo "")
held=$(grep -icE 'already served|place is held|held by' "$ROOT/b.log" || true)
echo "   B refused out loud: $([ "$held" -gt 0 ] && echo yes || echo NO)"
echo "   kernel.json still names A: $([ "$PIDB" = "$PIDA" ] && echo yes || echo "no — now $PIDB")"
live=0
for p in $PIDA $PIDB; do [ -n "$p" ] && kill -0 "$p" 2>/dev/null && live=$((live+1)); done
echo "   distinct live kernels: $live"
if [ -n "$PIDB" ] && [ "$PIDB" != "$PIDA" ] && kill -0 "$PIDA" 2>/dev/null; then
    echo "   VERDICT: the symlinked place took a second kernel while the first still holds it"
else
    echo "   VERDICT: one kernel, as it should be"
fi
pkill -P $$ 2>/dev/null; sleep 1; rm -rf "$ROOT"

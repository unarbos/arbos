#!/usr/bin/env bash
# af-05c: both lock files removed under a serving kernel — did the second kernel actually serve?
# Exit 124 from `timeout` only says B was still running; it does not say B took the place. What says
# that is a second live pid and a kernel.json naming it while the first is still alive.
set -uo pipefail
K="${1:?kernel binary}"
ROOT=$(mktemp -d /tmp/af05c.XXXXXX); P="$ROOT/place"; mkdir -p "$P"
echo "## af-05c on $("$K" --version 2>&1 | head -1)"

timeout 120 bash "$HOME/arbos-qa/deploy/ns-wrap.sh" "$K" serve "$P" > "$ROOT/a.log" 2>&1 &
for _ in $(seq 40); do [ -s "$P/.arbos/runtime/kernel.json" ] && break; sleep 0.5; done
PIDA=$(python3 -c "import json;print(json.load(open('$P/.arbos/runtime/kernel.json'))['pid'])")
echo "   A serving, pid $PIDA"

rm -rf "$P/.arbos/runtime" "$P/.arbos/lock"
echo "   removed .arbos/runtime and .arbos/lock; A alive: $(kill -0 $PIDA 2>/dev/null && echo yes || echo no)"
sleep 2

timeout 40 bash "$HOME/arbos-qa/deploy/ns-wrap.sh" "$K" serve "$P" > "$ROOT/b.log" 2>&1 &
sleep 12
PIDB=""
[ -s "$P/.arbos/runtime/kernel.json" ] && PIDB=$(python3 -c "import json;print(json.load(open('$P/.arbos/runtime/kernel.json'))['pid'])" 2>/dev/null || true)
echo "   kernel.json now names pid: ${PIDB:-none}"
echo "   A alive: $(kill -0 $PIDA 2>/dev/null && echo yes || echo no)   B's pid alive: $([ -n "$PIDB" ] && kill -0 "$PIDB" 2>/dev/null && echo yes || echo no)"
echo "   distinct live kernels on this place: $(for p in $PIDA $PIDB; do kill -0 "$p" 2>/dev/null && echo "$p"; done | sort -u | wc -l)"
grep -iE 'already served|held' "$ROOT/b.log" | head -1 | sed 's/^/     B said: /' | cut -c1-140 || echo "     B said nothing about the place being held"
if [ -n "$PIDB" ] && [ "$PIDB" != "$PIDA" ] && kill -0 "$PIDA" 2>/dev/null && kill -0 "$PIDB" 2>/dev/null; then
  echo "   VERDICT: TWO KERNELS SERVE ONE PLACE — A=$PIDA and B=$PIDB both alive, kernel.json names B"
else
  echo "   VERDICT: only one kernel ended up serving"
fi
# Does #450's detector see the aftermath? That is the mitigation, and it decides how bad this is.
echo "   --- letting both run a little, then asking check ---"
sleep 8
kill "$PIDA" "${PIDB:-}" 2>/dev/null; sleep 2
"$K" check "$P" 2>&1 | grep -iE 'two kernels|still open|two checkpoints' | head -3 | sed 's/^/     check: /' | cut -c1-165
n=$("$K" check "$P" 2>&1 | grep -ciE 'two kernels|still open|two checkpoints')
echo "   double-serving warnings from check: $n"
pkill -P $$ 2>/dev/null; sleep 1; rm -rf "$ROOT"

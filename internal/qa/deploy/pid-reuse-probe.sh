#!/usr/bin/env bash
# #446 settled that "the record naming a live process wins". What if the live process is not a
# kernel?
#
# Pid reuse is ordinary: a machine crashes with a kernel.json on disk naming pid 4242, reboots, and
# something else gets 4242. The record still names a live process, so the liveness test passes and
# the record wins — but there is no kernel behind it. If the arriving kernel then refuses, the
# person is locked out of their own place by a number.
#
# Staged exactly: a kernel.json naming a pid that is alive and is demonstrably not a kernel (a
# `sleep`), then a kernel asked to serve that place.
set -uo pipefail
K="${1:?kernel binary}"
NS="$HOME/arbos-qa/deploy/ns-wrap.sh"
ROOT=$(mktemp -d /tmp/pidreuse.XXXXXX); P="$ROOT/place"; mkdir -p "$P"

echo "## pid reuse on $("$K" --version 2>&1 | head -1)"

# Let a kernel make the place properly, then stop it, so every folder is real.
timeout 60 bash "$NS" "$K" serve "$P" > "$ROOT/boot.log" 2>&1 &
for _ in $(seq 40); do [ -s "$P/.arbos/runtime/kernel.json" ] && break; sleep 0.5; done
BOOT=$(python3 -c "import json;print(json.load(open('$P/.arbos/runtime/kernel.json'))['pid'])" 2>/dev/null || echo "")
[ -n "$BOOT" ] && kill -INT "$BOOT" 2>/dev/null
sleep 2

# An impostor: alive, and plainly not a kernel.
sleep 600 &
IMPOSTOR=$!
echo "   impostor pid $IMPOSTOR is a \`sleep\`, alive: $(kill -0 $IMPOSTOR 2>/dev/null && echo yes || echo no)"

for rec in "$P/.arbos/runtime/kernel.json" "$P/.arbos/kernel.json"; do
    [ -e "$rec" ] || continue
    python3 - "$rec" "$IMPOSTOR" <<'PY'
import json, sys
p, pid = sys.argv[1], int(sys.argv[2])
d = json.load(open(p))
d["pid"] = pid
json.dump(d, open(p, "w"))
PY
    echo "   rewrote $(basename "$(dirname "$rec")")/$(basename "$rec") to name pid $IMPOSTOR"
done

timeout 30 bash "$NS" "$K" serve "$P" > "$ROOT/b.log" 2>&1 &
sleep 12
NEW=$(python3 -c "import json;print(json.load(open('$P/.arbos/runtime/kernel.json'))['pid'])" 2>/dev/null || echo "")
refused=$(grep -icE 'already served|place is held|held by' "$ROOT/b.log" || true)
echo "   the arriving kernel refused: $([ "$refused" -gt 0 ] && echo yes || echo no)"
echo "   kernel.json now names: ${NEW:-none} (impostor was $IMPOSTOR)"
if [ "$refused" -gt 0 ] && [ "$NEW" = "$IMPOSTOR" ]; then
    echo "   VERDICT: LOCKED OUT — a \`sleep\` holds the place; the record's pid is alive so it wins"
    grep -iE 'already served|held' "$ROOT/b.log" | head -1 | sed 's/^/     it said: /' | cut -c1-150
elif [ -n "$NEW" ] && [ "$NEW" != "$IMPOSTOR" ]; then
    echo "   VERDICT: served anyway — the kernel saw through the impostor"
else
    echo "   VERDICT: neither; what it said:"; head -3 "$ROOT/b.log" | sed 's/^/     /' | cut -c1-150
fi
kill "$IMPOSTOR" 2>/dev/null; pkill -P $$ 2>/dev/null; sleep 1; rm -rf "$ROOT"

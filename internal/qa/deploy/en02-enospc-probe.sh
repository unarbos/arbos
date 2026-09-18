#!/usr/bin/env bash
# en-02: a genuinely full filesystem — the other cause `drop_partial_line` names.
#
# `en-01` reached the failed-write arm by RLIMIT_FSIZE, the "size limit" half of
# "disk full, size limit". This is the other half: a 512 KiB tmpfs, the place on it, the transcript
# padded to near the ceiling, then a turn. The append meets ENOSPC on a real file with a real
# length, which is what `set_len` needs to cut the headless remainder back off.
#
# Everything happens inside one user+mount namespace, because a tmpfs mounted in the kernel's own
# namespace (ns-wrap's) is invisible to whoever wants to pad the transcript or read the result
# afterwards. The cost is that the whole chain runs as root-in-namespace: fine for a probe about
# space, wrong for anything about permissions, which is why the uw-* family must not borrow this.
set -uo pipefail
K="${1:?kernel binary}"
SIZE="${2:-512k}"
OUT=/tmp/handover/en02.out
: > "$OUT"

unshare -Urm --map-root-user bash -s -- "$K" "$SIZE" "$OUT" <<'INNER'
set -uo pipefail
K="$1"; SIZE="$2"; OUT="$3"
ROOT=$(mktemp -d /tmp/en02.XXXXXX)
P="$ROOT/place"
mkdir -p "$P"
mount -t tmpfs -o "size=$SIZE,mode=1777" tmpfs "$P" || { echo "mount failed" >> "$OUT"; exit 1; }
printf 'filesystem: %s\n' "$(df -h "$P" | tail -1 | awk '{print $2}')" >> "$OUT"

# Bootstrap the kernel's own files on the tmpfs, then stop it.
timeout 25 bash "$HOME/arbos-qa/deploy/ns-wrap.sh" "$K" serve "$P" > "$ROOT/boot.log" 2>&1 &
BOOT=$!
for _ in $(seq 40); do [ -s "$P/.arbos/agents/root/agent.md" ] && break; sleep 0.5; done
kill $BOOT 2>/dev/null; wait $BOOT 2>/dev/null
T="$P/.arbos/agents/root/transcript.jsonl"
[ -f "$T" ] || { echo "no transcript after bootstrap" >> "$OUT"; exit 1; }

# Pad with whole events until the filesystem is nearly full, leaving less room than one append needs.
python3 - "$T" "$P" >> "$OUT" <<'PY'
import json, os, sys, time
t, place = sys.argv[1], sys.argv[2]
pad = json.dumps({"ts": int(time.time() * 1000), "kind": "assistant", "text": "x" * 400}) + "\n"
with open(t, "a") as f:
    while True:
        st = os.statvfs(place)
        free = st.f_bavail * st.f_frsize
        if free < 4096:
            break
        try:
            f.write(pad); f.flush()
        except OSError as e:
            print(f"padding stopped on {type(e).__name__}: {e}")
            break
# Leave a sliver: smaller than one event, larger than nothing. Filling to exactly 0 makes the write
# fail at byte 0, and then there is no headless line for `drop_partial_line` to cut — the easy case.
# A partial write needs room for some of the buffer and not all of it.
st = os.statvfs(place)
free = st.f_bavail * st.f_frsize
want_left = 200
if free > want_left:
    with open(os.path.join(place, ".filler"), "wb") as g:
        try:
            g.write(b"\0" * (free - want_left)); g.flush(); os.fsync(g.fileno())
        except OSError as e:
            print(f"filler stopped on {type(e).__name__}")
st = os.statvfs(place)
print(f"free before the turn: {st.f_bavail * st.f_frsize} bytes")
good = bad = 0
for line in open(t, errors="replace").read().split("\n"):
    if not line.strip():
        continue
    try:
        json.loads(line); good += 1
    except ValueError:
        bad += 1
print(f"before: readable {good} unparseable {bad}")
PY

# One turn, with the replay provider so no model is needed.
echo '{"agent":"root","content":"FULL"}' > "$ROOT/replies.jsonl"
timeout 45 bash "$HOME/arbos-qa/deploy/ns-wrap.sh" "$K" serve "$P" --provider replay --replies "$ROOT/replies.jsonl" > "$ROOT/serve.log" 2>&1 &
S=$!
sleep 6
timeout 30 "$K" run --place "$P" --timeout 20 "ENOSPC-MARKER: reply with the single word FULL." >> "$ROOT/run.log" 2>&1
sleep 2
ALIVE=no; kill -0 $S 2>/dev/null && ALIVE=yes
kill $S 2>/dev/null; wait $S 2>/dev/null

python3 - "$T" "$P" "$ROOT/serve.log" >> "$OUT" <<'PY'
import json, os, sys
t, place, log = sys.argv[1], sys.argv[2], sys.argv[3]
good = []; bad = []
for line in open(t, errors="replace").read().split("\n"):
    if not line.strip():
        continue
    try:
        good.append(json.loads(line))
    except ValueError:
        bad.append(line)
print(f"after:  readable {len(good)} unparseable {len(bad)}")
for b in bad[:2]:
    print(f"  unparseable tail: {b[:150]!r}")
marked = [e for e in good if "ENOSPC-MARKER" in json.dumps(e)]
swallowed = [b for b in bad if "ENOSPC-MARKER" in b]
print(f"marker on a readable event: {bool(marked)}   marker inside an unparseable line: {bool(swallowed)}")
err = open(log, errors="replace").read().lower()
print(f"kernel said no space: {any(w in err for w in ('no space', 'enospc'))}")
PY
echo "kernel alive after: $ALIVE" >> "$OUT"
rm -rf "$ROOT" 2>/dev/null || true
INNER

echo "=== en-02 on $("$K" --version 2>&1 | head -1) ==="
sed 's/^/  /' "$OUT" | cut -c1-170

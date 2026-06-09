#!/usr/bin/env bash
# Broad end-to-end proof for arbos + pi. Re-run this script to verify the full
# chain: build, fake zero-setup, live LLM (OpenRouter via doppler), tools, control
# seam, delegation, session persistence.
#
# Usage:
#   ./scripts/e2e-live.sh              # fake + live (needs doppler OPENROUTER key)
#   ./scripts/e2e-live.sh --fake-only  # skip live LLM calls
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PATH="${HOME}/go-sdk/go/bin:${HOME}/.cargo/bin:${PATH}"

FAKE_ONLY=0
[[ "${1:-}" == "--fake-only" ]] && FAKE_ONLY=1

pass=0
fail=0
check() {
	local name="$1"
	shift
	if "$@"; then
		echo "PASS: $name"
		pass=$((pass + 1))
	else
		echo "FAIL: $name" >&2
		fail=$((fail + 1))
	fi
}

contains() {
	local haystack="$1"
	local needle="$2"
	[[ "$haystack" == *"$needle"* ]]
}

echo "=== static gate ==="
go build ./...
go vet ./...
go test ./... -race
make generate-check
golangci-lint run

echo "=== build binary ==="
go build -o /tmp/arbos-e2e ./cmd/arbos
ARBOS=/tmp/arbos-e2e

echo "=== fake zero-setup ==="
unset OPENROUTER_API_KEY ARBOS_OPENAI_BASE_URL ARBOS_OPENAI_API_KEY
WORKDIR=$(mktemp -d)
cd "$WORKDIR"
OUT=$("$ARBOS" -db "$WORKDIR/fake.db" -prompt "please use the tool" 2>&1)
echo "$OUT" | tail -8
check "fake ls tool" contains "$OUT" '✓ ls'
check "fake turn complete" contains "$OUT" 'deterministic fake response'

OUT=$("$ARBOS" -db "$WORKDIR/fake.db" -prompt "please fetch the page" 2>&1)
check "fake fetch tool" contains "$OUT" '✓ fetch'

echo "=== control seam (fake) ==="
cd "$ROOT"
go test ./internal/control/... -race -count=1

if [[ "$FAKE_ONLY" -eq 1 ]]; then
	echo "=== skipped live LLM (--fake-only) ==="
	echo "RESULT: $pass passed, $fail failed"
	exit "$fail"
fi

echo "=== load OpenRouter from doppler ==="
export OPENROUTER_API_KEY="$(doppler secrets get OPENROUTER_API_KEY --project arbos --config dev --plain)"
export ARBOS_MODEL="${ARBOS_MODEL:-google/gemini-2.5-flash}"
unset ARBOS_OPENAI_BASE_URL ARBOS_OPENAI_API_KEY

LIVE=$(mktemp -d)
cd "$LIVE"
echo "hello world" > hello.txt
DB="$LIVE/live.db"

echo "=== live read ==="
OUT=$("$ARBOS" -db "$DB" -prompt 'Use read on hello.txt. Reply with ONLY the file contents.' 2>&1)
echo "$OUT" | tail -10
check "live read" contains "$OUT" 'hello world'

echo "=== live write ==="
OUT=$("$ARBOS" -db "$DB" -prompt 'Use write to create marker.txt with exactly: arbos-live-ok' 2>&1)
check "live write disk" test -f marker.txt && grep -q "arbos-live-ok" marker.txt

echo "=== live edit ==="
echo "original line" > editme.txt
OUT=$("$ARBOS" -db "$DB" -prompt 'You must use tools. First read editme.txt, then use edit to replace the word original with edited. Do both now.' 2>&1)
echo "$OUT" | tail -8
check "live edit disk" grep -q "edited line" editme.txt
check "live edit tool called" contains "$OUT" '-> edit('

echo "=== live bash ==="
"$ARBOS" -db "$DB" -prompt 'Use bash: echo bash-ok > bash_out.txt' >/dev/null 2>&1
check "live bash disk" test -f bash_out.txt && grep -q "bash-ok" bash_out.txt

echo "=== live grep ==="
echo "unique token grepproof42" > hay.txt
OUT=$("$ARBOS" -db "$DB" -prompt 'Use grep for grepproof42 in hay.txt' 2>&1)
check "live grep output" contains "$OUT" 'grepproof42'

echo "=== live control seam ==="
python3 - "$ARBOS" "$DB" <<'PY'
import json, subprocess, sys, os
arbos, db = sys.argv[1], sys.argv[2]
env = os.environ.copy()
p = subprocess.Popen([arbos, "-db", db, "-serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=env)
def send(obj):
    p.stdin.write(json.dumps(obj)+"\n")
    p.stdin.flush()
send({"type":"open"})
opened = json.loads(p.stdout.readline())
assert opened["type"] == "opened"
send({"type":"intent","intent":{"kind":"prompt","text":"Reply with exactly: SEAM_OK"}})
for _ in range(300):
    line = p.stdout.readline()
    if not line:
        break
    msg = json.loads(line)
    if msg.get("type") == "event" and msg["envelope"]["event"].get("kind") == "turn_complete":
        p.terminate()
        print("ok")
        sys.exit(0)
p.terminate()
sys.exit(1)
PY
check "live control seam" true

echo "=== live delegation ==="
SUBDIR="$LIVE/subrepo"
mkdir -p "$SUBDIR"
echo "sub content" > "$SUBDIR/inside.txt"
OUT=$("$ARBOS" -db "$DB" -prompt "Use delegate with cwd=$SUBDIR and instruction: read inside.txt and return its exact contents only" 2>&1)
echo "$OUT" | tail -12
check "delegation read subrepo" contains "$OUT" 'sub content'

echo "=== session persistence ==="
"$ARBOS" -db "$DB" -prompt 'Reply ONLY: saved' >/dev/null 2>&1
EVENTS=$(python3 - "$DB" <<'PY'
import sqlite3, sys
db = sys.argv[1]
con = sqlite3.connect(db)
print(con.execute("SELECT COUNT(*) FROM events").fetchone()[0])
PY
)
check "sqlite has persisted events" test "${EVENTS:-0}" -gt 5
echo "  (persisted event count: $EVENTS)"

echo "=== live fetch ==="
OUT=$("$ARBOS" -db "$DB" -prompt 'Use fetch on https://example.com. Reply with ONLY the HTTP status code number.' 2>&1)
check "live fetch" contains "$OUT" '200'

echo "=== live memory ==="
OUT=$("$ARBOS" -db "$DB" -prompt 'Use memory action remember key=e2e value=memory-ok' 2>&1)
check "live memory tool" contains "$OUT" '-> memory('

echo "=== live session resume ==="
OUT1=$("$ARBOS" -db "$DB" -prompt 'Write resume.txt with exactly: resumed-ok' 2>&1)
SID=$(echo "$OUT1" | sed -n 's/^session //p' | head -1)
OUT2=$("$ARBOS" -db "$DB" -session "$SID" -prompt 'Read resume.txt. Reply with its contents only.' 2>&1)
check "live session resume" contains "$OUT2" 'resumed-ok'

echo ""
echo "RESULT: $pass passed, $fail failed (live workdir: $LIVE)"
exit "$fail"

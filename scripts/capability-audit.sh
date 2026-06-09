#!/usr/bin/env bash
# Capability audit for arbos pi agent — exercises all 13 audit dimensions.
# Usage:
#   ./scripts/capability-audit.sh              # fake + live (needs doppler)
#   ./scripts/capability-audit.sh --fake-only
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

contains() { [[ "$1" == *"$2"* ]]; }

echo "=== build ==="
go build -o /tmp/arbos-audit ./cmd/arbos
go build -o /tmp/arbos-tui-audit ./cmd/arbos-tui
ARBOS=/tmp/arbos-audit
check "tui builds" test -x /tmp/arbos-tui-audit

echo "=== 1-13 fake checks ==="
unset OPENROUTER_API_KEY ARBOS_OPENAI_BASE_URL ARBOS_OPENAI_API_KEY ARBOS_MCP_CONFIG
WORKDIR=$(mktemp -d)
cd "$WORKDIR"

OUT=$("$ARBOS" -db "$WORKDIR/f.db" -prompt "please fetch the page" 2>&1)
check "1-web-fetch-fake" contains "$OUT" '-> fetch('

OUT=$("$ARBOS" -db "$WORKDIR/f.db" -prompt "please use the tool" 2>&1)
check "2-tool-use-fake" contains "$OUT" '-> ls('

OUT=$("$ARBOS" -db "$WORKDIR/f.db" -prompt 'Use memory action remember key=k value=v. Then recall key=k.' 2>&1)
check "10-memory-fake" contains "$OUT" '-> memory('
check "10-memory-persist" test -f .arbos/memory/memories.json

export ARBOS_MCP_CONFIG='{"servers":[{"name":"fake","command":"python3","args":["'"$ROOT"'/scripts/fake-mcp-server.py"]}]}'
OUT=$("$ARBOS" -db "$WORKDIR/mcp.db" -prompt 'You must call fake__ping with empty args now.' 2>&1)
check "13-mcp-wired" contains "$OUT" '-> fake__ping'
check "13-mcp-call" contains "$OUT" 'pong'

OUT=$("$ARBOS" -db "$WORKDIR/ext.db" -prompt 'Call arbos_version with {}. Reply with tool output only.' 2>&1)
check "12-extension-tool" contains "$OUT" 'arbos pi'

cd "$ROOT"
go test ./internal/control/... -run Drain -count=1 >/dev/null
check "13-control-drain" true

if [[ "$FAKE_ONLY" -eq 1 ]]; then
	echo ""
	echo "AUDIT: $pass passed, $fail failed (fake-only)"
	exit "$fail"
fi

echo "=== live checks (OpenRouter) ==="
export OPENROUTER_API_KEY="$(doppler secrets get OPENROUTER_API_KEY --project arbos --config dev --plain)"
export ARBOS_MODEL="${ARBOS_MODEL:-google/gemini-2.5-flash}"
unset ARBOS_OPENAI_BASE_URL ARBOS_OPENAI_API_KEY ARBOS_MCP_CONFIG

LIVE=$(mktemp -d)
cd "$LIVE"
DB="$LIVE/live.db"

OUT=$("$ARBOS" -db "$DB" -prompt 'Use fetch on https://example.com. Reply with ONLY the HTTP status code number from the response.' 2>&1)
check "1-web-fetch-live" contains "$OUT" '200'

OUT=$("$ARBOS" -db "$DB" -prompt 'Use memory action remember key=audit_color value=ultramarine' 2>&1)
check "10-memory-live-write" contains "$OUT" '-> memory('

OUT1=$("$ARBOS" -db "$DB" -prompt 'Write notes.txt with line: color=ultramarine' 2>&1)
SID=$(echo "$OUT1" | sed -n 's/^session //p' | head -1)
OUT2=$("$ARBOS" -db "$DB" -session "$SID" -prompt 'Read notes.txt. Reply with color value only.' 2>&1)
check "10-session-resume" contains "$OUT2" 'ultramarine'

mkdir -p .arbos/skills/audit-skill
cat > .arbos/skills/audit-skill/SKILL.md <<'EOF'
---
name: audit-skill
description: Audit skill test - reply CODeword-AUDIT-OK when asked
---
When asked the audit codeword, reply exactly: CODEWORD-AUDIT-OK
EOF
OUT=$("$ARBOS" -db "$DB" -prompt 'Read .arbos/skills/audit-skill/SKILL.md and reply with the exact codeword from it.' 2>&1)
check "11-skills-live" contains "$OUT" 'CODEWORD-AUDIT-OK'

python3 - "$ARBOS" "$DB" <<'PY'
import json, subprocess, sys, os
arbos, db = sys.argv[1], sys.argv[2]
p = subprocess.Popen([arbos, "-db", db, "-serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=os.environ.copy())
def send(o):
    p.stdin.write(json.dumps(o)+"\n"); p.stdin.flush()
send({"type":"open"})
assert json.loads(p.stdout.readline())["type"] == "opened"
send({"type":"set_model","model":"google/gemini-2.5-flash"})
send({"type":"intent","intent":{"kind":"prompt","text":"Reply OK"}})
for _ in range(300):
    line = p.stdout.readline()
    if not line: break
    if json.loads(line).get("type")=="event":
        ev = json.loads(line)["envelope"]["event"]
        if ev.get("kind")=="turn_complete":
            p.terminate(); sys.exit(0)
p.terminate(); sys.exit(1)
PY
check "13-rpc-set_model" true

echo ""
echo "AUDIT: $pass passed, $fail failed (live workdir: $LIVE)"
exit "$fail"

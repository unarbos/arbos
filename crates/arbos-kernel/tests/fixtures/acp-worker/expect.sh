#!/bin/sh
set -e
t=.arbos/agents/root/transcript.jsonl
grep -q '"kind":"thinking"' "$t"
grep -q '"name":"acp:read"' "$t"
grep -q '"kind":"assistant","text":"Wrote out.txt with: hello from the fixture' "$t"
grep -q '"kind":"turn_complete"' "$t"
test -f out.txt
grep -q 'done by acp: hello from the fixture' out.txt

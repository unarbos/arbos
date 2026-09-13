#!/bin/sh
# An inbox node with wake fires a turn; the scripted model answers once;
# the turn completes; the node is done.
set -e
t=.arbos/agents/root/transcript.jsonl
grep -q '"kind":"assistant","text":"Hello from the fixture."' "$t"
grep -q '"kind":"turn_complete"' "$t"
tail -n 1 .arbos/agents/root/plan.jsonl | grep -q '"status":"done"'

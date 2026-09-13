#!/bin/sh
# A waking inbox message fires a turn; the scripted model answers once;
# the turn completes; the claimed message's turn folder records success.
set -e
t=.arbos/agents/root/transcript.jsonl
grep -q '"kind":"assistant","text":"Hello from the fixture."' "$t"
grep -q '"kind":"turn_complete"' "$t"
grep -q 'verdict = "success"' .arbos/agents/root/turns/t0001/meta.toml

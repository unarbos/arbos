#!/bin/sh
# The authored plan.jsonl holds one pending message node from an older
# kernel: the start migrates it to an inbox file, the file fires a turn,
# the scripted model answers once, the turn completes and its folder
# closes as a success.
set -e
t=.arbos/agents/root/transcript.jsonl
grep -q '"kind":"assistant","text":"Hello from the fixture."' "$t"
grep -q '"kind":"turn_complete"' "$t"
test -e .arbos/agents/root/plan.jsonl.migrated
grep -q 'verdict = "success"' .arbos/agents/root/turns/t0001/meta.toml

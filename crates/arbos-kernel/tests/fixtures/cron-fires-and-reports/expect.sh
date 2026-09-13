#!/bin/sh
# The node was due at 09:00:00Z; the kernel's clock started at 09:00:01Z.
# It fires once, its report lands on root's transcript as a say to the
# user, and the node is done.
set -e
t=.arbos/agents/root/transcript.jsonl
grep -q '"kind":"say"' "$t"
grep -q 'BTC: 42' "$t"
tail -n 1 .arbos/agents/root/plan.jsonl | grep -q '"status":"done"'

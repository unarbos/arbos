#!/bin/sh
# The subscription was due at 09:00:00Z; the kernel's clock started at
# 09:00:01Z. It fires once with no model turn, its reading lands on root's
# transcript as a say to the user, and the one-shot file is removed.
set -e
t=.arbos/agents/root/transcript.jsonl
grep -q '"kind":"say"' "$t"
grep -q 'BTC: 42' "$t"
test ! -e .arbos/agents/root/subscriptions/0001-btc-price.toml

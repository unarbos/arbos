#!/bin/sh
set -e
test -f ticks.txt
grep -q shell-tick ticks.txt
f=.arbos/agents/root/subscriptions/0001-tick.toml
grep -q '^id = 1' "$f"
grep -q '^created = ' "$f"
grep -q '^next_due = ' "$f"
grep -q 'subscription_unreadable' .arbos/runtime/kernel.log
grep -q '0002-broken.toml' .arbos/runtime/kernel.log

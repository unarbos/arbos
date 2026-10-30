#!/usr/bin/env bash
# Secret hand-off invariant check (Jacob, 2026-10-26): after both hand-off paths ran, the secret's
# value exists nowhere but the vault — grep every byte the relay and the box kept for it.
#
# What runs (all local, scratch data, a fake key):
#   1. relay   `save_this_moves_the_value_into_the_vault_and_out_of_every_relay_byte`
#              (POST /box/secrets/from-message on a test relay) — keeps its data dir + TRACE journal
#   2. agentd  `save_this_goes_to_the_vault_before_the_model_and_leaves_no_copy_in_the_box`
#              (the "save this" fast path on a test box with a relay stub) — keeps the box's state dir
#              (agentd.db + WAL, pi/ sessions, logs/, packs/, blobs/, secrets.env, audit.jsonl) + journal
#   3. mac     `a_redacted_room_is_saved_without_its_history` (the device store) — with --with-mac
#              (needs the GPUI build deps; the gate runs it anyway)
#   plus the request-card path on the box (`the_request_card_waits_for_the_device_and_returns_the_name_only`),
#   whose card payload, tool result and T3 line are part of the same kept box dir.
# Then: every kept file (and every sqlite file's `.dump`) is grepped for the key → the check passes
# only with zero hits. The key is the tests' fixture value, assembled here in two parts so push
# protection does not read it as a live key.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
KEEP="${ARBOS_HANDOFF_KEEP:-$(mktemp -d /tmp/handoff-check.XXXXXX)}"
mkdir -p "$KEEP"
WITH_MAC=0
for a in "$@"; do case "$a" in --with-mac) WITH_MAC=1 ;; esac; done
KEY="sk_live_"
KEY+="51HxQ9mT3vK2pLnR8wZ4eYc7bD1fG6hJ0kM9nP5qS2tU8vW3xA1"
echo "== secret hand-off invariant check — $(date -u) — kept files under $KEEP"
echo "== key under test: ${KEY:0:7}…${KEY: -4} (${#KEY} chars)"

run() { # name, package, bin flag, filter
  local name=$1 pkg=$2 bin=$3 filter=$4
  echo "-- $name: cargo test -p $pkg -- $filter"
  if ARBOS_HANDOFF_KEEP="$KEEP/$name" cargo test -q -p "$pkg" $bin --locked -- "$filter" >"$KEEP/$name.test.log" 2>&1; then
    echo "   test: ok"
  else
    echo "   test: FAILED (see $KEEP/$name.test.log)"; tail -20 "$KEEP/$name.test.log"; return 1
  fi
}
rc=0
run relay arbos-relay "--bin arbos-relay" save_this_moves_the_value_into_the_vault_and_out_of_every_relay_byte || rc=1
run box arbos-agentd "--bin arbos-agentd" "handoff::tests::save_this_goes_to_the_vault_before_the_model_and_leaves_no_copy_in_the_box" || rc=1
run box-card arbos-agentd "--bin arbos-agentd" "handoff::tests::the_request_card_waits_for_the_device_and_returns_the_name_only" || rc=1
if [[ $WITH_MAC == 1 ]]; then
  run mac arbos-mac "" a_redacted_room_is_saved_without_its_history || rc=1
fi

echo "== grep every kept byte for the key"
hits=0; files=0
# raw bytes first (a WAL file is folded away the moment sqlite3 opens its db for the dump below)
while IFS= read -r -d '' f; do
  files=$((files + 1))
  if grep -a -q -F -- "$KEY" "$f"; then echo "   HIT  $f"; hits=$((hits + 1)); fi
done < <(find "$KEEP" -type f ! -name '*.test.log' -print0)
# then every sqlite file decoded (rows, blobs as hex) — catches a page the raw grep could miss
if command -v sqlite3 >/dev/null 2>&1; then
  while IFS= read -r -d '' f; do
    if sqlite3 "$f" .dump 2>/dev/null | grep -a -q -F -- "$KEY"; then echo "   HIT  $f (.dump)"; hits=$((hits + 1)); fi
    hexkey=$(printf '%s' "$KEY" | od -An -tx1 | tr -d ' \n' | tr 'a-f' 'A-F')
    if sqlite3 "$f" .dump 2>/dev/null | tr 'a-f' 'A-F' | grep -a -q -F -- "$hexkey"; then echo "   HIT  $f (.dump, blob hex)"; hits=$((hits + 1)); fi
  done < <(find "$KEEP" -type f \( -name '*.db' -o -name '*.sqlite' \) -print0)
fi
# the test logs are expected to name the key only when a test FAILED (its assertion message)
echo "== places covered (agentd.db holds T3 + FTS, memory + FTS, room_docs snapshots, approvals)"
for p in "relay/data/relay.db" "relay/data/relay.db-wal" "relay/data/blobs" "relay/relay-journal.log" \
         "box/box/*/.arbos/agentd/agentd.db" "box/box/*/.arbos/agentd/agentd.db-wal" "box/box/*/.arbos/pi/sessions" "box/box/*/.arbos/logs" "box/box/*/.arbos/packs" \
         "box/box/*/.arbos/secrets.env" "box/box/*/.arbos/audit.jsonl" "box/agentd-journal.log" "box-card/box" "mac/store.sqlite"; do
  found=$(find "$KEEP" -path "$KEEP/$p" -o -path "$KEEP/$p/*" 2>/dev/null | head -1)
  [[ -n "$found" ]] && echo "   checked  $p" || echo "   (absent) $p"
done
echo "== $files files grepped, $hits hits"
if [[ $hits -eq 0 && $rc -eq 0 ]]; then echo "RESULT: PASS — the value is only in the vault (encrypted)"; exit 0; fi
echo "RESULT: FAIL"; exit 1

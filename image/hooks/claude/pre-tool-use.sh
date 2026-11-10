#!/bin/bash
# Claude Code PreToolUse hook (D64): deny-list veto + audit before execution.
# Reads the hook JSON on stdin; exit 2 blocks the call and the stderr text reaches the model.
set -u
AUDIT="${ARBOS_AUDIT_PATH:-${ARBOS_STATE_DIR:-/work/.arbos}/audit.jsonl}"
input="$(cat)"
tool="$(printf '%s' "$input" | jq -r '.tool_name // ""')"
args="$(printf '%s' "$input" | jq -c '.tool_input // {}')"
session="$(printf '%s' "$input" | jq -r '.session_id // ""')"
hay="$tool $args"

audit() {
  local input_hash
  mkdir -p "$(dirname "$AUDIT")" 2>/dev/null || return 0
  input_hash="$(printf '%s' "$args" | sha256sum 2>/dev/null)" || return 0
  input_hash="${input_hash%% *}"
  jq -cn --arg session "$session" --arg tool "$tool" --arg denied "$1" --arg input_hash "$input_hash" \
    '{ts: (now * 1000 | floor), kind: "tool_call", role: "subagent", harness: "claude_code",
      session: $session, tool: $tool, denied: (if $denied == "" then null else $denied end), input_hash: $input_hash}' \
    2>/dev/null >> "$AUDIT" || true
}

block() {
  audit "$1"
  echo "Blocked by the Arbos deny-list ($1): $2" >&2
  exit 2
}

if printf '%s' "$hay" | grep -Eq 'dtn_[A-Za-z0-9]{20,}|DAYTONA_API_KEY|api\.daytona\.io/api/(api-keys|organizations)'; then
  block daytona-api-key "Daytona API keys never exist in a box"
fi
if printf '%s' "$hay" | grep -Eq '/(admin|v1/admin|pair/new|devices)(/|\?|[[:space:]]|$)'; then
  block relay-admin "relay admin endpoints are not reachable with a box token"
fi
case "$tool" in
  Bash|Write|Edit|MultiEdit)
    if printf '%s' "$hay" | grep -Eq '(^|[[:space:]]|["'"'"'])/work(/[^/[:space:]]+)?/\.arbos/(agentd|audit\.jsonl)'; then
      block arbos-state-tamper "agentd state and the audit log are not writable by the model"
    fi ;;
esac

audit ""
exit 0
